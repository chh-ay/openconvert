use std::path::Path;

use serde::{Deserialize, Serialize};

use ffmpeg::media::Type;
use ffmpeg_next as ffmpeg;

/// Microseconds per second; libav reports container duration in this base.
const AV_TIME_BASE: i64 = 1_000_000;

/// Media metadata discovered by probing the container in-process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaProbeResult {
    /// Duration in milliseconds, if the container reports one.
    pub duration_ms: Option<u64>,
    /// Video width in pixels for the best video stream.
    pub width: Option<u32>,
    /// Video height in pixels for the best video stream.
    pub height: Option<u32>,
    /// Codec name for the best video stream.
    pub video_codec: Option<String>,
    /// Codec name for the best audio stream.
    pub audio_codec: Option<String>,
    /// Whether the source is a still image rather than timed video/audio.
    pub is_image: bool,
}

/// Errors returned while probing media metadata.
#[derive(Debug, thiserror::Error)]
pub enum MediaProbeError {
    /// Input path did not point to a file.
    #[error("media file does not exist: {0}")]
    MissingFile(String),

    /// libav could not open or read the media.
    #[error("failed to read media metadata")]
    Decode(#[from] ffmpeg::Error),
}

/// Probes media metadata in-process via libav — no `ffprobe` process is spawned.
///
/// Returns the container duration, the best video stream's size and codec, the
/// best audio stream's codec (its presence signals audio), and whether the
/// source is a still image.
pub fn probe_media(path: &Path) -> Result<MediaProbeResult, MediaProbeError> {
    if !path.is_file() {
        return Err(MediaProbeError::MissingFile(path.display().to_string()));
    }
    crate::decode::ensure_init();

    let input = ffmpeg::format::input(&path)?;

    let duration = input.duration();
    let duration_ms =
        (duration > 0).then(|| (i128::from(duration) * 1_000 / i128::from(AV_TIME_BASE)) as u64);

    let mut width = None;
    let mut height = None;
    let mut video_codec = None;
    if let Some(stream) = input.streams().best(Type::Video) {
        let parameters = stream.parameters();
        video_codec = Some(parameters.id().name().to_owned());
        if let Ok(context) = ffmpeg::codec::context::Context::from_parameters(parameters) {
            if let Ok(decoder) = context.decoder().video() {
                width = Some(decoder.width());
                height = Some(decoder.height());
            }
        }
    }

    let audio_codec = input
        .streams()
        .best(Type::Audio)
        .map(|stream| stream.parameters().id().name().to_owned());

    let is_image = is_still_image(video_codec.as_deref(), audio_codec.as_deref(), duration_ms);

    Ok(MediaProbeResult {
        duration_ms,
        width,
        height,
        video_codec,
        audio_codec,
        is_image,
    })
}

/// Classifies a source as a still image: a still-image video codec, no audio,
/// and no meaningful container duration. Motion-JPEG video and animated GIF/WebP
/// report a real duration, so they stay classified as video.
fn is_still_image(
    video_codec: Option<&str>,
    audio_codec: Option<&str>,
    duration_ms: Option<u64>,
) -> bool {
    let still_codec = matches!(
        video_codec,
        Some("png" | "mjpeg" | "jpeg" | "bmp" | "tiff" | "webp" | "ppm" | "pgm" | "gif")
    );
    let short = duration_ms.is_none_or(|duration| duration <= 100);
    still_codec && audio_codec.is_none() && short
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

    #[test]
    fn is_still_image_classifies_a_still_png_as_an_image() {
        assert!(is_still_image(Some("png"), None, None));
    }

    #[test]
    fn is_still_image_rejects_video_with_a_real_duration() {
        assert!(!is_still_image(Some("h264"), None, Some(5_000)));
    }

    #[test]
    fn is_still_image_rejects_an_image_codec_that_carries_audio() {
        assert!(!is_still_image(Some("png"), Some("aac"), None));
    }

    #[test]
    fn probe_media_reads_video_dimensions_from_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let result = probe_media(&sample).unwrap();

        assert_eq!((result.width, result.height), (Some(320), Some(240)));
    }

    #[test]
    fn probe_media_reports_a_positive_duration_for_video() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let result = probe_media(&sample).unwrap();

        assert!(result.duration_ms.is_some_and(|duration| duration > 0));
    }

    #[test]
    fn probe_media_does_not_classify_video_as_an_image() {
        let dir = tempfile::tempdir().unwrap();
        let sample = build_sample(dir.path());

        let result = probe_media(&sample).unwrap();

        assert!(!result.is_image);
    }

    #[test]
    fn probe_media_reports_missing_files() {
        let error = probe_media(Path::new("/nonexistent/openconvert-probe.mp4")).unwrap_err();

        assert!(matches!(error, MediaProbeError::MissingFile(_)));
    }
}
