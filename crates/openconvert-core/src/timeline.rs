use serde::{Deserialize, Serialize};

/// Stable identifier for a timeline track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TrackId(pub u64);

/// Stable identifier for a clip on a timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClipId(pub u64);

/// The kind of media a clip carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ClipKind {
    /// Moving image decoded from the source over time.
    #[default]
    Video,
    /// Audio-only source.
    Audio,
    /// A still image shown for the clip's whole duration.
    Image,
}

/// How a clip's frame is scaled into the output canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FitMode {
    /// Scale to fit inside the canvas, preserving aspect (letterbox/pillarbox).
    #[default]
    Contain,
    /// Scale to fill the canvas, preserving aspect, cropping the overflow.
    Cover,
    /// Stretch to fill the canvas exactly, ignoring the source aspect ratio.
    Stretch,
}

/// A media segment placed on a [`Timeline`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    /// Stable clip identifier.
    pub id: ClipId,
    /// Source media path as stored in the project file.
    pub source_path: String,
    /// Start time on the output timeline, in milliseconds.
    pub timeline_start_ms: u64,
    /// Start offset inside the source media, in milliseconds.
    pub source_start_ms: u64,
    /// Clip duration, in milliseconds.
    pub duration_ms: u64,
    /// Total length of the source media, in milliseconds.
    ///
    /// Zero means the source length is unknown, so trims stay inside the clip's
    /// current source window. A positive value records the full media length so
    /// trims may extend anywhere inside the real source.
    #[serde(default)]
    pub source_duration_ms: u64,
    /// Whether the source media has an audio stream.
    ///
    /// Older project files did not store this, and the old exporter assumed
    /// audio was present, so the serde default preserves that behavior for
    /// existing projects.
    #[serde(default = "default_clip_has_audio")]
    pub has_audio: bool,
    /// Source video width in pixels, or zero when unknown/audio-only.
    #[serde(default)]
    pub source_width: u32,
    /// Source video height in pixels, or zero when unknown/audio-only.
    #[serde(default)]
    pub source_height: u32,
    /// Whether this clip's audio is excluded from playback and export.
    #[serde(default)]
    pub muted: bool,
    /// What kind of media this clip carries.
    #[serde(default)]
    pub kind: ClipKind,
    /// How the clip's frame is scaled into the output canvas.
    #[serde(default)]
    pub fit_mode: FitMode,
}

impl Clip {
    /// Returns the exclusive end time on the output timeline.
    pub fn timeline_end_ms(&self) -> u64 {
        self.timeline_start_ms.saturating_add(self.duration_ms)
    }

    /// Returns the inclusive lower and exclusive upper source offsets a trim
    /// may select. The window is fixed to the current source span when the
    /// source length is unknown, otherwise the whole source is selectable.
    pub fn source_bounds(&self) -> (u64, u64) {
        if matches!(self.kind, ClipKind::Image) {
            // A still image has no intrinsic length, so it can be stretched to
            // any duration on the timeline.
            (0, u64::MAX)
        } else if self.source_duration_ms == 0 {
            (
                self.source_start_ms,
                self.source_start_ms.saturating_add(self.duration_ms),
            )
        } else {
            (0, self.source_duration_ms)
        }
    }

    fn contains_split(&self, at_ms: u64) -> bool {
        self.timeline_start_ms < at_ms && at_ms < self.timeline_end_ms()
    }

    fn overlaps(&self, other: &Clip) -> bool {
        self.timeline_start_ms < other.timeline_end_ms()
            && other.timeline_start_ms < self.timeline_end_ms()
    }
}

fn default_clip_has_audio() -> bool {
    true
}

/// A single ordered lane of non-overlapping clips.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    /// Stable track identifier.
    pub id: TrackId,
    clips: Vec<Clip>,
}

impl Track {
    /// Returns the clips on this track in timeline order.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// Returns whether the track contains no clips.
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

/// Editable media timeline with undo/redo history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timeline {
    next_track_id: u64,
    next_clip_id: u64,
    tracks: Vec<Track>,
    #[serde(skip)]
    undo: Vec<Edit>,
    #[serde(skip)]
    redo: Vec<Edit>,
}

impl PartialEq for Timeline {
    fn eq(&self, other: &Self) -> bool {
        self.next_track_id == other.next_track_id
            && self.next_clip_id == other.next_clip_id
            && self.tracks == other.tracks
    }
}

