//! In-process video decoding for interactive preview playback.
//!
//! This module links libav directly so the editor can seek instantly, decode
//! ahead of a presentation clock, and hand RGBA frames to the GPU without
//! spawning a process per frame.

use std::path::{Path, PathBuf};
use std::sync::Once;

use ffmpeg::format::{context::Input, input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context as Scaler, flag::Flags};
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;

/// Microseconds per second; libav's seek timestamps use this fixed base.
const AV_TIME_BASE: i64 = 1_000_000;

/// One decoded frame, scaled to the requested preview size and packed tightly
/// as RGBA8 (`width * height * 4` bytes, no row padding).
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// Presentation timestamp inside the source media, in milliseconds.
    pub pts_ms: u64,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Tightly packed RGBA8 pixels.
    pub rgba: Vec<u8>,
}

/// Errors returned while decoding video frames.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// A libav call failed.
    #[error("ffmpeg decode error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    /// The source had no decodable video stream.
    #[error("no video stream in {0}")]
    NoVideoStream(String),
    /// The source produced no decodable frame at the requested position.
    #[error("no frame decoded from {0}")]
    NoFrame(String),
}

/// A seekable, in-process decoder that yields RGBA frames at a fixed output size.
pub struct VideoDecoder {
    input: Input,
    decoder: ffmpeg::decoder::Video,
    scaler: Scaler,
    decoded_frame: Video,
    rgba_frame: Video,
    stream_index: usize,
    seconds_per_tick: f64,
    eof: bool,
}

impl VideoDecoder {
    /// Opens `path` and prepares to emit frames that fit within
    /// `max_width`×`max_height` while preserving the source aspect ratio.
    pub fn open(path: &Path, max_width: u32, max_height: u32) -> Result<Self, DecodeError> {
        ensure_init();

        let input = input(&path)?;
        let stream = input
            .streams()
            .best(Type::Video)
            .ok_or_else(|| DecodeError::NoVideoStream(path.display().to_string()))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let seconds_per_tick = time_base.numerator() as f64 / time_base.denominator().max(1) as f64;

        let decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = decoder_ctx.decoder().video()?;

        let (out_width, out_height) =
            fit_within(decoder.width(), decoder.height(), max_width, max_height);
        let scaler = Scaler::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGBA,
            out_width,
            out_height,
            Flags::BILINEAR,
        )?;

