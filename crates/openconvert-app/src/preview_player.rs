//! Decode-ahead preview playback driven by a presentation clock.
//!
//! A background thread decodes frames from one source into a bounded channel
//! (so memory and decode-ahead are capped), and the UI thread presents the frame
//! whose source timestamp matches the playback clock. This replaces the old
//! "spawn FFmpeg per frame over a pipe" approach: no process churn, accurate
//! pacing, and instant in-process seeks.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;
use openconvert_media::{DecodedFrame, VideoDecoder};

/// Largest preview frame the decoder produces; sources fit inside this box with
/// aspect preserved. Keeping it modest bounds decode cost and upload bandwidth.
pub const PREVIEW_MAX_WIDTH: u32 = 960;
/// See [`PREVIEW_MAX_WIDTH`].
pub const PREVIEW_MAX_HEIGHT: u32 = 540;

/// Frames the decoder may run ahead of the clock (bounds memory + paces decode).
const DECODE_AHEAD: usize = 6;
/// Frames the UI may hold while waiting for their presentation time.
const MAX_PENDING: usize = 8;

/// A running decode-ahead player for a single source position.
pub struct PreviewPlayer {
    frames: Receiver<DecodedFrame>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    pending: VecDeque<DecodedFrame>,
}

impl PreviewPlayer {
    /// Opens `path`, seeks to `start_ms`, and begins decoding ahead. Returns
    /// `None` only if the worker thread cannot be spawned.
    pub fn start(path: PathBuf, start_ms: u64, ctx: egui::Context) -> Self {
        let (tx, frames) = sync_channel::<DecodedFrame>(DECODE_AHEAD);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();

        let worker = thread::spawn(move || {
            let mut decoder = match VideoDecoder::open(&path, PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT)
            {
                Ok(decoder) => decoder,
                Err(_) => return,
            };
            let _ = decoder.seek(start_ms);

            while !worker_stop.load(Ordering::Relaxed) {
                match decoder.next_frame() {
                    Ok(Some(frame)) if frame.pts_ms < start_ms => {
                        // libav seeks to the previous keyframe. Those pre-roll
                        // frames are needed internally to decode correctly, but
                        // presenting them makes resume/playback jump backward for
                        // several seconds on long-GOP sources.
                        continue;
                    }
                    Ok(Some(frame)) => {
                        if !send_blocking(&tx, frame, &worker_stop, &ctx) {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => return,
                }
            }
        });

        Self {
            frames,
            stop,
            worker: Some(worker),
            pending: VecDeque::new(),
        }
    }

    /// Returns the newest decoded frame whose source timestamp is at or before
    /// `media_position_ms`, dropping any older frames. Frames scheduled later
    /// stay buffered for a future call. Returns `None` when nothing is ready.
    pub fn frame_at(&mut self, media_position_ms: u64) -> Option<DecodedFrame> {
        while self.pending.len() < MAX_PENDING {
            match self.frames.try_recv() {
                Ok(frame) => self.pending.push_back(frame),
                Err(_) => break,
            }
        }

        let mut present = None;
        while self
            .pending
            .front()
            .is_some_and(|f| f.pts_ms <= media_position_ms)
        {
            present = self.pending.pop_front();
        }
        present
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // The worker checks `stop` between frames and inside the send retry,
            // so this joins promptly and leaves no orphaned decoder/thread.
            let _ = worker.join();
        }
    }
}

/// Sends a frame, retrying while the buffer is full but bailing out the moment a
/// stop is requested. Returns `false` if the player should terminate.
fn send_blocking(
    tx: &std::sync::mpsc::SyncSender<DecodedFrame>,
    frame: DecodedFrame,
    stop: &AtomicBool,
    ctx: &egui::Context,
) -> bool {
    let mut frame = frame;
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match tx.try_send(frame) {
            Ok(()) => {
                ctx.request_repaint();
                return true;
            }
            Err(TrySendError::Full(returned)) => {
                frame = returned;
                thread::sleep(Duration::from_millis(3));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn build_sample(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("sample.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=15:duration=1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-y",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg builds the test asset");
        assert!(status.success(), "ffmpeg failed to build the test asset");
        path
    }

    #[test]
    fn delivers_a_decoded_frame_for_the_clock_position() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let ctx = egui::Context::default();

        let mut player = PreviewPlayer::start(sample, 0, ctx);

        let mut frame = None;
        for _ in 0..300 {
            if let Some(decoded) = player.frame_at(900) {
                frame = Some(decoded);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Dropping `player` at end of scope must not hang (worker joins cleanly).
        assert!(frame.is_some());
    }

    #[test]
    fn drops_preroll_frames_after_a_keyframe_seek() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());
        let ctx = egui::Context::default();

        let mut player = PreviewPlayer::start(sample, 500, ctx);

        let mut preroll = None;
        for _ in 0..100 {
            if let Some(decoded) = player.frame_at(499) {
                preroll = Some(decoded);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(preroll.is_none());
    }
}
