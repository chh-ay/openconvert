//! In-process audio decoding for interactive preview playback.
//!
//! rodio/symphonia cannot reliably seek the audio track inside a *video*
//! container (an audio-only file seeks fine, but a `.mp4` carrying H.264 + AAC
//! reports a successful seek while the audio keeps playing from the start). The
//! preview therefore decodes audio through libav — the same library the exporter
//! uses — which seeks every container correctly. Output is interleaved stereo
//! `f32` at a fixed rate so it can be handed straight to the audio mixer.

use std::path::Path;

use ffmpeg::format::sample::Type as SampleType;
use ffmpeg::format::{context::Input, input, Sample};
use ffmpeg::media::Type;
use ffmpeg::software::resampling::Context as Resampler;
use ffmpeg::util::frame::audio::Audio as AudioFrame;
use ffmpeg::{ChannelLayout, Packet};
use ffmpeg_next as ffmpeg;

use crate::decode::{ensure_init, DecodeError};

/// Output sample rate of the streaming decoder, in Hz.
pub const OUTPUT_SAMPLE_RATE: u32 = 48_000;
/// Output channel count; samples are emitted interleaved as stereo.
pub const OUTPUT_CHANNELS: u16 = 2;

/// Microseconds per second; libav seek timestamps use this fixed base.
const AV_TIME_BASE: i64 = 1_000_000;

/// A seekable libav audio decoder that yields interleaved stereo `f32` samples
/// at [`OUTPUT_SAMPLE_RATE`].
///
/// Construction seeks to the requested position (container seeks land on a
/// packet at or before the target, then the leading samples are trimmed so the
/// first emitted sample is exactly at the requested time). Pull successive
/// blocks with [`AudioStreamDecoder::next_chunk`] until it returns `None`.
pub struct AudioStreamDecoder {
    input: Input,
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<Resampler>,
    stream_index: usize,
    seconds_per_tick: f64,
    decoded: AudioFrame,
    /// Source position (ms) below which decoded samples are dropped, so a
    /// keyframe-aligned container seek still starts sample-accurately.
    skip_before_ms: u64,
    eof: bool,
}

impl AudioStreamDecoder {
    /// Opens `path`, selects the best audio stream, and seeks so the first
    /// emitted sample is at `start_ms`.
    pub fn open(path: &Path, start_ms: u64) -> Result<Self, DecodeError> {
        ensure_init();

        let mut input = input(&path)?;
        let stream = input
            .streams()
            .best(Type::Audio)
            .ok_or_else(|| DecodeError::NoAudioStream(path.display().to_string()))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let seconds_per_tick = time_base.numerator() as f64 / time_base.denominator().max(1) as f64;

        let decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let mut decoder = decoder_ctx.decoder().audio()?;

        if start_ms > 0 {
            let timestamp = (start_ms as i64).saturating_mul(AV_TIME_BASE / 1_000);
            // Seek lands on a packet at or before the target; `trim_leading`
            // drops the overshoot so playback starts exactly at `start_ms`.
            let _ = input.seek(timestamp, ..timestamp);
            decoder.flush();
        }

        Ok(Self {
            input,
            decoder,
            resampler: None,
            stream_index,
            seconds_per_tick,
            decoded: AudioFrame::empty(),
            skip_before_ms: start_ms,
            eof: false,
        })
    }

    /// Returns the next block of interleaved stereo `f32` samples, or `None`
    /// once the stream is exhausted. The block length is always a multiple of
    /// [`OUTPUT_CHANNELS`].
    pub fn next_chunk(&mut self) -> Result<Option<Vec<f32>>, DecodeError> {
        loop {
            if self.decoder.receive_frame(&mut self.decoded).is_ok() {
                let frame_pts_ms = self
                    .decoded
                    .pts()
                    .or_else(|| self.decoded.timestamp())
                    .map(|ticks| (ticks as f64 * self.seconds_per_tick * 1_000.0).max(0.0) as u64)
                    .unwrap_or(0);
                let chunk = self.resample()?;
                if chunk.is_empty() {
                    continue;
                }
                if let Some(trimmed) = self.trim_leading(frame_pts_ms, chunk) {
                    return Ok(Some(trimmed));
                }
                continue;
            }
            if self.eof {
                return Ok(None);
            }

            let mut packet = Packet::empty();
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

    /// Drops the samples that precede the seek target. Returns `None` when the
    /// whole block is before the target (so the caller decodes the next one).
    fn trim_leading(&mut self, frame_pts_ms: u64, chunk: Vec<f32>) -> Option<Vec<f32>> {
        if self.skip_before_ms == 0 || frame_pts_ms >= self.skip_before_ms {
            self.skip_before_ms = 0;
            return Some(chunk);
        }

        let lead_ms = self.skip_before_ms - frame_pts_ms;
        let channels = usize::from(OUTPUT_CHANNELS);
        let skip_samples = (u64::from(OUTPUT_SAMPLE_RATE) * lead_ms / 1_000) as usize * channels;
        if skip_samples >= chunk.len() {
            return None;
        }
        self.skip_before_ms = 0;
        Some(chunk[skip_samples..].to_vec())
    }

    /// Resamples the current decoded frame to interleaved stereo `f32` at
    /// [`OUTPUT_SAMPLE_RATE`].
    fn resample(&mut self) -> Result<Vec<f32>, DecodeError> {
        if self.resampler.is_none() {
            self.resampler = Some(Resampler::get(
                self.decoded.format(),
                frame_layout(&self.decoded),
                self.decoded.rate(),
                Sample::F32(SampleType::Planar),
                ChannelLayout::STEREO,
                OUTPUT_SAMPLE_RATE,
            )?);
        }
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };

        let mut out = AudioFrame::empty();
        resampler.run(&self.decoded, &mut out)?;
        let frames = out.samples();
        if frames == 0 {
            return Ok(Vec::new());
        }

        let left = out.plane::<f32>(0);
        let right = out.plane::<f32>(1);
        let mut interleaved = Vec::with_capacity(frames * usize::from(OUTPUT_CHANNELS));
        for index in 0..frames {
            interleaved.push(left[index]);
            interleaved.push(right[index]);
        }
        Ok(interleaved)
    }
}

/// The channel layout of a decoded frame, falling back to stereo when the
/// source leaves the layout unset.
fn frame_layout(frame: &AudioFrame) -> ChannelLayout {
    if frame.channel_layout().channels() > 0 {
        frame.channel_layout()
    } else if frame.channels() > 0 {
        ChannelLayout::default(i32::from(frame.channels()))
    } else {
        ChannelLayout::STEREO
    }
}
