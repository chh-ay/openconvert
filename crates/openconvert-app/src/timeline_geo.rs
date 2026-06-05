//! Pure timeline geometry, zoom math, snapping, and drag resolution.
//!
//! This module is deliberately free of egui so the interactive timeline's math
//! can be unit-tested in isolation. The view layer turns these results into
//! painted rectangles and pointer handling.

use openconvert_core::{ClipId, TrackId};

/// Lowest timeline zoom, in pixels per second of media.
///
/// At 0.25 px/s, one hour of timeline occupies 900 px, so even a narrow lane
/// can fit an hour-scale project.
pub const MIN_PIXELS_PER_SECOND: f32 = 0.25;
/// Highest timeline zoom, in pixels per second of media.
pub const MAX_PIXELS_PER_SECOND: f32 = 1_400.0;
/// Default timeline zoom, in pixels per second of media.
pub const DEFAULT_PIXELS_PER_SECOND: f32 = 90.0;
/// Multiplier applied per zoom-in / zoom-out step.
pub const ZOOM_STEP: f32 = 1.25;

/// Pixel width of a clip trim handle and the hit margin used to grab it.
pub const TRIM_HANDLE_PX: f32 = 9.0;
/// Smallest duration a clip may be trimmed to, in milliseconds.
pub const MIN_CLIP_DURATION_MS: u64 = 100;
/// Target spacing between ruler ticks, in pixels.
pub const MIN_TICK_PX: f32 = 72.0;

/// Clamps a zoom value to the supported range.
pub fn clamp_pixels_per_second(value: f32) -> f32 {
    value.clamp(MIN_PIXELS_PER_SECOND, MAX_PIXELS_PER_SECOND)
}

/// Converts a millisecond span to a horizontal pixel span at the given zoom.
pub fn ms_to_px(ms: u64, pixels_per_second: f32) -> f32 {
    ms as f32 / 1_000.0 * pixels_per_second
}

/// Converts a horizontal pixel span back to milliseconds, saturating at zero.
pub fn px_to_ms(px: f32, pixels_per_second: f32) -> u64 {
    if px <= 0.0 || pixels_per_second <= 0.0 {
        return 0;
    }
    (px / pixels_per_second * 1_000.0).round() as u64
}

/// Snaps `target` to the nearest `candidate` within `threshold_ms`.
///
/// Returns `target` unchanged when no candidate is close enough, and the
/// closest candidate when several are within the threshold.
pub fn snap_ms(target: u64, candidates: &[u64], threshold_ms: u64) -> u64 {
    let mut best_candidate = target;
    let mut best_distance = threshold_ms.saturating_add(1);
    for &candidate in candidates {
        let distance = candidate.abs_diff(target);
        if distance <= threshold_ms && distance < best_distance {
            best_distance = distance;
            best_candidate = candidate;
        }
    }
    best_candidate
}

/// Snaps a moving clip so either its start or end aligns to a candidate.
///
/// Start alignment wins ties; returns the resolved start time.
pub fn snap_move_start_ms(
    start_ms: u64,
    duration_ms: u64,
    candidates: &[u64],
    threshold_ms: u64,
) -> u64 {
    let snapped_start = snap_ms(start_ms, candidates, threshold_ms);
    if snapped_start != start_ms {
        return snapped_start;
    }
    let end_ms = start_ms.saturating_add(duration_ms);
    let snapped_end = snap_ms(end_ms, candidates, threshold_ms);
    if snapped_end != end_ms {
        return snapped_end.saturating_sub(duration_ms);
    }
    start_ms
}

/// Region of a clip under the pointer, used to pick a drag intent and cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipZone {
    /// Left edge: trim the clip's start.
    TrimStart,
    /// Interior: move the whole clip.
    Body,
    /// Right edge: trim the clip's end.
    TrimEnd,
}

/// Classifies a pointer `x` within a clip rectangle spanning `[left_x, left_x + width)`.
///
/// Each trim handle is capped at a third of the clip so even narrow clips keep a
/// grabbable body.
pub fn clip_zone(left_x: f32, width: f32, pointer_x: f32, handle_px: f32) -> ClipZone {
    let handle = handle_px.min(width / 3.0).max(0.0);
    if pointer_x <= left_x + handle {
        ClipZone::TrimStart
    } else if pointer_x >= left_x + width - handle {
        ClipZone::TrimEnd
    } else {
        ClipZone::Body
    }
}

