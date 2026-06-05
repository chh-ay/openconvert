use std::path::{Path, PathBuf};

use anyhow::Result;
use openconvert_core::{Clip, ClipId, Project, TrackId};
use openconvert_media::{decode_frame_at, probe_media, DecodedFrame, ExportOptions};

use crate::preview_player::{PREVIEW_MAX_HEIGHT, PREVIEW_MAX_WIDTH};
use crate::timeline_geo::{clamp_pixels_per_second, ms_to_px, DEFAULT_PIXELS_PER_SECOND};

#[derive(Debug, Clone)]
pub struct EditorState {
    pub project: Project,
    pub selected_track: Option<TrackId>,
    pub selected_clip: Option<(TrackId, ClipId)>,
    pub playhead_ms: u64,
    pub export_options: ExportOptions,
    pub convert_input: Option<PathBuf>,
    pub convert_duration_ms: Option<u64>,
    pub convert_has_audio: bool,
    pub convert_width: u32,
    pub convert_height: u32,
    pub convert_is_image: bool,
    pub convert_preview_ms: u64,
    pub pixels_per_second: f32,
    pub timeline_scroll_x: f32,
    pub pending_fit: bool,
}

impl EditorState {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            selected_track: None,
            selected_clip: None,
            playhead_ms: 0,
            export_options: ExportOptions::default(),
            convert_input: None,
            convert_duration_ms: None,
            convert_has_audio: false,
            convert_width: 0,
            convert_height: 0,
            convert_is_image: false,
            convert_preview_ms: 0,
            pixels_per_second: DEFAULT_PIXELS_PER_SECOND,
            timeline_scroll_x: 0.0,
            pending_fit: true,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(Project::new("Untitled project"));
    }

    pub fn select_track(&mut self, track_id: TrackId) {
        self.selected_track = Some(track_id);
        self.selected_clip = None;
    }

    pub fn select_clip(&mut self, track_id: TrackId, clip_id: ClipId) {
        self.selected_track = Some(track_id);
        self.selected_clip = Some((track_id, clip_id));

        if let Some(clip) = self.find_clip(track_id, clip_id) {
            self.playhead_ms = clip.timeline_start_ms;
        }
    }

    pub fn selected_clip(&self) -> Option<(TrackId, ClipId)> {
        let selected = self.selected_clip?;
        self.find_clip(selected.0, selected.1)?;
        Some(selected)
    }

    pub fn selected_track_or_create(&mut self) -> TrackId {
        if let Some(track_id) = self.selected_track {
            if self
                .project
                .timeline
                .tracks()
                .iter()
                .any(|track| track.id == track_id)
            {
                return track_id;
            }
        }

        if let Some(track) = self.project.timeline.tracks().first() {
            self.selected_track = Some(track.id);
            return track.id;
        }

        let track_id = self.project.timeline.add_track();
        self.selected_track = Some(track_id);
        track_id
    }

    pub fn select_next_clip(&mut self) {
        let clips = self.clip_handles();

        if clips.is_empty() {
            self.selected_clip = None;
            return;
        }

        let index = self
            .selected_clip
            .and_then(|selected| clips.iter().position(|clip| *clip == selected))
            .map(|index| (index + 1) % clips.len())
            .unwrap_or(0);
        let selected = clips[index];
        self.select_clip(selected.0, selected.1);
    }

    pub fn select_previous_clip(&mut self) {
        let clips = self.clip_handles();

        if clips.is_empty() {
            self.selected_clip = None;
            return;
        }

        let index = self
            .selected_clip
            .and_then(|selected| clips.iter().position(|clip| *clip == selected))
            .map(|index| {
                if index == 0 {
                    clips.len() - 1
                } else {
                    index - 1
                }
            })
            .unwrap_or(0);
        let selected = clips[index];
        self.select_clip(selected.0, selected.1);
    }

    fn clip_handles(&self) -> Vec<(TrackId, ClipId)> {
        self.project
            .timeline
            .tracks()
            .iter()
            .flat_map(|track| track.clips().iter().map(move |clip| (track.id, clip.id)))
            .collect()
    }

    pub fn timeline_duration_ms(&self) -> u64 {
        self.project
            .timeline
            .tracks()
            .iter()
            .flat_map(|track| track.clips().iter())
            .map(|clip| clip.timeline_end_ms())
            .max()
            .unwrap_or(0)
    }

    /// Multiplies the timeline zoom by `factor`, keeping `anchor_ms` fixed at
    /// `anchor_offset_px` from the left of the lane area.
    pub fn zoom(&mut self, factor: f32, anchor_ms: u64, anchor_offset_px: f32) {
        let new_pps = clamp_pixels_per_second(self.pixels_per_second * factor);
        self.timeline_scroll_x = (ms_to_px(anchor_ms, new_pps) - anchor_offset_px).max(0.0);
        self.pixels_per_second = new_pps;
    }

    /// Requests that the next timeline layout fit the whole project in view.
    pub fn request_fit(&mut self) {
        self.pending_fit = true;
    }

    /// Sets zoom and scroll so the whole project fits `lane_width_px` of lanes.
    pub fn fit_view(&mut self, lane_width_px: f32) {
        self.pending_fit = false;
        self.timeline_scroll_x = 0.0;
        let duration = self.timeline_duration_ms();
        if duration == 0 || lane_width_px <= 0.0 {
            self.pixels_per_second = DEFAULT_PIXELS_PER_SECOND;
            return;
        }
        let seconds = duration as f32 / 1_000.0;
        // Small margin keeps the final clip edge off the right border.
        self.pixels_per_second = clamp_pixels_per_second(lane_width_px * 0.96 / seconds);
    }

    pub fn nudge_playhead(&mut self, delta_ms: i64) {
        let duration = self.timeline_duration_ms();
        let next = if delta_ms.is_negative() {
            self.playhead_ms.saturating_sub(delta_ms.unsigned_abs())
        } else {
            self.playhead_ms.saturating_add(delta_ms as u64)
        };

        self.playhead_ms = next.min(duration);

        if let Some((track_id, clip_id)) = self.clip_at_time(self.playhead_ms) {
            self.selected_track = Some(track_id);
            self.selected_clip = Some((track_id, clip_id));
        }
    }

    /// Returns the clip at `at_ms` on the topmost track that has one. Tracks
    /// later in the list render above earlier ones, so the topmost clip is the
    /// one visible in the composite and the natural target for preview/editing.
    pub fn clip_at_time(&self, at_ms: u64) -> Option<(TrackId, ClipId)> {
        self.project
            .timeline
            .tracks()
            .iter()
            .rev()
            .find_map(|track| {
                track
                    .clips()
                    .iter()
                    .find(|clip| clip.timeline_start_ms <= at_ms && at_ms < clip.timeline_end_ms())
                    .map(|clip| (track.id, clip.id))
            })
    }

    pub fn find_clip(&self, track_id: TrackId, clip_id: ClipId) -> Option<&Clip> {
        self.project
            .timeline
            .tracks()
            .iter()
            .find(|track| track.id == track_id)?
            .clips()
            .iter()
            .find(|clip| clip.id == clip_id)
    }
}

