use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

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
        if let Some(audio) = &self.active {
            audio.player.set_speed(speed);
        }
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

        let file = File::open(&source.path).map_err(|error| format!("open audio: {error}"))?;
        let decoder = Decoder::try_from(file).map_err(|error| format!("decode audio: {error}"))?;

        player.append(decoder);
        player.set_volume(if self.muted { 0.0 } else { self.volume });
        player.set_speed(self.speed);

        if source.media_position_ms > 0 {
            // `try_seek` is best-effort: streaming sources may not support it.
            let _ = player.try_seek(Duration::from_millis(source.media_position_ms));
        }
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