/// An in-progress timeline pointer drag.
///
/// Only the dragged element's identity is stored; the live geometry is recomputed
/// from the pointer each frame so a drag never mutates the model until release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineDrag {
    /// Scrubbing the playhead.
    Playhead,
    /// Moving a clip in time and/or onto another track.
    Move {
        /// Track the clip started on.
        track: TrackId,
        /// Clip being moved.
        clip: ClipId,
        /// Offset, in ms, from the clip start to the grabbed point.
        grab_offset_ms: u64,
    },
    /// Dragging the left edge to trim the clip's start.
    TrimStart {
        /// Track holding the clip.
        track: TrackId,
        /// Clip being trimmed.
        clip: ClipId,
    },
    /// Dragging the right edge to trim the clip's end.
    TrimEnd {
        /// Track holding the clip.
        track: TrackId,
        /// Clip being trimmed.
        clip: ClipId,
    },
}

impl TimelineDrag {
    /// Returns the clip identity targeted by this drag, if any.
    pub fn target(self) -> Option<(TrackId, ClipId)> {
        match self {
            TimelineDrag::Playhead => None,
            TimelineDrag::Move { track, clip, .. }
            | TimelineDrag::TrimStart { track, clip }
            | TimelineDrag::TrimEnd { track, clip } => Some((track, clip)),
        }
    }
}

/// Resolved clip placement on the timeline, shared by trim and move math.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipPlacement {
    /// Start on the output timeline, in milliseconds.
    pub timeline_start_ms: u64,
    /// Start offset inside the source media, in milliseconds.
    pub source_start_ms: u64,
    /// Clip duration, in milliseconds.
    pub duration_ms: u64,
}

/// Resolves a start-edge trim toward `desired_start_ms`, keeping the clip's
/// timeline end fixed.
///
/// The new start is clamped so the source never moves before `source_floor_ms`
/// and the clip keeps at least [`MIN_CLIP_DURATION_MS`].
pub fn resolve_trim_start(
    timeline_start_ms: u64,
    source_start_ms: u64,
    duration_ms: u64,
    source_floor_ms: u64,
    desired_start_ms: u64,
) -> ClipPlacement {
    let end_ms = timeline_start_ms.saturating_add(duration_ms);
    let max_left = source_start_ms.saturating_sub(source_floor_ms);
    let earliest = timeline_start_ms.saturating_sub(max_left);
    let latest = end_ms.saturating_sub(MIN_CLIP_DURATION_MS);
    let new_start = desired_start_ms.clamp(earliest, latest.max(earliest));

    let new_source_start = if new_start >= timeline_start_ms {
        source_start_ms.saturating_add(new_start - timeline_start_ms)
    } else {
        source_start_ms.saturating_sub(timeline_start_ms - new_start)
    };

    ClipPlacement {
        timeline_start_ms: new_start,
        source_start_ms: new_source_start,
        duration_ms: end_ms.saturating_sub(new_start),
    }
}

/// Resolves an end-edge trim toward `desired_end_ms`, keeping the clip's
/// timeline start fixed.
///
/// The end is clamped so the source never extends past `source_ceiling_ms` and
/// the clip keeps at least [`MIN_CLIP_DURATION_MS`].
pub fn resolve_trim_end(
    timeline_start_ms: u64,
    source_start_ms: u64,
    source_ceiling_ms: u64,
    desired_end_ms: u64,
) -> ClipPlacement {
    let max_duration = source_ceiling_ms.saturating_sub(source_start_ms);
    let min_end = timeline_start_ms.saturating_add(MIN_CLIP_DURATION_MS);
    let max_end = timeline_start_ms.saturating_add(max_duration);
    let new_end = desired_end_ms.clamp(min_end.min(max_end), max_end);

    ClipPlacement {
        timeline_start_ms,
        source_start_ms,
        duration_ms: new_end.saturating_sub(timeline_start_ms),
    }
}

/// Returns the track row index at vertical position `y` within the lanes area
/// that starts at `lanes_top` with rows of `track_height`.
pub fn track_index_at_y(
    y: f32,
    lanes_top: f32,
    track_height: f32,
    track_count: usize,
) -> Option<usize> {
    if y < lanes_top || track_height <= 0.0 || track_count == 0 {
        return None;
    }
    let index = ((y - lanes_top) / track_height) as usize;
    (index < track_count).then_some(index)
}

