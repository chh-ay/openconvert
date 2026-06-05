use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use eframe::egui::{self, RichText};
use openconvert_core::{
    ClipId, ClipKind, FitMode, FrameCache, Project, ResourcePolicy, Timeline, TrackId,
};
use openconvert_media::{
    composite_layers, decode_frame_at, render_timeline, CompositeLayer, DecodedFrame, ScrubDecoder,
};

use crate::editor::{preview_request_at_playhead, probe_media_for_import, EditorState};
use crate::messages::{AppMessage, Panel, PreviewKind, PreviewRequest};
use crate::native_dialog;
use crate::player::{AudioSource, PlaybackState, PlaybackTarget};
use crate::preview::{playback_bucket_ms, preview_key, PreviewKey};
use crate::preview_player::{PreviewPlayer, PREVIEW_MAX_HEIGHT, PREVIEW_MAX_WIDTH};
use crate::theme::{self, PALETTE_MUTED};
use crate::timeline_geo::{ClipPlacement, TimelineDrag};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackStreamKey {
    target: PlaybackTarget,
    source_path: PathBuf,
    track_id: Option<TrackId>,
    clip_id: Option<ClipId>,
}

/// One video layer being presented: its source key (used to diff the active set
/// across ticks), how it fits the canvas, the decoder driving it (absent for a
/// still image), and the most recent frame to composite.
struct PreviewLayer {
    key: PlaybackStreamKey,
    fit: FitMode,
    player: Option<PreviewPlayer>,
    frame: Option<DecodedFrame>,
}

pub struct OpenConvertApp {
    pub(crate) editor: EditorState,
    pub(crate) sender: Sender<AppMessage>,
    pub(crate) receiver: Receiver<AppMessage>,
    pub(crate) egui_ctx: egui::Context,
    pub(crate) status: String,
    pub(crate) panel: Panel,
    pub(crate) preview: Option<egui::TextureHandle>,
    pub(crate) preview_in_flight: bool,
    pub(crate) pending_preview: Option<PreviewRequest>,
    pub(crate) last_preview_key: Option<PreviewKey>,
    pub(crate) frame_cache: FrameCache<PreviewKey, egui::TextureHandle>,
    pub(crate) timeline_drag: Option<TimelineDrag>,
    pub(crate) thumb_cache: FrameCache<PreviewKey, egui::TextureHandle>,
    pub(crate) thumb_in_flight: HashSet<PreviewKey>,
    pub(crate) thumb_failed: HashSet<PreviewKey>,
    pub(crate) playback: PlaybackState,
    preview_layers: Vec<PreviewLayer>,
    needs_composite: bool,
    scrub: ScrubWorker,
}