impl Eq for Timeline {}

/// Reversible timeline mutation recorded by the undo/redo stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Edit {
    /// A track was added.
    AddTrack { track: Track },
    /// A clip was added to a track.
    AddClip { track_id: TrackId, clip: Clip },
    /// A clip was removed from a track.
    RemoveClip { track_id: TrackId, clip: Clip },
    /// A clip was replaced by another clip with the same identity.
    ReplaceClip {
        track_id: TrackId,
        before: Clip,
        after: Clip,
    },
    /// A clip was split into two clips.
    SplitClip {
        track_id: TrackId,
        original: Clip,
        left: Clip,
        right: Clip,
    },
    /// A clip's mute state changed.
    ToggleMute {
        track_id: TrackId,
        before: Clip,
        after: Clip,
    },
    /// A clip moved to a new start time and/or onto another track.
    MoveClip {
        from_track: TrackId,
        to_track: TrackId,
        before: Clip,
        after: Clip,
    },
    /// A track moved to a new position in the stack (z-order).
    MoveTrack { from_index: usize, to_index: usize },
}

/// Errors returned by timeline editing operations.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TimelineError {
    /// Requested track does not exist.
    #[error("track not found")]
    TrackNotFound,
    /// Requested clip does not exist.
    #[error("clip not found")]
    ClipNotFound,
    /// Clip duration must be non-zero.
    #[error("clip duration must be greater than zero")]
    EmptyClip,
    /// Clip would overlap another clip on the same track.
    #[error("clip overlaps another clip on the same track")]
    OverlappingClip,
    /// Split point was not strictly inside the clip.
    #[error("split point must be strictly inside the clip")]
    SplitOutOfBounds,
    /// Trim bounds would produce an invalid source range.
    #[error("trim bounds must define a non-empty range inside the source clip")]
    TrimOutOfBounds,
    /// Undo stack is empty.
    #[error("nothing to undo")]
    NothingToUndo,
    /// Redo stack is empty.
    #[error("nothing to redo")]
    NothingToRedo,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            next_track_id: 1,
            next_clip_id: 1,
            tracks: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl Timeline {
    /// Creates an empty timeline.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns all tracks in timeline order.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Track and clip ids that render video at `at_ms`, in bottom-to-top
    /// compositing order. Timeline row 0 is the visual top layer, so rows are
    /// traversed in reverse to composite lower rows first and upper rows last.
    pub fn video_layers_at(&self, at_ms: u64) -> Vec<(TrackId, ClipId)> {
        self.tracks
            .iter()
            .rev()
            .filter_map(|track| {
                track
                    .clips
                    .iter()
                    .find(|clip| {
                        clip.timeline_start_ms <= at_ms
                            && at_ms < clip.timeline_end_ms()
                            && !matches!(clip.kind, ClipKind::Audio)
                    })
                    .map(|clip| (track.id, clip.id))
            })
            .collect()
    }

    /// Returns the number of tracks on the timeline.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Returns whether every track on the timeline is empty.
    pub fn is_empty(&self) -> bool {
        self.tracks.iter().all(Track::is_empty)
    }

    /// Returns a track by identifier.
    pub fn track(&self, track_id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == track_id)
    }

    /// Returns the last clip on a track, if any.
    pub fn last_clip(&self, track_id: TrackId) -> Option<&Clip> {
        self.track(track_id)?.clips.last()
    }

    /// Returns a clip by track and clip identifier.
    pub fn clip(&self, track_id: TrackId, clip_id: ClipId) -> Option<&Clip> {
        self.track(track_id)?
            .clips
            .iter()
            .find(|clip| clip.id == clip_id)
    }

    /// Adds a new empty track and records the operation for undo.
    pub fn add_track(&mut self) -> TrackId {
        let track = Track {
            id: TrackId(self.next_track_id),
            clips: Vec::new(),
        };
        self.next_track_id += 1;
        let id = track.id;
        self.tracks.push(track.clone());
        self.record(Edit::AddTrack { track });
        id
    }

    /// Moves a track to a new position in the stack, changing its z-order in the
    /// composited output. Index 0 is the bottom layer; higher indices render on
    /// top. The destination is clamped to the valid range.
    pub fn move_track(&mut self, track_id: TrackId, to_index: usize) -> Result<(), TimelineError> {
        let from_index = self.track_index(track_id)?;
        let to_index = to_index.min(self.tracks.len().saturating_sub(1));
        if from_index == to_index {
            return Ok(());
        }
        self.reorder_track(from_index, to_index);
        self.record(Edit::MoveTrack {
            from_index,
            to_index,
        });
        Ok(())
    }

    fn reorder_track(&mut self, from_index: usize, to_index: usize) {
        let track = self.tracks.remove(from_index);
        self.tracks.insert(to_index, track);
    }

    /// Adds a non-overlapping clip to an existing track.
    pub fn add_clip(
        &mut self,
        track_id: TrackId,
        source_path: String,
        timeline_start_ms: u64,
        source_start_ms: u64,
        duration_ms: u64,
    ) -> Result<ClipId, TimelineError> {
        if duration_ms == 0 {
            return Err(TimelineError::EmptyClip);
        }

        let clip_id = ClipId(self.next_clip_id);
        let clip = Clip {
            id: clip_id,
            source_path,
            timeline_start_ms,
            source_start_ms,
            duration_ms,
            source_duration_ms: 0,
            has_audio: true,
            source_width: 0,
            source_height: 0,
            muted: false,
            kind: ClipKind::default(),
            fit_mode: FitMode::default(),
        };

        self.insert_clip(track_id, clip.clone())?;
        self.next_clip_id += 1;
        self.record(Edit::AddClip { track_id, clip });
        Ok(clip_id)
    }

    /// Deletes a clip from a track and records the operation for undo.
    pub fn delete_clip(&mut self, track_id: TrackId, clip_id: ClipId) -> Result<(), TimelineError> {
        let clip = self.remove_clip(track_id, clip_id)?;
        self.record(Edit::RemoveClip { track_id, clip });
        Ok(())
    }

    /// Splits a clip at an interior timeline position.
    pub fn split_clip(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        at_ms: u64,
    ) -> Result<(ClipId, ClipId), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        let original = self.tracks[track_index].clips[clip_index].clone();

        if !original.contains_split(at_ms) {
            return Err(TimelineError::SplitOutOfBounds);
        }

        let left_duration = at_ms - original.timeline_start_ms;
        let right_duration = original.duration_ms - left_duration;
        let left_id = ClipId(self.next_clip_id);
        let right_id = ClipId(self.next_clip_id + 1);

        let left = Clip {
            id: left_id,
            duration_ms: left_duration,
            ..original.clone()
        };
        let right = Clip {
            id: right_id,
            timeline_start_ms: at_ms,
            source_start_ms: original.source_start_ms + left_duration,
            duration_ms: right_duration,
            ..original.clone()
        };

        self.next_clip_id += 2;
        self.tracks[track_index].clips.remove(clip_index);
        self.tracks[track_index].clips.push(left.clone());
        self.tracks[track_index].clips.push(right.clone());
        self.tracks[track_index]
            .clips
            .sort_by_key(|clip| clip.timeline_start_ms);
        self.record(Edit::SplitClip {
            track_id,
            original,
            left,
            right,
        });
        Ok((left_id, right_id))
    }

    /// Trims a clip to a non-empty source range.
    pub fn trim_clip(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        new_timeline_start_ms: u64,
        new_source_start_ms: u64,
        new_duration_ms: u64,
    ) -> Result<(), TimelineError> {
        if new_duration_ms == 0 {
            return Err(TimelineError::TrimOutOfBounds);
        }

        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        let before = self.tracks[track_index].clips[clip_index].clone();
        let (source_floor, source_ceiling) = before.source_bounds();

        if new_source_start_ms < source_floor
            || new_source_start_ms + new_duration_ms > source_ceiling
        {
            return Err(TimelineError::TrimOutOfBounds);
        }

        let after = Clip {
            timeline_start_ms: new_timeline_start_ms,
            source_start_ms: new_source_start_ms,
            duration_ms: new_duration_ms,
            ..before.clone()
        };

        self.tracks[track_index].clips.remove(clip_index);
        if self.has_overlap(track_index, &after) {
            self.tracks[track_index].clips.insert(clip_index, before);
            return Err(TimelineError::OverlappingClip);
        }
        self.tracks[track_index].clips.push(after.clone());
        self.tracks[track_index]
            .clips
            .sort_by_key(|clip| clip.timeline_start_ms);
        self.record(Edit::ReplaceClip {
            track_id,
            before,
            after,
        });
        Ok(())
    }

    /// Toggles whether a clip contributes audio during playback and export.
    pub fn toggle_clip_mute(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
    ) -> Result<bool, TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        let before = self.tracks[track_index].clips[clip_index].clone();
        let after = Clip {
            muted: !before.muted,
            ..before.clone()
        };
        self.tracks[track_index].clips[clip_index] = after.clone();
        self.record(Edit::ToggleMute {
            track_id,
            before,
            after: after.clone(),
        });
        Ok(after.muted)
    }

    /// Moves a clip to a new start time, optionally onto another track.
    ///
    /// Both endpoints are resolved before any mutation so a missing destination
    /// never strands the clip, and an overlap on the destination restores the
    /// clip to its original place.
    pub fn move_clip(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        dest_track_id: TrackId,
        new_timeline_start_ms: u64,
    ) -> Result<(), TimelineError> {
        let source_index = self.track_index(track_id)?;
        let dest_index = self.track_index(dest_track_id)?;
        let clip_index = self.clip_index(source_index, clip_id)?;
        let before = self.tracks[source_index].clips[clip_index].clone();

        if track_id == dest_track_id && before.timeline_start_ms == new_timeline_start_ms {
            return Ok(());
        }

        let after = Clip {
            timeline_start_ms: new_timeline_start_ms,
            ..before.clone()
        };

        self.tracks[source_index].clips.remove(clip_index);
        if self.has_overlap(dest_index, &after) {
            self.tracks[source_index].clips.insert(clip_index, before);
            return Err(TimelineError::OverlappingClip);
        }

        self.tracks[dest_index].clips.push(after.clone());
        self.tracks[dest_index]
            .clips
            .sort_by_key(|clip| clip.timeline_start_ms);
        self.record(Edit::MoveClip {
            from_track: track_id,
            to_track: dest_track_id,
            before,
            after,
        });
        Ok(())
    }

    /// Records the full source media length for a clip so later trims can
    /// extend anywhere inside the real media. The value is clamped to at least
    /// the clip's current source window.
    pub fn set_clip_source_duration(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        source_duration_ms: u64,
    ) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        let clip = &mut self.tracks[track_index].clips[clip_index];
        clip.source_duration_ms =
            source_duration_ms.max(clip.source_start_ms.saturating_add(clip.duration_ms));
        Ok(())
    }

    /// Records whether the source media exposes an audio stream.
    pub fn set_clip_has_audio(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        has_audio: bool,
    ) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        self.tracks[track_index].clips[clip_index].has_audio = has_audio;
        Ok(())
    }

    /// Records the source video dimensions, or zeroes for unknown/audio-only media.
    pub fn set_clip_video_size(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        width: u32,
        height: u32,
    ) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        let clip = &mut self.tracks[track_index].clips[clip_index];
        clip.source_width = width;
        clip.source_height = height;
        Ok(())
    }

    /// Records what kind of media the clip carries.
    pub fn set_clip_kind(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        kind: ClipKind,
    ) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        self.tracks[track_index].clips[clip_index].kind = kind;
        Ok(())
    }

    /// Sets how the clip's frame is scaled into the output canvas.
    pub fn set_clip_fit_mode(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        fit_mode: FitMode,
    ) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        self.tracks[track_index].clips[clip_index].fit_mode = fit_mode;
        Ok(())
    }

    /// Reverts the most recent edit.
    pub fn undo(&mut self) -> Result<(), TimelineError> {
        let edit = self.undo.pop().ok_or(TimelineError::NothingToUndo)?;
        self.apply_inverse(&edit)?;
        self.redo.push(edit);
        Ok(())
    }

    /// Reapplies the most recently undone edit.
    pub fn redo(&mut self) -> Result<(), TimelineError> {
        let edit = self.redo.pop().ok_or(TimelineError::NothingToRedo)?;
        self.apply(&edit)?;
        self.undo.push(edit);
        Ok(())
    }

    /// Validates timeline invariants loaded from external data.
    pub fn validate(&self) -> Result<(), TimelineError> {
        for track in &self.tracks {
            for clip in &track.clips {
                if clip.duration_ms == 0 {
                    return Err(TimelineError::EmptyClip);
                }
            }
            for pair in track.clips.windows(2) {
                if pair[0].overlaps(&pair[1]) {
                    return Err(TimelineError::OverlappingClip);
                }
            }
        }
        Ok(())
    }

    fn insert_clip(&mut self, track_id: TrackId, clip: Clip) -> Result<(), TimelineError> {
        let track_index = self.track_index(track_id)?;
        if self.has_overlap(track_index, &clip) {
            return Err(TimelineError::OverlappingClip);
        }
        self.tracks[track_index].clips.push(clip);
        self.tracks[track_index]
            .clips
            .sort_by_key(|clip| clip.timeline_start_ms);
        Ok(())
    }

    fn remove_clip(&mut self, track_id: TrackId, clip_id: ClipId) -> Result<Clip, TimelineError> {
        let track_index = self.track_index(track_id)?;
        let clip_index = self.clip_index(track_index, clip_id)?;
        Ok(self.tracks[track_index].clips.remove(clip_index))
    }

    fn record(&mut self, edit: Edit) {
        self.undo.push(edit);
        self.redo.clear();
    }

    fn apply(&mut self, edit: &Edit) -> Result<(), TimelineError> {
        match edit {
            Edit::AddTrack { track } => {
                self.tracks.push(track.clone());
                self.reserve_track_id_after(track.id);
            }
            Edit::AddClip { track_id, clip } => {
                self.insert_clip(*track_id, clip.clone())?;
                self.reserve_clip_id_after(clip.id);
            }
            Edit::RemoveClip { track_id, clip } => {
                self.remove_clip(*track_id, clip.id)?;
            }
            Edit::ReplaceClip {
                track_id,
                before,
                after,
            } => {
                self.remove_clip(*track_id, before.id)?;
                self.insert_clip(*track_id, after.clone())?;
            }
            Edit::SplitClip {
                track_id,
                original,
                left,
                right,
            } => {
                self.remove_clip(*track_id, original.id)?;
                self.insert_clip(*track_id, left.clone())?;
                self.insert_clip(*track_id, right.clone())?;
                self.reserve_clip_id_after(right.id);
            }
            Edit::ToggleMute {
                track_id,
                before,
                after,
            } => {
                let track_index = self.track_index(*track_id)?;
                let clip_index = self.clip_index(track_index, before.id)?;
                self.tracks[track_index].clips[clip_index] = after.clone();
            }
            Edit::MoveClip {
                from_track,
                to_track,
                before,
                after,
            } => {
                self.remove_clip(*from_track, before.id)?;
                self.insert_clip(*to_track, after.clone())?;
            }
            Edit::MoveTrack {
                from_index,
                to_index,
            } => {
                self.reorder_track(*from_index, *to_index);
            }
        }
        Ok(())
    }

    fn apply_inverse(&mut self, edit: &Edit) -> Result<(), TimelineError> {
        match edit {
            Edit::AddTrack { track } => {
                let index = self.track_index(track.id)?;
                self.tracks.remove(index);
                self.release_last_track_id(track.id);
            }
            Edit::AddClip { track_id, clip } => {
                self.remove_clip(*track_id, clip.id)?;
                self.release_last_clip_id(clip.id);
            }
            Edit::RemoveClip { track_id, clip } => {
                self.insert_clip(*track_id, clip.clone())?;
            }
            Edit::ReplaceClip {
                track_id,
                before,
                after,
            } => {
                self.remove_clip(*track_id, after.id)?;
                self.insert_clip(*track_id, before.clone())?;
            }
            Edit::SplitClip {
                track_id,
                original,
                left,
                right,
            } => {
                self.remove_clip(*track_id, left.id)?;
                self.remove_clip(*track_id, right.id)?;
                self.insert_clip(*track_id, original.clone())?;
                self.release_last_clip_id(right.id);
                self.release_last_clip_id(left.id);
            }
            Edit::ToggleMute {
                track_id,
                before,
                after,
            } => {
                let track_index = self.track_index(*track_id)?;
                let clip_index = self.clip_index(track_index, after.id)?;
                self.tracks[track_index].clips[clip_index] = before.clone();
            }
            Edit::MoveClip {
                from_track,
                to_track,
                before,
                after,
            } => {
                self.remove_clip(*to_track, after.id)?;
                self.insert_clip(*from_track, before.clone())?;
            }
            Edit::MoveTrack {
                from_index,
                to_index,
            } => {
                self.reorder_track(*to_index, *from_index);
            }
        }
        Ok(())
    }

    fn track_index(&self, track_id: TrackId) -> Result<usize, TimelineError> {
        self.tracks
            .iter()
            .position(|track| track.id == track_id)
            .ok_or(TimelineError::TrackNotFound)
    }

    fn clip_index(&self, track_index: usize, clip_id: ClipId) -> Result<usize, TimelineError> {
        self.tracks[track_index]
            .clips
            .iter()
            .position(|clip| clip.id == clip_id)
            .ok_or(TimelineError::ClipNotFound)
    }

    fn has_overlap(&self, track_index: usize, candidate: &Clip) -> bool {
        self.tracks[track_index]
            .clips
            .iter()
            .any(|clip| clip.overlaps(candidate))
    }

    fn reserve_track_id_after(&mut self, track_id: TrackId) {
        self.next_track_id = self.next_track_id.max(track_id.0 + 1);
    }

    fn release_last_track_id(&mut self, track_id: TrackId) {
        if self.next_track_id == track_id.0 + 1 {
            self.next_track_id = track_id.0;
        }
    }

    fn reserve_clip_id_after(&mut self, clip_id: ClipId) {
        self.next_clip_id = self.next_clip_id.max(clip_id.0 + 1);
    }

    fn release_last_clip_id(&mut self, clip_id: ClipId) {
        if self.next_clip_id == clip_id.0 + 1 {
            self.next_clip_id = clip_id.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_clip_timeline() -> (Timeline, TrackId, ClipId) {
        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        let clip = timeline
            .add_clip(track, "clip.mp4".into(), 0, 100, 1_000)
            .unwrap();
        (timeline, track, clip)
    }

    #[test]
    fn split_rejects_clip_edges_and_preserves_original_until_valid_split() {
        let (mut timeline, track, clip) = one_clip_timeline();
        assert_eq!(
            timeline.split_clip(track, clip, 0),
            Err(TimelineError::SplitOutOfBounds)
        );
        assert_eq!(
            timeline.split_clip(track, clip, 1_000),
            Err(TimelineError::SplitOutOfBounds)
        );
        let (left, right) = timeline.split_clip(track, clip, 400).unwrap();
        let clips = timeline.tracks()[0].clips();
        assert_eq!((clips[0].id, clips[0].duration_ms), (left, 400));
        assert_eq!(
            (clips[1].id, clips[1].source_start_ms, clips[1].duration_ms),
            (right, 500, 600)
        );
        timeline.validate().unwrap();
    }

    #[test]
    fn adjacent_clips_are_allowed_but_overlapping_clips_are_rejected() {
        let (mut timeline, track, _) = one_clip_timeline();
        assert!(timeline
            .add_clip(track, "clip.mp4".into(), 1_000, 0, 500)
            .is_ok());
        assert_eq!(
            timeline.add_clip(track, "clip.mp4".into(), 999, 0, 500),
            Err(TimelineError::OverlappingClip)
        );
    }

    #[test]
    fn trim_rejects_empty_out_of_source_and_overlapping_ranges() {
        let (mut timeline, track, clip) = one_clip_timeline();
        assert_eq!(
            timeline.trim_clip(track, clip, 0, 100, 0),
            Err(TimelineError::TrimOutOfBounds)
        );
        assert_eq!(
            timeline.trim_clip(track, clip, 0, 50, 100),
            Err(TimelineError::TrimOutOfBounds)
        );
        timeline
            .add_clip(track, "clip.mp4".into(), 1_100, 0, 100)
            .unwrap();
        assert_eq!(
            timeline.trim_clip(track, clip, 1_050, 100, 100),
            Err(TimelineError::OverlappingClip)
        );
    }

    #[test]
    fn undo_redo_restores_exact_timeline_state() {
        let (mut timeline, track, clip) = one_clip_timeline();
        let before = timeline.clone();
        timeline.split_clip(track, clip, 500).unwrap();
        assert_ne!(timeline, before);
        timeline.undo().unwrap();
        assert_eq!(timeline, before);
        timeline.redo().unwrap();
        assert_eq!(timeline.tracks()[0].clips().len(), 2);
    }

    #[test]
    fn delete_clip_round_trips_with_undo_redo() {
        let (mut timeline, track, clip) = one_clip_timeline();
        timeline.delete_clip(track, clip).unwrap();
        assert!(timeline.tracks()[0].clips().is_empty());
        timeline.undo().unwrap();
        assert_eq!(timeline.tracks()[0].clips()[0].id, clip);
        timeline.redo().unwrap();
        assert!(timeline.tracks()[0].clips().is_empty());
    }

    #[test]
    fn toggle_mute_round_trips_with_undo_redo() {
        let (mut timeline, track, clip) = one_clip_timeline();
        assert!(timeline.toggle_clip_mute(track, clip).unwrap());
        assert!(timeline.tracks()[0].clips()[0].muted);
        timeline.undo().unwrap();
        assert!(!timeline.tracks()[0].clips()[0].muted);
        timeline.redo().unwrap();
        assert!(timeline.tracks()[0].clips()[0].muted);
    }

    #[test]
    fn move_clip_shifts_start_and_round_trips_with_undo_redo() {
        let (mut timeline, track, clip) = one_clip_timeline();
        timeline.move_clip(track, clip, track, 5_000).unwrap();
        assert_eq!(timeline.tracks()[0].clips()[0].timeline_start_ms, 5_000);
        timeline.undo().unwrap();
        assert_eq!(timeline.tracks()[0].clips()[0].timeline_start_ms, 0);
        timeline.redo().unwrap();
        assert_eq!(timeline.tracks()[0].clips()[0].timeline_start_ms, 5_000);
    }

    #[test]
    fn move_clip_relocates_clip_onto_another_track() {
        let (mut timeline, track, clip) = one_clip_timeline();
        let other = timeline.add_track();
        timeline.move_clip(track, clip, other, 2_000).unwrap();
        assert!(timeline.track(track).unwrap().is_empty());
        assert_eq!(timeline.track(other).unwrap().clips()[0].id, clip);
    }

    #[test]
    fn move_clip_rejects_overlap_and_preserves_source_clip() {
        let (mut timeline, track, clip) = one_clip_timeline();
        timeline
            .add_clip(track, "b.mp4".into(), 1_000, 0, 1_000)
            .unwrap();
        assert_eq!(
            timeline.move_clip(track, clip, track, 1_500),
            Err(TimelineError::OverlappingClip)
        );
        assert_eq!(timeline.tracks()[0].clips()[0].timeline_start_ms, 0);
    }

    #[test]
    fn trim_extends_inside_known_source_duration() {
        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        let clip = timeline
            .add_clip(track, "c.mp4".into(), 0, 1_000, 1_000)
            .unwrap();
        timeline
            .set_clip_source_duration(track, clip, 5_000)
            .unwrap();
        timeline.trim_clip(track, clip, 0, 500, 3_000).unwrap();
        let clip = &timeline.tracks()[0].clips()[0];
        assert_eq!((clip.source_start_ms, clip.duration_ms), (500, 3_000));
    }

    #[test]
    fn move_track_changes_stacking_order() {
        let mut timeline = Timeline::new();
        let bottom = timeline.add_track();
        let top = timeline.add_track();

        timeline.move_track(top, 0).unwrap();

        assert_eq!(timeline.tracks()[0].id, top);
        let _ = bottom;
    }

    #[test]
    fn move_track_round_trips_with_undo() {
        let mut timeline = Timeline::new();
        let first = timeline.add_track();
        let _second = timeline.add_track();
        let _third = timeline.add_track();

        timeline.move_track(first, 2).unwrap();
        timeline.undo().unwrap();

        assert_eq!(timeline.tracks()[0].id, first);
    }

    #[test]
    fn video_layers_at_lists_overlapping_clips_in_visual_bottom_to_top_order() {
        let mut timeline = Timeline::new();
        let top = timeline.add_track();
        let bottom = timeline.add_track();
        let top_clip = timeline.add_clip(top, "a.mp4".into(), 0, 0, 1_000).unwrap();
        let bottom_clip = timeline
            .add_clip(bottom, "b.mp4".into(), 0, 0, 1_000)
            .unwrap();

        let layers = timeline.video_layers_at(500);

        assert_eq!(layers, vec![(bottom, bottom_clip), (top, top_clip)]);
    }

    #[test]
    fn video_layers_at_excludes_audio_only_clips() {
        let mut timeline = Timeline::new();
        let track = timeline.add_track();
        let clip = timeline
            .add_clip(track, "a.mp3".into(), 0, 0, 1_000)
            .unwrap();
        timeline
            .set_clip_kind(track, clip, ClipKind::Audio)
            .unwrap();

        let layers = timeline.video_layers_at(500);

        assert!(layers.is_empty());
    }
}
