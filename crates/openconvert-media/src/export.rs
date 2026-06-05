use openconvert_core::Timeline;

/// Output container — the file an export produces.
///
/// The container fixes the muxer, file extension, and audio codec. The video
/// codec is a separate choice ([`VideoCodec`]) for the containers that allow
/// one; WebM is always VP9 and MP3 has no video stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// MP4 container with AAC audio.
    Mp4,
    /// Matroska container with AAC audio.
    Mkv,
    /// WebM container with VP9 video and Opus audio.
    WebM,
    /// QuickTime MOV container with AAC audio.
    Mov,
    /// MP3 audio-only output.
    Mp3,
}

impl Container {
    /// Returns the file extension for this container.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
            Container::Mov => "mov",
            Container::Mp3 => "mp3",
        }
    }

    /// Returns a short user-facing label for this container.
    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mkv => "MKV",
            Container::WebM => "WebM",
            Container::Mov => "MOV",
            Container::Mp3 => "MP3",
        }
    }

    /// Whether this container carries a video stream.
    pub fn has_video(self) -> bool {
        !matches!(self, Container::Mp3)
    }

    /// Whether the user may pick the video codec. WebM is always VP9 and MP3 has
    /// no video, so only MP4/MKV/MOV offer a choice.
    pub fn allows_video_codec_choice(self) -> bool {
        matches!(self, Container::Mp4 | Container::Mkv | Container::Mov)
    }
}

/// User-selectable video codec for containers that allow a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 / AVC — broadest playback compatibility.
    H264,
    /// H.265 / HEVC — smaller files, needs a newer decoder.
    H265,
}

impl VideoCodec {
    /// Returns a short user-facing label for this codec.
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
        }
    }
}

/// Export quality preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionPreset {
    /// Prefer quality over output size.
    HighQuality,
    /// Balance quality and output size.
    Balanced,
    /// Prefer smaller output size.
    Small,
}

impl CompressionPreset {
    /// Returns a user-facing label for this preset.
    pub fn label(self) -> &'static str {
        match self {
            CompressionPreset::HighQuality => "High quality",
            CompressionPreset::Balanced => "Balanced",
            CompressionPreset::Small => "Small file",
        }
    }

    /// Returns the default CRF-like video quality value for this preset.
    pub fn default_video_quality(self) -> u8 {
        match self {
            CompressionPreset::HighQuality => 18,
            CompressionPreset::Balanced => 23,
            CompressionPreset::Small => 30,
        }
    }

    /// Returns the default audio bitrate for this preset and container. MP3
    /// output is audio-only, so it is given more headroom than a soundtrack
    /// muxed beside video.
    pub fn default_audio_bitrate_kbps(self, container: Container) -> u16 {
        match (self, container) {
            (CompressionPreset::HighQuality, Container::Mp3) => 256,
            (CompressionPreset::Balanced, Container::Mp3) => 192,
            (CompressionPreset::Small, Container::Mp3) => 128,
            (CompressionPreset::HighQuality, _) => 192,
            (CompressionPreset::Balanced, _) => 128,
            (CompressionPreset::Small, _) => 96,
        }
    }
}

/// User-selected export settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    /// Output container.
    pub container: Container,
    /// Video codec, used when the container allows a choice.
    pub video_codec: VideoCodec,
    /// Compression preset used to derive defaults.
    pub compression: CompressionPreset,
    /// Video quality value passed to the selected encoder.
    pub video_quality: u8,
    /// Audio bitrate in kilobits per second.
    pub audio_bitrate_kbps: u16,
}

impl ExportOptions {
    /// A short description of the chosen container and codec for status text.
    pub fn summary(self) -> String {
        match self.container {
            Container::Mp3 => "MP3".to_owned(),
            Container::WebM => "WebM (VP9)".to_owned(),
            container => format!("{} ({})", container.label(), self.video_codec.label()),
        }
    }
}

impl Default for ExportOptions {
    fn default() -> Self {
        let compression = CompressionPreset::Balanced;

        Self {
            container: Container::Mp4,
            video_codec: VideoCodec::H264,
            compression,
            video_quality: compression.default_video_quality(),
            audio_bitrate_kbps: compression.default_audio_bitrate_kbps(Container::Mp4),
        }
    }
}

/// The composite output size: the first clip with known source dimensions, or a
/// 720p default. Shared with the preview so playback composites onto the same
/// canvas the export will render.
pub fn output_video_size(timeline: &Timeline) -> (u32, u32) {
    timeline
        .tracks()
        .iter()
        .flat_map(|track| track.clips())
        .find_map(|clip| {
            (clip.source_width > 0 && clip.source_height > 0)
                .then_some((clip.source_width, clip.source_height))
        })
        .unwrap_or((1280, 720))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp4_container_uses_the_mp4_extension() {
        assert_eq!(Container::Mp4.extension(), "mp4");
    }

    #[test]
    fn h265_codec_is_labelled_h265() {
        assert_eq!(VideoCodec::H265.label(), "H.265");
    }

    #[test]
    fn mp4_allows_a_video_codec_choice() {
        assert!(Container::Mp4.allows_video_codec_choice());
    }

    #[test]
    fn webm_does_not_allow_a_video_codec_choice() {
        assert!(!Container::WebM.allows_video_codec_choice());
    }

    #[test]
    fn mp3_has_no_video_stream() {
        assert!(!Container::Mp3.has_video());
    }

    #[test]
    fn summary_names_the_container_and_chosen_codec() {
        let options = ExportOptions {
            container: Container::Mp4,
            video_codec: VideoCodec::H265,
            ..ExportOptions::default()
        };

        assert_eq!(options.summary(), "MP4 (H.265)");
    }

    #[test]
    fn mp3_balanced_audio_bitrate_is_higher_than_video_defaults() {
        let bitrate = CompressionPreset::Balanced.default_audio_bitrate_kbps(Container::Mp3);

        assert_eq!(bitrate, 192);
    }

    #[test]
    fn output_video_size_uses_the_first_known_clip_dimensions() {
        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        let clip = timeline
            .add_clip(track, "input.mp4".to_owned(), 0, 0, 1_000)
            .unwrap();
        timeline.set_clip_video_size(track, clip, 640, 360).unwrap();

        assert_eq!(output_video_size(&timeline), (640, 360));
    }

    #[test]
    fn output_video_size_defaults_to_720p_when_sources_are_unknown() {
        assert_eq!(output_video_size(&Timeline::new()), (1280, 720));
    }
}
