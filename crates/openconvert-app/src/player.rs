use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use openconvert_media::audio::{AudioStreamDecoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE};
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaybackTarget {
    Timeline,
    Convert,
}

pub struct AudioSource {
    pub path: PathBuf,
    /// Position to seek inside the media file (ms).
    pub media_position_ms: u64,
}

const SPEED_EPSILON: f32 = 0.001;
const STRETCH_BLOCK_FRAMES: usize = 1_024;

/// Interleaved-stereo channel count from [`openconvert_media::audio`], as the
/// non-zero type rodio's [`Source`] requires. Evaluated at compile time.
const AUDIO_CHANNELS: ChannelCount = match NonZeroU16::new(OUTPUT_CHANNELS) {
    Some(value) => value,
    None => panic!("OUTPUT_CHANNELS must be non-zero"),
};
/// Output sample rate from [`openconvert_media::audio`], as rodio's non-zero
/// [`SampleRate`]. Evaluated at compile time.
const AUDIO_SAMPLE_RATE: SampleRate = match NonZeroU32::new(OUTPUT_SAMPLE_RATE) {
    Some(value) => value,
    None => panic!("OUTPUT_SAMPLE_RATE must be non-zero"),
};

struct PitchPreservingSpeed<S>
where
    S: Source<Item = f32>,
{
    input: S,
    stretch: signalsmith_stretch::Stretch,
    speed: f32,
    channels: ChannelCount,
    sample_rate: SampleRate,
    total_duration: Option<Duration>,
    input_buffer: Vec<f32>,
    output_buffer: Vec<f32>,
    output_pos: usize,
    flushed: bool,
}

impl<S> PitchPreservingSpeed<S>
where
    S: Source<Item = f32>,
{
    fn new(input: S, speed: f32) -> Self {
        let channels = input.channels();
        let sample_rate = input.sample_rate();
        let total_duration = input
            .total_duration()
            .map(|duration| duration.div_f32(speed));
        Self {
            input,
            stretch: signalsmith_stretch::Stretch::preset_cheaper(
                u32::from(channels.get()),
                sample_rate.get(),
            ),
            speed,
            channels,
            sample_rate,
            total_duration,
            input_buffer: Vec::new(),
            output_buffer: Vec::new(),
            output_pos: 0,
            flushed: false,
        }
    }

    fn refill_output(&mut self) {
        self.output_buffer.clear();
        self.output_pos = 0;

        let channel_count = usize::from(self.channels.get());
        let input_frames = ((STRETCH_BLOCK_FRAMES as f32 * self.speed).ceil() as usize).max(1);
        let input_samples = input_frames * channel_count;

        self.input_buffer.clear();
        self.input_buffer.resize(input_samples, 0.0);
        let mut written = 0usize;
        for sample in &mut self.input_buffer {
            let Some(next) = self.input.next() else {
                break;
            };
            *sample = next;
            written += 1;
        }
        self.input_buffer.truncate(written);

        let remainder = self.input_buffer.len() % channel_count;
        if remainder != 0 {
            self.input_buffer
                .resize(self.input_buffer.len() + channel_count - remainder, 0.0);
        }

        if self.input_buffer.is_empty() {
            if !self.flushed {
                let output_samples = self.stretch.output_latency() * channel_count;
                self.output_buffer.resize(output_samples, 0.0);
                self.stretch.flush(&mut self.output_buffer);
                self.flushed = true;
            }
            return;
        }

        let output_frames = ((self.input_buffer.len() / channel_count) as f32 / self.speed)
            .ceil()
            .max(1.0) as usize;
        self.output_buffer
            .resize(output_frames * channel_count, 0.0);
        self.stretch
            .process(&self.input_buffer, &mut self.output_buffer);
    }
}

impl<S> Iterator for PitchPreservingSpeed<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        while self.output_pos >= self.output_buffer.len() {
            if self.flushed {
                return None;
            }
            self.refill_output();
        }

        let sample = self.output_buffer[self.output_pos];
        self.output_pos += 1;
        Some(sample)
    }
}

impl<S> Source for PitchPreservingSpeed<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
}
struct ActiveAudio {
    player: Player,
}

struct PlaybackClock {
    base_position_ms: u64,
    started_at: Instant,
}

pub struct PlaybackState {
    target: PlaybackTarget,
    playing: bool,
    speed: f32,
    volume: f32,
    muted: bool,
    /// Audio device sink, lazily opened on first play and reused for the whole
    /// app lifetime. Reopening this on every seek is what was crashing the
    /// process, so we keep one alive and just attach fresh Players to its mixer.
    stream: Option<MixerDeviceSink>,
    /// Set once when the audio device fails to open; we then degrade to
    /// video-only previews instead of retrying every play.
    stream_error: Option<String>,
    active: Option<ActiveAudio>,
    clock: Option<PlaybackClock>,
}