/// Returns a human-friendly ruler tick spacing, in milliseconds, so labels land
/// roughly every [`MIN_TICK_PX`] at the given zoom.
pub fn ruler_step_ms(pixels_per_second: f32) -> u64 {
    const STEPS: [u64; 16] = [
        100, 250, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000,
        600_000, 900_000, 1_800_000, 3_600_000,
    ];
    for step in STEPS {
        if ms_to_px(step, pixels_per_second) >= MIN_TICK_PX {
            return step;
        }
    }
    STEPS[STEPS.len() - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    mod clamp_pixels_per_second {
        use super::*;

        #[test]
        fn raises_values_below_the_floor() {
            assert_eq!(clamp_pixels_per_second(0.1), MIN_PIXELS_PER_SECOND);
        }

        #[test]
        fn lowers_values_above_the_ceiling() {
            assert_eq!(clamp_pixels_per_second(9_000.0), MAX_PIXELS_PER_SECOND);
        }
    }

    mod px_conversions {
        use super::*;

        #[test]
        fn round_trips_seconds_at_default_zoom() {
            let px = ms_to_px(2_000, DEFAULT_PIXELS_PER_SECOND);
            assert_eq!(px_to_ms(px, DEFAULT_PIXELS_PER_SECOND), 2_000);
        }

        #[test]
        fn negative_pixels_saturate_to_zero() {
            assert_eq!(px_to_ms(-50.0, DEFAULT_PIXELS_PER_SECOND), 0);
        }
    }

    mod snap_ms {
        use super::*;

        #[test]
        fn snaps_to_the_closest_candidate_in_range() {
            assert_eq!(snap_ms(1_020, &[500, 1_000, 1_500], 50), 1_000);
        }

        #[test]
        fn leaves_target_when_no_candidate_is_close_enough() {
            assert_eq!(snap_ms(1_200, &[500, 1_000, 1_500], 50), 1_200);
        }
    }

    mod clip_zone {
        use super::*;

        #[test]
        fn left_edge_is_trim_start() {
            assert_eq!(
                clip_zone(100.0, 200.0, 104.0, TRIM_HANDLE_PX),
                ClipZone::TrimStart
            );
        }

        #[test]
        fn center_is_body() {
            assert_eq!(
                clip_zone(100.0, 200.0, 200.0, TRIM_HANDLE_PX),
                ClipZone::Body
            );
        }

        #[test]
        fn right_edge_is_trim_end() {
            assert_eq!(
                clip_zone(100.0, 200.0, 296.0, TRIM_HANDLE_PX),
                ClipZone::TrimEnd
            );
        }

        #[test]
        fn narrow_clip_keeps_a_grabbable_body() {
            assert_eq!(clip_zone(0.0, 12.0, 6.0, TRIM_HANDLE_PX), ClipZone::Body);
        }
    }

    mod resolve_trim_start {
        use super::*;

        #[test]
        fn extends_left_until_the_source_floor() {
            // Clip at timeline 2s, source window [1000,2000): dragging the start
            // far left stops once the source offset reaches the floor (0).
            let placement = resolve_trim_start(2_000, 1_000, 1_000, 0, 0);
            assert_eq!(placement.source_start_ms, 0);
        }

        #[test]
        fn shrinking_keeps_minimum_duration() {
            let placement = resolve_trim_start(0, 0, 1_000, 0, 5_000);
            assert_eq!(placement.duration_ms, MIN_CLIP_DURATION_MS);
        }
    }

    mod resolve_trim_end {
        use super::*;

        #[test]
        fn extends_right_until_the_source_ceiling() {
            let placement = resolve_trim_end(0, 0, 5_000, 9_000);
            assert_eq!(placement.duration_ms, 5_000);
        }

        #[test]
        fn shrinking_keeps_minimum_duration() {
            let placement = resolve_trim_end(0, 0, 5_000, 10);
            assert_eq!(placement.duration_ms, MIN_CLIP_DURATION_MS);
        }
    }

    mod track_index_at_y {
        use super::*;

        #[test]
        fn maps_y_to_the_row_under_the_pointer() {
            assert_eq!(track_index_at_y(70.0, 10.0, 44.0, 3), Some(1));
        }

        #[test]
        fn rejects_y_below_every_row() {
            assert_eq!(track_index_at_y(300.0, 10.0, 44.0, 3), None);
        }
    }

    mod ruler_step_ms {
        use super::*;

        #[test]
        fn uses_finer_steps_when_zoomed_in() {
            assert!(ruler_step_ms(800.0) < ruler_step_ms(20.0));
        }

        #[test]
        fn supports_hour_scale_overview() {
            assert!(ms_to_px(3_600_000, MIN_PIXELS_PER_SECOND) <= 900.0);
        }
    }
}
