//! Preview-frame bucketing, cache keys, and temp paths for smooth scrubbing.
//!
//! Scrubbing asks for a preview at every pointer position. Quantizing those
//! positions into buckets lets the editor reuse decoded textures from a cache
//! and skip re-decoding positions it has already seen.

use std::path::{Path, PathBuf};

/// Cache key for a decoded preview frame: source path plus quantized media time.
pub type PreviewKey = (PathBuf, u64);

/// Width of a preview bucket, in milliseconds.
///
/// Sub-bucket pointer movement maps onto the same key, so a fast scrub extracts
/// far fewer frames and reuses cached textures when the pointer doubles back.
pub const PREVIEW_BUCKET_MS: u64 = 80;

/// Coarse preview cadence used only during playback.
///
/// Playback repaints the playhead at about 30 FPS, but the preview frame is
/// still extracted through FFmpeg seeks. Keeping playback previews to every six
/// normal preview buckets prevents the UI from launching a decoder seek for
/// every repaint.
pub const PLAYBACK_PREVIEW_BUCKET_MS: u64 = PREVIEW_BUCKET_MS * 6;

/// Quantizes a media position to the nearest preview bucket.
pub fn bucket_ms(media_ms: u64) -> u64 {
    let half = PREVIEW_BUCKET_MS / 2;
    (media_ms + half) / PREVIEW_BUCKET_MS * PREVIEW_BUCKET_MS
}

/// Quantizes a media position for low-cost playback preview refreshes.
pub fn playback_bucket_ms(media_ms: u64) -> u64 {
    media_ms / PLAYBACK_PREVIEW_BUCKET_MS * PLAYBACK_PREVIEW_BUCKET_MS
}

/// Returns the cache key for a source position.
pub fn preview_key(source: &Path, media_ms: u64) -> PreviewKey {
    (source.to_path_buf(), bucket_ms(media_ms))
}

/// Width of a timeline-thumbnail bucket, in milliseconds.
///
/// Thumbnails are coarse on purpose: snapping source positions to a wide bucket
/// keeps the number of decoded frames per clip small and lets thumbnails be
/// reused across zoom levels and small scrolls.
pub const THUMB_BUCKET_MS: u64 = 500;

/// Returns the cache key for a timeline thumbnail at a source position.
pub fn thumb_key(source: &Path, media_ms: u64) -> PreviewKey {
    (
        source.to_path_buf(),
        media_ms / THUMB_BUCKET_MS * THUMB_BUCKET_MS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    mod bucket_ms {
        use super::*;

        #[test]
        fn zero_stays_zero() {
            assert_eq!(bucket_ms(0), 0);
        }

        #[test]
        fn rounds_to_the_nearest_bucket() {
            assert_eq!(bucket_ms(95), PREVIEW_BUCKET_MS);
        }
    }

    mod preview_key {
        use super::*;

        #[test]
        fn near_positions_share_a_key() {
            let path = Path::new("clip.mp4");
            assert_eq!(preview_key(path, 1_000), preview_key(path, 1_010));
        }
    }

    mod playback_bucket_ms {
        use super::*;

        #[test]
        fn keeps_positions_inside_the_same_playback_bucket_together() {
            assert_eq!(playback_bucket_ms(PLAYBACK_PREVIEW_BUCKET_MS - 1), 0);
        }

        #[test]
        fn advances_on_the_next_playback_bucket() {
            assert_eq!(
                playback_bucket_ms(PLAYBACK_PREVIEW_BUCKET_MS),
                PLAYBACK_PREVIEW_BUCKET_MS
            );
        }

        #[test]
        fn one_second_playback_is_bounded_to_at_most_three_preview_extractions() {
            let unique_buckets: std::collections::BTreeSet<_> = (0..30)
                .map(|frame| playback_bucket_ms(frame * 33))
                .collect();
            assert!(unique_buckets.len() <= 3);
        }
    }

    mod thumb_key {
        use super::*;

        #[test]
        fn positions_in_the_same_bucket_share_a_key() {
            let path = Path::new("clip.mp4");
            assert_eq!(thumb_key(path, 1_000), thumb_key(path, 1_400));
        }
    }
}