impl OpenConvertApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::configure_style(&cc.egui_ctx);
        let (sender, receiver) = mpsc::channel();
        let status = "In-process media engine ready".to_owned();

        let scrub = ScrubWorker::spawn(sender.clone(), cc.egui_ctx.clone());

        Self {
            editor: EditorState::new(Project::new("Untitled project")),
            sender,
            receiver,
            egui_ctx: cc.egui_ctx.clone(),
            status,
            panel: Panel::Edit,
            preview: None,
            preview_in_flight: false,
            pending_preview: None,
            last_preview_key: None,
            frame_cache: FrameCache::new(ResourcePolicy::default().max_cached_frames),
            timeline_drag: None,
            thumb_cache: FrameCache::new(256),
            thumb_in_flight: HashSet::new(),
            thumb_failed: HashSet::new(),
            playback: PlaybackState::new(),
            preview_layers: Vec::new(),
            needs_composite: false,
            scrub,
        }
    }

    fn process_messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                AppMessage::Status(status) => self.status = status,
                AppMessage::Error(error) => self.status = error,
                AppMessage::ProjectOpened { path, project } => {
                    self.editor = EditorState::new(project);
                    self.playback.stop();
                    self.preview = None;
                    self.stop_preview_player();
                    self.editor.select_next_clip();
                    self.editor.request_fit();
                    self.request_preview_at_playhead(true);
                    self.status = format!("Opened project {}", path.display());
                }
                AppMessage::MediaImported {
                    path,
                    duration_ms,
                    has_audio,
                    width,
                    height,
                    is_image,
                    preview_frame,
                } => {
                    let track_id = self.editor.selected_track_or_create();
                    let timeline_start_ms = self
                        .editor
                        .project
                        .timeline
                        .tracks()
                        .iter()
                        .find(|track| track.id == track_id)
                        .and_then(|track| track.clips().last())
                        .map(|clip| clip.timeline_end_ms())
                        .unwrap_or(0);

                    match self.editor.project.timeline.add_clip(
                        track_id,
                        path.display().to_string(),
                        timeline_start_ms,
                        0,
                        duration_ms,
                    ) {
                        Ok(clip_id) => {
                            let _ = self.editor.project.timeline.set_clip_source_duration(
                                track_id,
                                clip_id,
                                duration_ms,
                            );
                            let _ = self
                                .editor
                                .project
                                .timeline
                                .set_clip_has_audio(track_id, clip_id, has_audio);
                            let _ = self
                                .editor
                                .project
                                .timeline
                                .set_clip_video_size(track_id, clip_id, width, height);
                            if is_image {
                                let _ = self.editor.project.timeline.set_clip_kind(
                                    track_id,
                                    clip_id,
                                    openconvert_core::ClipKind::Image,
                                );
                            }
                            self.editor.select_clip(track_id, clip_id);
                            self.editor.request_fit();
                            self.status = format!("Imported {}", path.display());

                            if let Some(frame) = preview_frame {
                                self.set_preview_from_frame(ctx, &frame);
                            }
                        }
                        Err(error) => self.status = format!("Import failed: {error}"),
                    }
                }
                AppMessage::PreviewReady { key, frame } => {
                    self.preview_in_flight = false;
                    self.cache_preview(ctx, key, &frame);
                    self.start_pending_preview();
                }
                AppMessage::PreviewFailed { error } => {
                    self.preview_in_flight = false;
                    self.status = error;
                    self.start_pending_preview();
                }
                AppMessage::ThumbReady { key, frame } => {
                    self.thumb_in_flight.remove(&key);
                    self.store_thumbnail(ctx, key, &frame);
                }
                AppMessage::ThumbFailed { key } => {
                    self.thumb_in_flight.remove(&key);
                    self.thumb_failed.insert(key);
                }
                AppMessage::ExportFinished { path, options } => {
                    self.status = format!("Exported {} as {}", path.display(), options.summary());
                }
                AppMessage::ConvertInputSelected {
                    path,
                    duration_ms,
                    preview_frame,
                    has_audio,
                    width,
                    height,
                    is_image,
                } => {
                    self.editor.convert_input = Some(path.clone());
                    self.editor.convert_duration_ms = Some(duration_ms);
                    self.editor.convert_preview_ms = duration_ms / 2;
                    self.editor.convert_has_audio = has_audio;
                    self.editor.convert_width = width;
                    self.editor.convert_height = height;
                    self.editor.convert_is_image = is_image;
                    self.last_preview_key = None;
                    self.status = format!("Loaded conversion input {}", path.display());

                    if let Some(frame) = preview_frame {
                        self.set_preview_from_frame(ctx, &frame);
                    }
                }
                AppMessage::ConvertFinished {
                    input,
                    output,
                    options,
                } => {
                    self.status = format!(
                        "Converted {} to {} as {}",
                        input.display(),
                        output.display(),
                        options.summary()
                    );
                }
            }
        }
    }

    fn set_preview_from_frame(&mut self, ctx: &egui::Context, frame: &DecodedFrame) {
        self.preview = Some(frame_to_texture(ctx, "openconvert-preview", frame));
    }

    fn cache_preview(&mut self, ctx: &egui::Context, key: PreviewKey, frame: &DecodedFrame) {
        let texture = frame_to_texture(ctx, "openconvert-preview", frame);
        self.frame_cache.insert(key, texture.clone());
        self.preview = Some(texture);
    }

    /// Requests a timeline thumbnail unless it is cached, already extracting, or
    /// previously failed. At most a few extractions run at once so the filmstrip
    /// fills progressively without flooding FFmpeg or stalling the UI.
    pub(crate) fn request_thumbnail(&mut self, key: PreviewKey) {
        const MAX_THUMB_IN_FLIGHT: usize = 3;
        if self.thumb_cache.get(&key).is_some()
            || self.thumb_in_flight.contains(&key)
            || self.thumb_failed.contains(&key)
            || self.thumb_in_flight.len() >= MAX_THUMB_IN_FLIGHT
        {
            return;
        }
        self.thumb_in_flight.insert(key.clone());
        let sender = self.sender.clone();
        let ctx = self.egui_ctx.clone();
        thread::spawn(move || {
            let bucket = key.1;
            // Decode in-process: one libav open, no FFmpeg process and no disk
            // round-trip. The 160px box keeps each cached thumbnail small.
            let message = match decode_frame_at(&key.0, bucket, 160, 160) {
                Ok(frame) => AppMessage::ThumbReady { key, frame },
                Err(_) => AppMessage::ThumbFailed { key },
            };
            let _ = sender.send(message);
            ctx.request_repaint();
        });
    }

    fn store_thumbnail(&mut self, ctx: &egui::Context, key: PreviewKey, frame: &DecodedFrame) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &frame.rgba,
        );
        let texture = ctx.load_texture("openconvert-thumb", image, egui::TextureOptions::LINEAR);
        self.thumb_cache.insert(key, texture);
    }

    // ------------------------------------------------------------------ project ops
    pub(crate) fn new_project(&mut self) {
        self.editor.reset();
        self.preview = None;
        self.playback.stop();
        self.status = "Started a new project".to_owned();
    }

    pub(crate) fn open_project(&self) {
        let sender = self.sender.clone();

        thread::spawn(move || {
            let Some(path) = native_dialog::open_project() else {
                let _ = sender.send(AppMessage::Status("Open cancelled".to_owned()));
                return;
            };

            match Project::load(&path) {
                Ok(project) => {
                    let _ = sender.send(AppMessage::ProjectOpened { path, project });
                }
                Err(error) => {
                    let _ = sender.send(AppMessage::Error(format!("Project open failed: {error}")));
                }
            }
        });
    }

    pub(crate) fn save_project(&self) {
        let sender = self.sender.clone();
        let snapshot = self.editor.project.clone();

        thread::spawn(move || {
            let Some(path) = native_dialog::save_project() else {
                let _ = sender.send(AppMessage::Status("Save cancelled".to_owned()));
                return;
            };

            match snapshot.save(&path) {
                Ok(()) => {
                    let _ = sender.send(AppMessage::Status(format!("Saved {}", path.display())));
                }
                Err(error) => {
                    let _ = sender.send(AppMessage::Error(format!("Project save failed: {error}")));
                }
            }
        });
    }

    pub(crate) fn import_media(&self) {
        let sender = self.sender.clone();

        thread::spawn(move || {
            let Some(path) = native_dialog::open_media() else {
                let _ = sender.send(AppMessage::Status("Import cancelled".to_owned()));
                return;
            };

            match probe_media_for_import(&path) {
                Ok(media) => {
                    let _ = sender.send(AppMessage::MediaImported {
                        path,
                        duration_ms: media.duration_ms,
                        has_audio: media.has_audio,
                        width: media.width,
                        height: media.height,
                        is_image: media.is_image,
                        preview_frame: media.preview_frame,
                    });
                }
                Err(error) => {
                    let _ = sender.send(AppMessage::Error(format!("Import failed: {error}")));
                }
            }
        });
    }

    pub(crate) fn select_convert_input(&self) {
        let sender = self.sender.clone();

        thread::spawn(move || {
            let Some(path) = native_dialog::open_media() else {
                let _ = sender.send(AppMessage::Status(
                    "Convert input selection cancelled".to_owned(),
                ));
                return;
            };

            match probe_media_for_import(&path) {
                Ok(media) => {
                    let _ = sender.send(AppMessage::ConvertInputSelected {
                        path,
                        duration_ms: media.duration_ms,
                        has_audio: media.has_audio,
                        width: media.width,
                        height: media.height,
                        is_image: media.is_image,
                        preview_frame: media.preview_frame,
                    });
                }
                Err(error) => {
                    let _ =
                        sender.send(AppMessage::Error(format!("Convert input failed: {error}")));
                }
            }
        });
    }

    pub(crate) fn export_media(&self) {
        let sender = self.sender.clone();
        let timeline = self.editor.project.timeline.clone();
        let options = self.editor.export_options;

        thread::spawn(move || {
            let Some(mut path) = native_dialog::save_export() else {
                let _ = sender.send(AppMessage::Status("Export cancelled".to_owned()));
                return;
            };

            path.set_extension(options.container.extension());
            let result =
                render_timeline(&timeline, &path, options).map_err(|error| error.to_string());

            match result {
                Ok(()) => {
                    let _ = sender.send(AppMessage::ExportFinished { path, options });
                }
                Err(error) => {
                    let _ = sender.send(AppMessage::Error(format!("Export failed: {error}")));
                }
            }
        });
    }

    pub(crate) fn convert_input_file(&self) {
        let Some(input) = self.editor.convert_input.clone() else {
            let _ = self
                .sender
                .send(AppMessage::Error("Pick an input file first".to_owned()));
            return;
        };
        let duration_ms = self.editor.convert_duration_ms.unwrap_or(1);
        let has_audio = self.editor.convert_has_audio;
        let width = self.editor.convert_width;
        let height = self.editor.convert_height;
        let is_image = self.editor.convert_is_image;
        let options = self.editor.export_options;
        let sender = self.sender.clone();

        thread::spawn(move || {
            let Some(mut output) = native_dialog::save_export() else {
                let _ = sender.send(AppMessage::Status("Convert cancelled".to_owned()));
                return;
            };

            output.set_extension(options.container.extension());
            let result = (|| {
                let mut timeline = Timeline::new();
                let track = timeline.add_track();
                let clip_id = timeline
                    .add_clip(track, input.display().to_string(), 0, 0, duration_ms)
                    .map_err(|error| error.to_string())?;
                let _ = timeline.set_clip_has_audio(track, clip_id, has_audio);
                let _ = timeline.set_clip_video_size(track, clip_id, width, height);
                if is_image {
                    let _ =
                        timeline.set_clip_kind(track, clip_id, openconvert_core::ClipKind::Image);
                }
                render_timeline(&timeline, &output, options).map_err(|error| error.to_string())
            })();
            match result {
                Ok(()) => {
                    let _ = sender.send(AppMessage::ConvertFinished {
                        input,
                        output,
                        options,
                    });
                }
                Err(error) => {
                    let _ = sender.send(AppMessage::Error(format!("Convert failed: {error}")));
                }
            }
        });
    }

    // ------------------------------------------------------------------ preview
    pub(crate) fn request_preview_at_playhead(&mut self, force: bool) {
        let Some((media_path, at_ms)) = preview_request_at_playhead(&self.editor) else {
            return;
        };
        self.request_preview(PreviewKind::Timeline, media_path, at_ms, force);
    }

    pub(crate) fn show_cached_playback_preview_at_playhead(&mut self) {
        let Some((media_path, at_ms)) = preview_request_at_playhead(&self.editor) else {
            return;
        };
        self.show_cached_playback_preview(media_path, at_ms);
    }

    pub(crate) fn request_convert_preview(&mut self, force: bool) {
        let Some(media_path) = self.editor.convert_input.clone() else {
            return;
        };
        let at_ms = self.editor.convert_preview_ms;
        self.request_preview(PreviewKind::Convert, media_path, at_ms, force);
    }

    pub(crate) fn show_cached_playback_convert_preview(&mut self) {
        let Some(media_path) = self.editor.convert_input.clone() else {
            return;
        };
        self.show_cached_playback_preview(media_path, self.editor.convert_preview_ms);
    }

    fn show_cached_playback_preview(&mut self, media_path: PathBuf, at_ms: u64) {
        let key = preview_key(&media_path, playback_bucket_ms(at_ms));
        if let Some(texture) = self.frame_cache.get(&key).cloned() {
            self.preview = Some(texture);
            self.last_preview_key = Some(key);
        }
    }

    /// Switching panels must drop the other mode's preview frame and any
    /// in-flight playback; otherwise a stale timeline frame lingers in the
    /// Convert preview (and vice versa). Then load the new panel's frame.
    pub(crate) fn on_panel_changed(&mut self) {
        self.playback.stop();
        self.stop_preview_player();
        self.preview = None;
        self.last_preview_key = None;
        self.pending_preview = None;
        match self.panel {
            Panel::Edit => self.request_preview_at_playhead(true),
            Panel::Convert => self.request_convert_preview(true),
        }
    }

    // ---------------------------------------------------------- playback preview
    /// The video layers to present for `target`, bottom-to-top. For the timeline
    /// these are every video-rendering clip under the playhead (so overlapping
    /// tracks composite); for convert it is the single input. Each entry carries
    /// its source key, fit mode, whether it is a still image, and the source
    /// position to decode.
    fn playback_layers(
        &self,
        target: PlaybackTarget,
    ) -> Vec<(PlaybackStreamKey, FitMode, bool, u64)> {
        match target {
            PlaybackTarget::Timeline => self
                .editor
                .project
                .timeline
                .video_layers_at(self.editor.playhead_ms)
                .into_iter()
                .filter_map(|(track_id, clip_id)| {
                    let clip = self.editor.find_clip(track_id, clip_id)?;
                    let media_position_ms = clip.source_start_ms
                        + self
                            .editor
                            .playhead_ms
                            .saturating_sub(clip.timeline_start_ms);
                    Some((
                        PlaybackStreamKey {
                            target,
                            source_path: PathBuf::from(&clip.source_path),
                            track_id: Some(track_id),
                            clip_id: Some(clip_id),
                        },
                        clip.fit_mode,
                        matches!(clip.kind, ClipKind::Image),
                        media_position_ms,
                    ))
                })
                .collect(),
            PlaybackTarget::Convert => match self.editor.convert_input.clone() {
                Some(source_path) => vec![(
                    PlaybackStreamKey {
                        target,
                        source_path,
                        track_id: None,
                        clip_id: None,
                    },
                    FitMode::Stretch,
                    self.editor.convert_is_image,
                    self.editor.convert_preview_ms,
                )],
                None => Vec::new(),
            },
        }
    }

    /// Reseats preview decoders and audio to the clips under the playhead. Called
    /// every tick during playback: while the active set of clips is unchanged the
    /// existing decoders keep streaming; crossing a clip boundary (or a layout
    /// change) rebuilds the layer stack. `force` rebuilds even when unchanged,
    /// which seeks use to jump within a clip.
    fn reseat_playback_sources(&mut self, force: bool) {
        let target = self.playback.target();
        let specs = self.playback_layers(target);
        if specs.is_empty() {
            self.stop_preview_player();
            self.playback.stop_audio();
            return;
        }

        let unchanged = self.preview_layers.len() == specs.len()
            && self
                .preview_layers
                .iter()
                .zip(&specs)
                .all(|(layer, spec)| layer.key == spec.0);
        if !force && unchanged {
            return;
        }

        // Show a cached still immediately while the decoders warm up.
        match target {
            PlaybackTarget::Timeline => self.show_cached_playback_preview_at_playhead(),
            PlaybackTarget::Convert => self.show_cached_playback_convert_preview(),
        }

        self.stop_preview_player();
        self.preview_layers = specs
            .into_iter()
            .map(|(key, fit, is_image, media_position_ms)| {
                if is_image {
                    // A still is decoded once; it has no clock to follow.
                    let frame = decode_frame_at(
                        &key.source_path,
                        media_position_ms,
                        PREVIEW_MAX_WIDTH,
                        PREVIEW_MAX_HEIGHT,
                    )
                    .ok();
                    PreviewLayer {
                        key,
                        fit,
                        player: None,
                        frame,
                    }
                } else {
                    let player = PreviewPlayer::start(
                        key.source_path.clone(),
                        media_position_ms,
                        self.egui_ctx.clone(),
                    );
                    PreviewLayer {
                        key,
                        fit,
                        player: Some(player),
                        frame: None,
                    }
                }
            })
            .collect();
        self.needs_composite = true;

        let audio_source = self.audio_source_for_target(target);
        let _ = self.playback.restart_audio(audio_source);
    }

    fn stop_preview_player(&mut self) {
        // Dropping each player signals its worker to stop and joins it, so no
        // orphaned decoder thread is left running.
        self.preview_layers.clear();
        self.needs_composite = false;
    }

    /// Advances each layer's decoder to the playback clock and, when a new frame
    /// arrived (or the layer set just changed), composites the layers
    /// bottom-to-top into the preview texture. Uploads only when content changed.
    fn present_preview_frame(&mut self) {
        let target = self.playback.target();
        let positions: Vec<Option<u64>> = self
            .preview_layers
            .iter()
            .map(|layer| self.layer_media_position(&layer.key))
            .collect();

        let mut updated = false;
        for (layer, position) in self.preview_layers.iter_mut().zip(positions) {
            if let (Some(player), Some(position)) = (layer.player.as_mut(), position) {
                if let Some(frame) = player.frame_at(position) {
                    layer.frame = Some(frame);
                    updated = true;
                }
            }
        }

        if !updated && !self.needs_composite {
            return;
        }

        let layers: Vec<CompositeLayer> = self
            .preview_layers
            .iter()
            .filter_map(|layer| {
                layer.frame.as_ref().map(|frame| CompositeLayer {
                    rgba: &frame.rgba,
                    width: frame.width,
                    height: frame.height,
                    fit: layer.fit,
                })
            })
            .collect();
        if layers.is_empty() {
            return;
        }

        let (canvas_width, canvas_height) = self.preview_canvas_size(target);
        let rgba = composite_layers(canvas_width, canvas_height, &layers);
        self.needs_composite = false;

        let image = egui::ColorImage::from_rgba_unmultiplied(
            [canvas_width as usize, canvas_height as usize],
            &rgba,
        );
        if let Some(texture) = &mut self.preview {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.preview = Some(self.egui_ctx.load_texture(
                "openconvert-playback-preview",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }

    /// The source position to decode for a layer at the current playhead.
    fn layer_media_position(&self, key: &PlaybackStreamKey) -> Option<u64> {
        match (key.track_id, key.clip_id) {
            (Some(track_id), Some(clip_id)) => {
                let clip = self.editor.find_clip(track_id, clip_id)?;
                Some(
                    clip.source_start_ms
                        + self
                            .editor
                            .playhead_ms
                            .saturating_sub(clip.timeline_start_ms),
                )
            }
            _ => Some(self.editor.convert_preview_ms),
        }
    }

    /// The compositing canvas size: the export output size scaled to fit the
    /// preview box, so playback composites onto the same canvas the export uses.
    fn preview_canvas_size(&self, target: PlaybackTarget) -> (u32, u32) {
        let (width, height) = match target {
            PlaybackTarget::Timeline => {
                openconvert_media::export::output_video_size(&self.editor.project.timeline)
            }
            PlaybackTarget::Convert => {
                if self.editor.convert_width > 0 && self.editor.convert_height > 0 {
                    (self.editor.convert_width, self.editor.convert_height)
                } else {
                    (1280, 720)
                }
            }
        };
        fit_within_box(width, height, PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT)
    }

    /// Resolves a preview from the in-memory cache when possible, otherwise
    /// extracts it on a worker thread. This is only for scrubbing/seek/import
    /// style interactions; playback uses a persistent streaming decoder.
    fn request_preview(&mut self, kind: PreviewKind, media_path: PathBuf, at_ms: u64, force: bool) {
        let key = preview_key(&media_path, at_ms);

        if let Some(texture) = self.frame_cache.get(&key).cloned() {
            self.preview = Some(texture);
            self.last_preview_key = Some(key);
            return;
        }

        if !force && self.last_preview_key.as_ref() == Some(&key) {
            return;
        }
        self.last_preview_key = Some(key.clone());

        let request = PreviewRequest {
            kind,
            media_path,
            media_ms: key.1,
            key,
        };

        if self.preview_in_flight {
            self.pending_preview = Some(request);
            return;
        }
        self.start_preview(request);
    }

    fn start_pending_preview(&mut self) {
        if let Some(request) = self.pending_preview.take() {
            self.start_preview(request);
        }
    }

    fn start_preview(&mut self, request: PreviewRequest) {
        self.preview_in_flight = true;
        self.scrub.send(request);
    }

    // ------------------------------------------------------------------ playback
    pub(crate) fn toggle_playback(&mut self, target: PlaybackTarget) {
        if self.playback.is_playing() && self.playback.target() == target {
            self.playback.pause();
            self.stop_preview_player();
            self.status = "Playback paused".to_owned();
            return;
        }

        let base_position_ms = self.playback_position_ms(target);
        self.playback.start(target, base_position_ms);
        self.reseat_playback_sources(true);
        self.status = "Playback started".to_owned();
    }

    pub(crate) fn stop_playback(&mut self) {
        self.playback.stop();
        self.stop_preview_player();
        self.status = "Playback stopped".to_owned();
    }

    /// Move the playhead for a target. When `commit` is true *and* that
    /// target is currently playing, reseat the audio at the new position.
    /// Scrubbing passes `commit = false` so dragging only updates the
    /// playhead and preview — restarting the audio device per pointer event
    /// is what was making playback stall while seeking.
    pub(crate) fn seek_to(&mut self, target: PlaybackTarget, position_ms: u64, commit: bool) {
        match target {
            PlaybackTarget::Timeline => self.editor.playhead_ms = position_ms,
            PlaybackTarget::Convert => self.editor.convert_preview_ms = position_ms,
        }

        if self.playback.is_playing() && self.playback.target() == target {
            self.stop_preview_player();
        }

        match target {
            PlaybackTarget::Timeline => self.request_preview_at_playhead(false),
            PlaybackTarget::Convert => self.request_convert_preview(false),
        }

        if commit && self.playback.is_playing() && self.playback.target() == target {
            self.playback.start(target, position_ms);
            self.reseat_playback_sources(true);
        }
    }

    pub(crate) fn audio_source_for_target(&self, target: PlaybackTarget) -> Option<AudioSource> {
        match target {
            PlaybackTarget::Timeline => {
                let (track_id, clip_id) = self.editor.clip_at_time(self.editor.playhead_ms)?;
                let clip = self.editor.find_clip(track_id, clip_id)?;

                if clip.muted {
                    return None;
                }

                let media_position_ms = clip.source_start_ms
                    + self
                        .editor
                        .playhead_ms
                        .saturating_sub(clip.timeline_start_ms);
                Some(AudioSource {
                    path: PathBuf::from(&clip.source_path),
                    media_position_ms,
                })
            }
            PlaybackTarget::Convert => self.editor.convert_input.as_ref().map(|path| AudioSource {
                path: path.clone(),
                media_position_ms: self.editor.convert_preview_ms,
            }),
        }
    }

    /// Applies a new playback speed. While playing, the audio is rebuilt from
    /// the current position so the change is heard immediately: the libav audio
    /// source bakes its time-stretch ratio in at construction, so it cannot be
    /// retuned in place. The video clock just follows the rebased playback clock.
    pub(crate) fn set_playback_speed(&mut self, target: PlaybackTarget, speed: f32) {
        self.playback.set_speed(speed);
        if !self.playback.is_playing() || self.playback.target() != target {
            return;
        }
        let position_ms = self
            .playback
            .elapsed_position_ms(self.playback_position_ms(target));
        match target {
            PlaybackTarget::Timeline => self.editor.playhead_ms = position_ms,
            PlaybackTarget::Convert => self.editor.convert_preview_ms = position_ms,
        }
        let audio_source = self.audio_source_for_target(target);
        let _ = self.playback.restart_audio(audio_source);
    }

    fn playback_position_ms(&self, target: PlaybackTarget) -> u64 {
        match target {
            PlaybackTarget::Timeline => self.editor.playhead_ms,
            PlaybackTarget::Convert => self.editor.convert_preview_ms,
        }
    }

    fn sync_playback(&mut self, ctx: &egui::Context) {
        if !self.playback.is_playing() {
            return;
        }

        let fallback = match self.playback.target() {
            PlaybackTarget::Timeline => self.editor.playhead_ms.saturating_add(33),
            PlaybackTarget::Convert => self.editor.convert_preview_ms.saturating_add(33),
        };
        let elapsed_ms = self.playback.elapsed_position_ms(fallback);

        match self.playback.target() {
            PlaybackTarget::Timeline => {
                let duration = self.editor.timeline_duration_ms();
                if duration > 0 {
                    self.editor.playhead_ms = elapsed_ms.min(duration);
                    self.reseat_playback_sources(false);
                    if self.editor.playhead_ms >= duration {
                        self.stop_playback();
                    }
                }
            }
            PlaybackTarget::Convert => {
                if let Some(duration) = self.editor.convert_duration_ms {
                    self.editor.convert_preview_ms = elapsed_ms.min(duration);
                    self.reseat_playback_sources(false);
                    if self.editor.convert_preview_ms >= duration {
                        self.stop_playback();
                    }
                }
            }
        }

        self.present_preview_frame();

        ctx.request_repaint_after(Duration::from_millis(16));
    }

    // ------------------------------------------------------------------ clip ops
    pub(crate) fn commit_clip_move(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        dest_track_id: TrackId,
        new_start_ms: u64,
    ) {
        match self
            .editor
            .project
            .timeline
            .move_clip(track_id, clip_id, dest_track_id, new_start_ms)
        {
            Ok(()) => {
                self.editor.selected_track = Some(dest_track_id);
                self.editor.selected_clip = Some((dest_track_id, clip_id));
                self.request_preview_at_playhead(false);
                self.status = "Moved clip".to_owned();
            }
            Err(error) => self.status = format!("Move failed: {error}"),
        }
    }

    pub(crate) fn commit_clip_trim(
        &mut self,
        track_id: TrackId,
        clip_id: ClipId,
        placement: ClipPlacement,
    ) {
        match self.editor.project.timeline.trim_clip(
            track_id,
            clip_id,
            placement.timeline_start_ms,
            placement.source_start_ms,
            placement.duration_ms,
        ) {
            Ok(()) => {
                self.editor.selected_track = Some(track_id);
                self.editor.selected_clip = Some((track_id, clip_id));
                self.request_preview_at_playhead(false);
                self.status = "Trimmed clip".to_owned();
            }
            Err(error) => self.status = format!("Trim failed: {error}"),
        }
    }

    fn clip_for_playhead_edit(&self) -> Option<(TrackId, ClipId, openconvert_core::Clip)> {
        let playhead = self.editor.playhead_ms;
        if let Some((track_id, clip_id)) = self.editor.selected_clip() {
            if let Some(clip) = self.editor.find_clip(track_id, clip_id) {
                if clip.timeline_start_ms < playhead && playhead < clip.timeline_end_ms() {
                    return Some((track_id, clip_id, clip.clone()));
                }
            }
        }

        let (track_id, clip_id) = self.editor.clip_at_time(playhead)?;
        let clip = self.editor.find_clip(track_id, clip_id)?.clone();
        Some((track_id, clip_id, clip))
    }

    pub(crate) fn split_at_playhead(&mut self) {
        let Some((track_id, clip_id, clip)) = self.clip_for_playhead_edit() else {
            self.status = "Move the playhead inside a clip before cutting".to_owned();
            return;
        };

        if self.editor.playhead_ms <= clip.timeline_start_ms
            || self.editor.playhead_ms >= clip.timeline_end_ms()
        {
            self.status = "Move the playhead inside the clip before cutting".to_owned();
            return;
        }

        match self
            .editor
            .project
            .timeline
            .split_clip(track_id, clip_id, self.editor.playhead_ms)
        {
            Ok((left, _right)) => {
                self.editor.select_clip(track_id, left);
                self.request_preview_at_playhead(false);
                self.status = "Cut clip at playhead".to_owned();
            }
            Err(error) => self.status = format!("Cut failed: {error}"),
        }
    }

    pub(crate) fn trim_start_to_playhead(&mut self) {
        let Some((track_id, clip_id, clip)) = self.clip_for_playhead_edit() else {
            self.status = "Move playhead inside a clip before trimming start".to_owned();
            return;
        };
        let playhead = self.editor.playhead_ms;

        if playhead <= clip.timeline_start_ms || playhead >= clip.timeline_end_ms() {
            self.status = "Move playhead inside the clip before trimming start".to_owned();
            return;
        }

        let trim_amount = playhead - clip.timeline_start_ms;
        match self.editor.project.timeline.trim_clip(
            track_id,
            clip_id,
            playhead,
            clip.source_start_ms + trim_amount,
            clip.duration_ms - trim_amount,
        ) {
            Ok(()) => {
                self.editor.select_clip(track_id, clip_id);
                self.request_preview_at_playhead(false);
                self.status = "Trimmed clip start to playhead".to_owned();
            }
            Err(error) => self.status = format!("Trim failed: {error}"),
        }
    }

    pub(crate) fn trim_end_to_playhead(&mut self) {
        let Some((track_id, clip_id, clip)) = self.clip_for_playhead_edit() else {
            self.status = "Move playhead inside a clip before trimming end".to_owned();
            return;
        };
        let playhead = self.editor.playhead_ms;

        if playhead <= clip.timeline_start_ms || playhead >= clip.timeline_end_ms() {
            self.status = "Move playhead inside the clip before trimming end".to_owned();
            return;
        }

        match self.editor.project.timeline.trim_clip(
            track_id,
            clip_id,
            clip.timeline_start_ms,
            clip.source_start_ms,
            playhead - clip.timeline_start_ms,
        ) {
            Ok(()) => {
                self.editor.select_clip(track_id, clip_id);
                self.request_preview_at_playhead(false);
                self.status = "Trimmed clip end to playhead".to_owned();
            }
            Err(error) => self.status = format!("Trim failed: {error}"),
        }
    }

    pub(crate) fn toggle_selected_clip_mute(&mut self) {
        let Some((track_id, clip_id)) = self.editor.selected_clip() else {
            self.status = "Select a clip before muting".to_owned();
            return;
        };

        match self
            .editor
            .project
            .timeline
            .toggle_clip_mute(track_id, clip_id)
        {
            Ok(true) => self.status = "Muted selected clip audio".to_owned(),
            Ok(false) => self.status = "Unmuted selected clip audio".to_owned(),
            Err(error) => self.status = format!("Mute failed: {error}"),
        }
    }

    pub(crate) fn set_selected_clip_fit(&mut self, fit: openconvert_core::FitMode) {
        let Some((track_id, clip_id)) = self.editor.selected_clip() else {
            self.status = "Select a clip before changing fill mode".to_owned();
            return;
        };

        match self
            .editor
            .project
            .timeline
            .set_clip_fit_mode(track_id, clip_id, fit)
        {
            Ok(()) => {
                self.request_preview_at_playhead(true);
                self.status = "Updated clip fill mode".to_owned();
            }
            Err(error) => self.status = format!("Fill mode failed: {error}"),
        }
    }

    pub(crate) fn delete_selected_clip(&mut self) {
        let Some((track_id, clip_id)) = self.editor.selected_clip() else {
            self.status = "Select a clip before removing it".to_owned();
            return;
        };

        match self.editor.project.timeline.delete_clip(track_id, clip_id) {
            Ok(()) => {
                self.editor.selected_clip = None;
                self.status = "Removed selected clip".to_owned();
            }
            Err(error) => self.status = format!("Remove failed: {error}"),
        }
    }

    pub(crate) fn add_track(&mut self) {
        let track_id = self.editor.project.timeline.add_track();
        self.editor.select_track(track_id);
        self.status = format!("Added track {}", track_id.0);
    }

    pub(crate) fn undo(&mut self) {
        match self.editor.project.timeline.undo() {
            Ok(()) => self.status = "Undo applied".to_owned(),
            Err(error) => self.status = format!("Undo failed: {error}"),
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.editor.project.timeline.redo() {
            Ok(()) => self.status = "Redo applied".to_owned(),
            Err(error) => self.status = format!("Redo failed: {error}"),
        }
    }
}

impl eframe::App for OpenConvertApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_messages(&ctx);
        self.sync_playback(&ctx);

        // Spacebar toggles playback for the active panel unless a widget (e.g. a
        // text field or a focused button) is consuming keyboard input.
        if ctx.input(|input| input.key_pressed(egui::Key::Space))
            && ctx.memory(|memory| memory.focused().is_none())
        {
            let target = match self.panel {
                Panel::Edit => PlaybackTarget::Timeline,
                Panel::Convert => PlaybackTarget::Convert,
            };
            self.toggle_playback(target);
        }

        // `App::ui` provides a Ui with no background, so anything not
        // covered by a child panel would show the desktop wallpaper. Paint
        // the whole viewport first so the chrome stays opaque even when
        // panels overflow their default sizing.
        let viewport = ui.max_rect();
        ui.painter().rect_filled(viewport, 0.0, theme::PALETTE_BG);

        egui::Panel::top("top_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::PALETTE_BG)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show_inside(ui, |ui| self.draw_topbar(ui));

        egui::Panel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(theme::PALETTE_BG)
                    .inner_margin(egui::Margin::symmetric(20, 6)),
            )
            .show_inside(ui, |ui| {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&self.status).color(PALETTE_MUTED));
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::PALETTE_BG)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show_inside(ui, |ui| match self.panel {
                Panel::Edit => self.draw_edit_view(ui),
                Panel::Convert => self.draw_convert_view(ui),
            });
    }
}