        Ok(Self {
            input,
            decoder,
            scaler,
            decoded_frame: Video::empty(),
            rgba_frame: Video::empty(),
            stream_index,
            seconds_per_tick,
            eof: false,
        })
    }

    /// Seeks to the keyframe at or before `ms` and flushes buffered frames.
    ///
    /// Frame-accurate positioning is left to the caller: decode forward and drop
    /// frames whose [`DecodedFrame::pts_ms`] is before the desired time.
    pub fn seek(&mut self, ms: u64) -> Result<(), DecodeError> {
        let timestamp = (ms as i64).saturating_mul(AV_TIME_BASE / 1_000);
        self.input.seek(timestamp, ..timestamp)?;
        self.decoder.flush();
        self.eof = false;
        Ok(())
    }

    /// Returns the next decoded frame, or `None` once the stream is exhausted.
    pub fn next_frame(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
        loop {
            if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                let ticks = self
                    .decoded_frame
                    .pts()
                    .or_else(|| self.decoded_frame.timestamp())
                    .unwrap_or(0);
                return self.scale(ticks).map(Some);
            }
            if self.eof {
                return Ok(None);
            }

            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() == self.stream_index {
                        self.decoder.send_packet(&packet)?;
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    self.decoder.send_eof()?;
                    self.eof = true;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn scale(&mut self, ticks: i64) -> Result<DecodedFrame, DecodeError> {
        self.scaler.run(&self.decoded_frame, &mut self.rgba_frame)?;

        let width = self.rgba_frame.width();
        let height = self.rgba_frame.height();
        let row_bytes = width as usize * 4;
        let stride = self.rgba_frame.stride(0);
        let source = self.rgba_frame.data(0);

        // sws may pad rows to an alignment boundary; copy row-by-row so the
        // output is tightly packed for direct GPU upload.
        let packed_len = row_bytes * height as usize;
        let rgba = if stride == row_bytes {
            source[..packed_len].to_vec()
        } else {
            let mut packed = vec![0u8; packed_len];
            for y in 0..height as usize {
                let src_start = y * stride;
                packed[y * row_bytes..(y + 1) * row_bytes]
                    .copy_from_slice(&source[src_start..src_start + row_bytes]);
            }
            packed
        };
        let pts_ms = (ticks as f64 * self.seconds_per_tick * 1_000.0).max(0.0) as u64;

        Ok(DecodedFrame {
            pts_ms,
            width,
            height,
            rgba,
        })
    }
}

/// Decodes a single frame at `at_ms`, scaled to fit within
/// `max_width`×`max_height`. It opens a short-lived decoder, so it suits
/// one-shot needs like timeline thumbnails where a persistent [`VideoDecoder`]
/// would be wasteful. The frame is the keyframe at or before `at_ms` (thumbnails
/// do not need frame accuracy); the cost is one in-process open, with no FFmpeg
/// process spawned.
pub fn decode_frame_at(
    path: &Path,
    at_ms: u64,
    max_width: u32,
    max_height: u32,
) -> Result<DecodedFrame, DecodeError> {
    let mut decoder = VideoDecoder::open(path, max_width, max_height)?;
    // Still-image inputs may not be seekable; decoding from the start still
    // yields their only frame, so a seek failure here is non-fatal.
    let _ = decoder.seek(at_ms);
    decoder
        .next_frame()?
        .ok_or_else(|| DecodeError::NoFrame(path.display().to_string()))
}

/// A seekable decoder that stays open across calls, for scrub/seek preview.
///
/// [`decode_frame_at`] opens a short-lived decoder per call, so the dominant
/// cost of dragging the playhead is repeatedly opening the container and
/// initialising the codec. `ScrubDecoder` keeps the decoder open and reopens
/// only when the source path or output box changes, so seeking within one clip
/// pays that cost once instead of once per frame.
#[derive(Default)]
pub struct ScrubDecoder {
    current: Option<ScrubSource>,
}

struct ScrubSource {
    path: PathBuf,
    max_width: u32,
    max_height: u32,
    decoder: VideoDecoder,
}

impl ScrubDecoder {
    /// Creates a decoder with no source open yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the keyframe at or before `at_ms`, scaled to fit within
    /// `max_width`×`max_height`, matching the frame a fresh [`decode_frame_at`]
    /// would produce. Reopens the underlying decoder only when `path` or the
    /// output box differs from the previous call.
    pub fn frame_at(
        &mut self,
        path: &Path,
        at_ms: u64,
        max_width: u32,
        max_height: u32,
    ) -> Result<DecodedFrame, DecodeError> {
        let reuse = self.is_open_for(path, max_width, max_height);
        if !reuse {
            self.open(path, max_width, max_height)?;
        }
        match self.decode_at(at_ms)? {
            Some(frame) => Ok(frame),
            // A reused decoder can sit at EOF — a still image whose only frame
            // was already consumed, or a non-seekable source. Reopen once and
            // retry so the result matches a fresh decode.
            None if reuse => {
                self.open(path, max_width, max_height)?;
                self.decode_at(at_ms)?
                    .ok_or_else(|| DecodeError::NoFrame(path.display().to_string()))
            }
            None => Err(DecodeError::NoFrame(path.display().to_string())),
        }
    }

    fn is_open_for(&self, path: &Path, max_width: u32, max_height: u32) -> bool {
        self.current.as_ref().is_some_and(|source| {
            source.path == path && source.max_width == max_width && source.max_height == max_height
        })
    }

    fn open(&mut self, path: &Path, max_width: u32, max_height: u32) -> Result<(), DecodeError> {
        self.current = Some(ScrubSource {
            path: path.to_path_buf(),
            max_width,
            max_height,
            decoder: VideoDecoder::open(path, max_width, max_height)?,
        });
        Ok(())
    }

    fn decode_at(&mut self, at_ms: u64) -> Result<Option<DecodedFrame>, DecodeError> {
        let source = self
            .current
            .as_mut()
            .expect("a source is open before decoding");
        // Still images may be non-seekable; decoding from the start still yields
        // their only frame, so a seek failure here is non-fatal.
        let _ = source.decoder.seek(at_ms);
        source.decoder.next_frame()
    }
}

/// Scales `width`×`height` to fit within the box while preserving aspect ratio
/// and never upscaling. Output dimensions are even and at least 2 px.
fn fit_within(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_width.max(2), max_height.max(2));
    }
    let scale = (max_width as f64 / width as f64)
        .min(max_height as f64 / height as f64)
        .min(1.0);
    let even = |value: f64| ((value.round() as u32).max(2)) & !1;
    (even(width as f64 * scale), even(height as f64 * scale))
}