#[derive(Debug, Clone)]
pub struct ImportedMedia {
    pub duration_ms: u64,
    pub preview_frame: Option<DecodedFrame>,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
    pub is_image: bool,
}

/// Default timeline length given to a still image when it is imported, since an
/// image has no intrinsic duration. It can be trimmed/stretched afterwards.
pub const DEFAULT_IMAGE_DURATION_MS: u64 = 5_000;

pub fn probe_media_for_import(path: &Path) -> Result<ImportedMedia, String> {
    let result = probe_media(path).map_err(|error| error.to_string())?;
    let is_image = result.is_image;
    let duration_ms = if is_image {
        DEFAULT_IMAGE_DURATION_MS
    } else {
        result
            .duration_ms
            .filter(|duration| *duration > 0)
            .ok_or_else(|| "media has no positive duration".to_owned())?
    };
    let preview_at_ms = if is_image { 0 } else { duration_ms / 2 };
    // Decode the poster frame in-process; audio-only sources have no video to
    // show, and a decode failure should not abort an otherwise valid import.
    let preview_frame = if result.video_codec.is_some() {
        decode_frame_at(path, preview_at_ms, PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT).ok()
    } else {
        None
    };

    Ok(ImportedMedia {
        duration_ms,
        has_audio: result.audio_codec.is_some(),
        width: result.width.unwrap_or(0),
        height: result.height.unwrap_or(0),
        is_image,
        preview_frame,
    })
}

pub fn preview_request_at_playhead(editor: &EditorState) -> Option<(PathBuf, u64)> {
    let (track_id, clip_id) = editor.clip_at_time(editor.playhead_ms)?;
    let clip = editor.find_clip(track_id, clip_id)?;
    let offset = editor.playhead_ms.saturating_sub(clip.timeline_start_ms);

    Some((
        PathBuf::from(&clip.source_path),
        clip.source_start_ms + offset,
    ))
}