/// Scales `width`×`height` to fit within the box, preserving aspect and never
/// upscaling, with even dimensions kept at least 2. Mirrors the decoder's sizing
/// so the composite canvas matches decoded frame proportions.
fn fit_within_box(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (max_width.max(2), max_height.max(2));
    }
    let scale = (max_width as f32 / width as f32)
        .min(max_height as f32 / height as f32)
        .min(1.0);
    let even = |value: f32| ((value.round() as u32).max(2)) & !1;
    (even(width as f32 * scale), even(height as f32 * scale))
}

/// Builds a preview/thumbnail texture from a decoded RGBA frame, uploading it to
/// the GPU directly — no disk round-trip.
fn frame_to_texture(ctx: &egui::Context, name: &str, frame: &DecodedFrame) -> egui::TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [frame.width as usize, frame.height as usize],
        &frame.rgba,
    );
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// Background worker that decodes scrub/seek preview frames on a persistent
/// [`ScrubDecoder`], so dragging the playhead within one clip reopens the source
/// only when it changes — not once per frame, the way a fresh `decode_frame_at`
/// would.
struct ScrubWorker {
    tx: Option<Sender<PreviewRequest>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ScrubWorker {
    fn spawn(sender: Sender<AppMessage>, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel::<PreviewRequest>();
        let handle = thread::spawn(move || run_scrub_worker(&rx, &sender, &ctx));
        Self {
            tx: Some(tx),
            handle: Some(handle),
        }
    }

    fn send(&self, request: PreviewRequest) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(request);
        }
    }
}

impl Drop for ScrubWorker {
    fn drop(&mut self) {
        // Drop the sender so the worker's recv() returns, then join the thread:
        // the decoder is torn down deterministically with no orphaned libav
        // state, mirroring PreviewPlayer's shutdown.
        self.tx = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_scrub_worker(
    rx: &Receiver<PreviewRequest>,
    sender: &Sender<AppMessage>,
    ctx: &egui::Context,
) {
    let mut decoder = ScrubDecoder::new();
    while let Ok(request) = rx.recv() {
        match decoder.frame_at(
            &request.media_path,
            request.media_ms,
            PREVIEW_MAX_WIDTH,
            PREVIEW_MAX_HEIGHT,
        ) {
            Ok(frame) => {
                let _ = sender.send(AppMessage::PreviewReady {
                    key: request.key,
                    frame,
                });
            }
            Err(error) => {
                let prefix = match request.kind {
                    PreviewKind::Timeline => "Preview failed",
                    PreviewKind::Convert => "Convert preview failed",
                };
                let _ = sender.send(AppMessage::PreviewFailed {
                    error: format!("{prefix}: {error}"),
                });
            }
        }
        ctx.request_repaint();
    }
}