pub(crate) fn ensure_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if ffmpeg::init().is_ok() {
            // Keep libav from writing decode warnings to stderr during preview.
            ffmpeg::util::log::set_level(ffmpeg::util::log::Level::Fatal);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn build_sample(dir: &Path) -> PathBuf {
        let path = dir.join("sample.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=10:duration=1",
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

    fn build_still_image(dir: &Path) -> PathBuf {
        let path = dir.join("still.png");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=64x48",
                "-frames:v",
                "1",
                "-y",
            ])
            .arg(&path)
            .status()
            .expect("ffmpeg builds the still image asset");
        assert!(
            status.success(),
            "ffmpeg failed to build the still image asset"
        );
        path
    }

    fn center_rgb(frame: &DecodedFrame) -> [u8; 3] {
        let x = frame.width as usize / 2;
        let y = frame.height as usize / 2;
        let index = (y * frame.width as usize + x) * 4;
        [
            frame.rgba[index],
            frame.rgba[index + 1],
            frame.rgba[index + 2],
        ]
    }

    #[test]
    fn decodes_first_frame_preserving_aspect_within_the_box() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let mut decoder = VideoDecoder::open(&sample, 160, 90).unwrap();
        let frame = decoder.next_frame().unwrap().expect("at least one frame");

        // 320x240 (4:3) fit within 160x90 -> 120x90, tightly packed RGBA.
        assert_eq!((frame.width, frame.height), (120, 90));
    }

    #[test]
    fn decode_frame_at_returns_a_scaled_frame_for_a_seek_position() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let frame = decode_frame_at(&sample, 500, 160, 160).expect("a frame near 500ms");

        // 320x240 (4:3) fit within the 160x160 box -> 160x120.
        assert_eq!((frame.width, frame.height), (160, 120));
    }

    #[test]
    fn scrub_decoder_returns_a_scaled_frame_for_a_seek_position() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let mut decoder = ScrubDecoder::new();
        let frame = decoder
            .frame_at(&sample, 500, 160, 160)
            .expect("a frame near 500ms");

        // 320x240 (4:3) fit within the 160x160 box -> 160x120.
        assert_eq!((frame.width, frame.height), (160, 120));
    }

    #[test]
    fn scrub_decoder_reuses_one_source_across_backward_seeks() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let mut decoder = ScrubDecoder::new();
        decoder
            .frame_at(&sample, 700, 160, 160)
            .expect("a frame near 700ms");
        let frame = decoder
            .frame_at(&sample, 200, 160, 160)
            .expect("a frame near 200ms after a backward seek on the reused decoder");

        assert_eq!((frame.width, frame.height), (160, 120));
    }

    #[test]
    fn decode_frame_at_decodes_a_real_still_image() {
        let dir = tempfile::tempdir().unwrap();
        let image = build_still_image(dir.path());

        let frame = decode_frame_at(&image, 500, 160, 160).expect("the still image decodes");
        let [red, green, blue] = center_rgb(&frame);

        assert!(
            red > 200 && green < 40 && blue < 40,
            "center pixel was [{red}, {green}, {blue}]"
        );
    }

    #[test]
    fn scrub_decoder_decodes_a_real_still_image_after_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let image = build_still_image(dir.path());

        let mut decoder = ScrubDecoder::new();
        decoder
            .frame_at(&image, 0, 160, 160)
            .expect("the first still image decode succeeds");
        let frame = decoder
            .frame_at(&image, 500, 160, 160)
            .expect("the reused still image decoder reopens at eof");

        assert_eq!((frame.width, frame.height), (64, 48));
    }
}