impl PlaybackState {
    pub fn new() -> Self {
        Self {
            target: PlaybackTarget::Timeline,
            playing: false,
            speed: 1.0,
            volume: 0.8,
            muted: false,
            stream: None,
            stream_error: None,
            active: None,
            clock: None,
        }
    }

    pub fn target(&self) -> PlaybackTarget {
        self.target
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    pub fn set_speed(&mut self, speed: f32) {
        if self.playing {
            let current_position_ms = self.elapsed_position_ms(self.logical_position_ms());
            self.clock = Some(PlaybackClock {
                base_position_ms: current_position_ms,
                started_at: Instant::now(),
            });
        }
        self.speed = speed;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.apply_volume();
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.apply_volume();
    }

    fn apply_volume(&self) {
        if let Some(audio) = &self.active {
            audio
                .player
                .set_volume(if self.muted { 0.0 } else { self.volume });
        }
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }

        let current_position_ms = self.elapsed_position_ms(self.logical_position_ms());
        self.playing = false;
        if let Some(audio) = &self.active {
            audio.player.pause();
        }
        self.active = None;
        self.clock = Some(PlaybackClock {
            base_position_ms: current_position_ms,
            started_at: Instant::now(),
        });
    }

    pub fn stop(&mut self) {
        self.playing = false;
        if let Some(audio) = &self.active {
            audio.player.pause();
        }
        self.active = None;
        self.clock = None;
    }

    /// Begin playback at the given timeline position. The logical playback clock
    /// is independent from any clip's audio, so the caller advances the playhead
    /// from this clock and reseats audio/video per clip. Audio is attached
    /// separately through [`PlaybackState::restart_audio`].
    pub fn start(&mut self, target: PlaybackTarget, base_position_ms: u64) {
        self.target = target;
        self.playing = true;
        self.clock = Some(PlaybackClock {
            base_position_ms,
            started_at: Instant::now(),
        });
    }
    /// Timeline-relative elapsed position.
    pub fn elapsed_position_ms(&self, fallback: u64) -> u64 {
        let Some(clock) = &self.clock else {
            return fallback;
        };
        if !self.playing {
            return clock.base_position_ms;
        }

        let elapsed_ms = clock.started_at.elapsed().as_secs_f64() * 1_000.0 * self.speed as f64;
        clock
            .base_position_ms
            .saturating_add(elapsed_ms.max(0.0) as u64)
    }

    pub fn restart_audio(&mut self, audio_source: Option<AudioSource>) -> Result<(), String> {
        self.stop_audio();

        let Some(source) = audio_source else {
            return Ok(());
        };

        if self.muted {
            return Ok(());
        }

        self.start_audio(source)
    }

    pub fn stop_audio(&mut self) {
        if let Some(audio) = &self.active {
            audio.player.pause();
        }
        self.active = None;
    }

    fn logical_position_ms(&self) -> u64 {
        self.clock
            .as_ref()
            .map(|clock| clock.base_position_ms)
            .unwrap_or(0)
    }

    fn start_audio(&mut self, source: AudioSource) -> Result<(), String> {
        let stream = self.ensure_stream()?;
        let player = Player::connect_new(stream.mixer());
        let decoder = LibavAudioSource::open(&source.path, source.media_position_ms)?;
        let audio: Box<dyn Source<Item = f32> + Send> = if needs_pitch_preserving_speed(self.speed)
        {
            Box::new(PitchPreservingSpeed::new(decoder, self.speed))
        } else {
            Box::new(decoder)
        };

        player.append(audio);
        player.set_volume(if self.muted { 0.0 } else { self.volume });
        player.play();

        self.active = Some(ActiveAudio { player });

        Ok(())
    }

    fn ensure_stream(&mut self) -> Result<&MixerDeviceSink, String> {
        if let Some(error) = &self.stream_error {
            return Err(error.clone());
        }
        if self.stream.is_none() {
            match DeviceSinkBuilder::open_default_sink() {
                Ok(stream) => self.stream = Some(stream),
                Err(error) => {
                    let message = format!("audio device unavailable: {error}");
                    self.stream_error = Some(message.clone());
                    return Err(message);
                }
            }
        }
        self.stream
            .as_ref()
            .ok_or_else(|| "audio device unavailable: stream was not initialized".to_owned())
    }
}

fn needs_pitch_preserving_speed(speed: f32) -> bool {
    (speed - 1.0).abs() > SPEED_EPSILON
}

