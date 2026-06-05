use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rodio::decoder::DecoderBuilder;
use rodio::{
    ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source,
};

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
        let decoder = open_seeked_decoder(&source.path, source.media_position_ms)?;
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

fn open_seeked_decoder(
    path: &Path,
    media_position_ms: u64,
) -> Result<Decoder<BufReader<File>>, String> {
    let mut decoder = open_decoder(path, false)?;
    if media_position_ms == 0 {
        return Ok(decoder);
    }

    let position = Duration::from_millis(media_position_ms);
    if decoder.try_seek(position).is_ok() {
        return Ok(decoder);
    }

    let mut decoder = open_decoder(path, true)?;
    decoder
        .try_seek(position)
        .map_err(|error| format!("seek audio: {error}"))?;
    Ok(decoder)
}

fn open_decoder(path: &Path, coarse_seek: bool) -> Result<Decoder<BufReader<File>>, String> {
    let file = File::open(path).map_err(|error| format!("open audio: {error}"))?;
    let byte_len = file
        .metadata()
        .map_err(|error| format!("read audio metadata: {error}"))?
        .len();
    DecoderBuilder::new()
        .with_data(BufReader::new(file))
        .with_byte_len(byte_len)
        .with_seekable(true)
        .with_coarse_seek(coarse_seek)
        .build()
        .map_err(|error| format!("decode audio: {error}"))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
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

    #[test]
    fn seeked_decoder_starts_at_the_requested_wav_position() {
        let dir = tempfile::tempdir().expect("temp dir exists");
        let path = dir.path().join("seek.wav");
        write_test_wav(&path);

        let mut decoder = open_seeked_decoder(&path, 600).expect("decoder seeks into wav");

        assert!(decoder.next().is_some_and(|sample| sample > 0.0));
    }

    fn write_test_wav(path: &Path) {
        const SAMPLE_RATE: u32 = 1_000;
        const SAMPLE_COUNT: u32 = 1_000;
        const CHANNELS: u16 = 1;
        const BYTES_PER_SAMPLE: u16 = 2;
        const DATA_SIZE: u32 = SAMPLE_COUNT * BYTES_PER_SAMPLE as u32;

        let mut file = File::create(path).expect("wav fixture can be created");
        file.write_all(b"RIFF").expect("wav riff header");
        file.write_all(&(36 + DATA_SIZE).to_le_bytes())
            .expect("wav riff size");
        file.write_all(b"WAVE").expect("wav wave header");
        file.write_all(b"fmt ").expect("wav fmt chunk");
        file.write_all(&16u32.to_le_bytes()).expect("wav fmt len");
        file.write_all(&1u16.to_le_bytes()).expect("wav pcm tag");
        file.write_all(&CHANNELS.to_le_bytes())
            .expect("wav channels");
        file.write_all(&SAMPLE_RATE.to_le_bytes())
            .expect("wav sample rate");
        file.write_all(&(SAMPLE_RATE * BYTES_PER_SAMPLE as u32).to_le_bytes())
            .expect("wav byte rate");
        file.write_all(&(CHANNELS * BYTES_PER_SAMPLE).to_le_bytes())
            .expect("wav block align");
        file.write_all(&(BYTES_PER_SAMPLE * 8).to_le_bytes())
            .expect("wav bits per sample");
        file.write_all(b"data").expect("wav data chunk");
        file.write_all(&DATA_SIZE.to_le_bytes())
            .expect("wav data size");
        for index in 0..SAMPLE_COUNT {
            let sample: i16 = if index < 500 { -16_384 } else { 16_384 };
            file.write_all(&sample.to_le_bytes())
                .expect("wav sample data");
        }
    }
}