/// A rodio [`Source`] that streams interleaved stereo `f32` samples from a
/// libav-backed [`AudioStreamDecoder`]. libav seeks the audio track inside a
/// video container correctly, where rodio's symphonia decoder reports a
/// successful seek but keeps playing from the start.
struct LibavAudioSource {
    decoder: AudioStreamDecoder,
    current: std::vec::IntoIter<f32>,
    exhausted: bool,
}

impl LibavAudioSource {
    fn open(path: &Path, media_position_ms: u64) -> Result<Self, String> {
        let decoder = AudioStreamDecoder::open(path, media_position_ms)
            .map_err(|error| format!("decode audio: {error}"))?;
        Ok(Self {
            decoder,
            current: Vec::new().into_iter(),
            exhausted: false,
        })
    }
}

impl Iterator for LibavAudioSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(sample) = self.current.next() {
                return Some(sample);
            }
            if self.exhausted {
                return None;
            }
            match self.decoder.next_chunk() {
                Ok(Some(chunk)) => self.current = chunk.into_iter(),
                Ok(None) | Err(_) => {
                    self.exhausted = true;
                    return None;
                }
            }
        }
    }
}

impl Source for LibavAudioSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        AUDIO_CHANNELS
    }

    fn sample_rate(&self) -> SampleRate {
        AUDIO_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};

    use rodio::buffer::SamplesBuffer;

    use super::*;

    fn channels(value: u16) -> ChannelCount {
        NonZeroU16::new(value).expect("test channel count is non-zero")
    }

    fn sample_rate(value: u32) -> SampleRate {
        NonZeroU32::new(value).expect("test sample rate is non-zero")
    }

    #[test]
    fn default_speed_does_not_need_pitch_preservation() {
        assert!(!needs_pitch_preserving_speed(1.0));
    }

    #[test]
    fn faster_speed_needs_pitch_preservation() {
        assert!(needs_pitch_preserving_speed(2.0));
    }

    #[test]
    fn pitch_preserving_speed_keeps_the_source_sample_rate() {
        let source = SamplesBuffer::new(channels(1), sample_rate(48_000), vec![0.0; 4_096]);
        let stretched = PitchPreservingSpeed::new(source, 2.0);

        assert_eq!(stretched.sample_rate().get(), 48_000);
    }

    #[test]
    fn pitch_preserving_speed_reports_shorter_duration_when_faster() {
        let source = SamplesBuffer::new(channels(1), sample_rate(48_000), vec![0.0; 48_000]);
        let stretched = PitchPreservingSpeed::new(source, 2.0);

        assert_eq!(stretched.total_duration(), Some(Duration::from_millis(500)));
    }

    fn build_half_silent_media(path: &Path) {
        // 4s mono tone, but volume forced to 0 for the first 2s. Seeking past
        // 2s must yield loud samples; seeking to 0 must yield near silence.
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100:duration=4",
                "-af",
                "volume=0:enable='lt(t,2)'",
                "-y",
            ])
            .arg(path)
            .status()
            .expect("ffmpeg builds the half-silent media asset");
        assert!(status.success(), "ffmpeg failed to build the asset");
    }

    fn max_abs_after_seek(path: &Path, media_position_ms: u64) -> f32 {
        LibavAudioSource::open(path, media_position_ms)
            .expect("audio source opens")
            .take(48_000)
            .fold(0.0_f32, |acc, sample| acc.max(sample.abs()))
    }

    #[test]
    fn seeked_decoder_starts_at_the_requested_mp3_position() {
        let dir = tempfile::tempdir().expect("temp dir exists");
        let path = dir.path().join("seek.mp3");
        build_half_silent_media(&path);

        assert!(max_abs_after_seek(&path, 3_000) > 0.03);
    }

    fn build_half_silent_video(path: &Path) {
        // Realistic timeline source: H.264 video + AAC audio, audio silent for
        // the first 2s. This is the multi-stream case the app actually plays.
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=160x120:rate=30:duration=4",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=1000:sample_rate=44100:duration=4",
                "-af",
                "volume=0:enable='lt(t,2)'",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(path)
            .status()
            .expect("ffmpeg builds the half-silent video asset");
        assert!(status.success(), "ffmpeg failed to build the video asset");
    }

    #[test]
    fn seeked_decoder_starts_at_the_requested_video_mp4_position() {
        let dir = tempfile::tempdir().expect("temp dir exists");
        let path = dir.path().join("seek_video.mp4");
        build_half_silent_video(&path);

        assert!(max_abs_after_seek(&path, 3_000) > 0.03);
    }

    #[test]
    fn unseeked_decoder_starts_silent_for_half_silent_media() {
        let dir = tempfile::tempdir().expect("temp dir exists");
        let path = dir.path().join("seek.mp3");
        build_half_silent_media(&path);

        assert!(max_abs_after_seek(&path, 0) < 0.05);
    }
}
