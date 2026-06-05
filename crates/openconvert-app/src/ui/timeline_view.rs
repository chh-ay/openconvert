use std::path::Path;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};

use crate::app::OpenConvertApp;
use crate::player::PlaybackTarget;
use crate::preview::{thumb_key, PreviewKey};
use crate::theme::{
    button, format_time, icon_button, PlayerIcon, PALETTE_ACCENT, PALETTE_BORDER,
    PALETTE_BORDER_SOFT, PALETTE_CLIP, PALETTE_CLIP_MUTED, PALETTE_CLIP_SELECTED,
    PALETTE_CLIP_STROKE, PALETTE_GRID, PALETTE_MUTED, PALETTE_PLAYHEAD, PALETTE_RULER_BG,
    PALETTE_SURFACE_RAISED, PALETTE_TEXT, PALETTE_TIMELINE_BG, PALETTE_TRACK_A, PALETTE_TRACK_B,
    PALETTE_TRACK_SELECTED, RADIUS_BUTTON, RADIUS_CARD,
};
use crate::timeline_geo::{
    clip_context_ms, clip_zone, ms_to_px, px_to_ms, resolve_trim_end, resolve_trim_start,
    ruler_step_ms, snap_move_start_ms, snap_ms, track_index_at_y, ClipPlacement, ClipZone,
    TimelineDrag, TRIM_HANDLE_PX, ZOOM_STEP,
};

const HEADER_WIDTH: f32 = 88.0;
const RULER_HEIGHT: f32 = 26.0;
const TRACK_HEIGHT: f32 = 58.0;
const CLIP_VPAD: f32 = 7.0;
const TIMELINE_PAD: f32 = 6.0;
const EDGE_PAD: f32 = 8.0;
const SNAP_PX: f32 = 9.0;

impl OpenConvertApp {
    pub(crate) fn draw_timeline_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(PALETTE_SURFACE_RAISED)
            .stroke(Stroke::new(1.0, PALETTE_BORDER_SOFT))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 32.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if toolbar_icon_button(ui, PlayerIcon::Previous)
                            .on_hover_text("Previous clip")
                            .clicked()
                        {
                            self.editor.select_previous_clip();
                            self.request_preview_at_playhead(false);
                        }
                        if toolbar_icon_button(ui, PlayerIcon::Next)
                            .on_hover_text("Next clip")
                            .clicked()
                        {
                            self.editor.select_next_clip();
                            self.request_preview_at_playhead(false);
                        }
                        if toolbar_button(ui, "-1s").clicked() {
                            self.editor.nudge_playhead(-1_000);
                            self.request_preview_at_playhead(false);
                        }
                        if toolbar_button(ui, "+1s").clicked() {
                            self.editor.nudge_playhead(1_000);
                            self.request_preview_at_playhead(false);
                        }

                        ui.add(toolbar_divider());

                        if toolbar_button(ui, "Split")
                            .on_hover_text("Cut at playhead")
                            .clicked()
                        {
                            self.split_at_playhead();
                        }
                        if toolbar_button(ui, "Mute")
                            .on_hover_text("Mute / unmute selected clip")
                            .clicked()
                        {
                            self.toggle_selected_clip_mute();
                        }
                        if toolbar_button(ui, "Delete")
                            .on_hover_text("Remove selected clip")
                            .clicked()
                        {
                            self.delete_selected_clip();
                        }

                        ui.add(toolbar_divider());

                        if toolbar_button(ui, "Zoom -")
                            .on_hover_text("Zoom out")
                            .clicked()
                        {
                            self.zoom_timeline(1.0 / ZOOM_STEP);
                        }
                        if toolbar_button(ui, "Fit")
                            .on_hover_text("Fit project to view")
                            .clicked()
                        {
                            self.editor.request_fit();
                        }
                        if toolbar_button(ui, "Zoom +")
                            .on_hover_text("Zoom in")
                            .clicked()
                        {
                            self.zoom_timeline(ZOOM_STEP);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if toolbar_button(ui, "Redo").clicked() {
                                self.redo();
                            }
                            if toolbar_button(ui, "Undo").clicked() {
                                self.undo();
                            }
                        });
                    },
                );
            });
    }

    /// Zooms the timeline around the playhead so it stays put on screen.
    fn zoom_timeline(&mut self, factor: f32) {
        let anchor_ms = self.editor.playhead_ms;
        let anchor_offset =
            ms_to_px(anchor_ms, self.editor.pixels_per_second) - self.editor.timeline_scroll_x;
        self.editor.zoom(factor, anchor_ms, anchor_offset);
    }

    pub(crate) fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let track_count = self.editor.project.timeline.track_count().max(1);
        let real_duration = self.editor.timeline_duration_ms();
        let duration = real_duration.max(1);

        let height = RULER_HEIGHT + track_count as f32 * TRACK_HEIGHT + TIMELINE_PAD * 2.0;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), height),
            Sense::click_and_drag(),
        );

        let content_left = rect.left() + HEADER_WIDTH;
        let lanes_top = rect.top() + RULER_HEIGHT;
        let visible_width = (rect.right() - content_left - EDGE_PAD).max(16.0);

        if self.editor.pending_fit {
            self.editor.fit_view(visible_width);
        }

        // Mouse-wheel: ctrl/pinch zooms around the pointer, otherwise pans.
        if response.hovered() {
            let (zoom_delta, scroll_delta, hover_x) = ui.input(|input| {
                (
                    input.zoom_delta(),
                    input.smooth_scroll_delta,
                    input.pointer.hover_pos().map(|pos| pos.x),
                )
            });
            if (zoom_delta - 1.0).abs() > f32::EPSILON {
                let anchor_x = hover_x.unwrap_or(content_left);
                let offset = (anchor_x - content_left).max(0.0);
                let anchor_ms = px_to_ms(
                    offset + self.editor.timeline_scroll_x,
                    self.editor.pixels_per_second,
                );
                self.editor.zoom(zoom_delta, anchor_ms, offset);
            } else if scroll_delta != Vec2::ZERO {
                let delta = if scroll_delta.x.abs() > scroll_delta.y.abs() {
                    scroll_delta.x
                } else {
                    scroll_delta.y
                };
                self.editor.timeline_scroll_x -= delta;
            }
        }

        let pps = self.editor.pixels_per_second;
        let content_px = ms_to_px(duration, pps);
        let max_scroll = (content_px + 160.0 - visible_width).max(0.0);

        // Keep the playhead on screen while playing.
        if self.playback.is_playing() && self.playback.target() == PlaybackTarget::Timeline {
            let playhead_px = ms_to_px(self.editor.playhead_ms, pps);
            let rel = playhead_px - self.editor.timeline_scroll_x;
            if rel < visible_width * 0.1 || rel > visible_width * 0.9 {
                self.editor.timeline_scroll_x = playhead_px - visible_width * 0.4;
            }
        }

        self.editor.timeline_scroll_x = self.editor.timeline_scroll_x.clamp(0.0, max_scroll);
        let scroll = self.editor.timeline_scroll_x;

        let ms_to_x = |ms: u64| content_left - scroll + ms_to_px(ms, pps);
        let x_to_ms = |x: f32| px_to_ms(x - content_left + scroll, pps);

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, RADIUS_CARD, PALETTE_TIMELINE_BG);
        painter.rect_stroke(
            rect,
            RADIUS_CARD,
            Stroke::new(1.0, PALETTE_BORDER),
            StrokeKind::Inside,
        );

        // Ruler band plus vertical grid lines down the lanes.
        let ruler_rect = Rect::from_min_max(
            Pos2::new(content_left, rect.top()),
            Pos2::new(rect.right(), lanes_top),
        );
        painter.rect_filled(ruler_rect, 0.0, PALETTE_RULER_BG);

        // Corner cell names the panel so the tracks area reads as the timeline.
        painter.rect_filled(
            Rect::from_min_max(rect.left_top(), Pos2::new(content_left, lanes_top)),
            0.0,
            PALETTE_SURFACE_RAISED,
        );
        painter.text(
            Pos2::new(rect.left() + 12.0, rect.top() + RULER_HEIGHT * 0.5),
            Align2::LEFT_CENTER,
            "Timeline",
            FontId::proportional(12.5),
            PALETTE_TEXT,
        );

        let step = ruler_step_ms(pps);
        let first_ms = x_to_ms(content_left) / step * step;
        let last_ms = x_to_ms(rect.right());
        let mut tick_ms = first_ms;
        while tick_ms <= last_ms {
            let x = ms_to_x(tick_ms);
            if x >= content_left - 1.0 {
                painter.line_segment(
                    [Pos2::new(x, lanes_top - 6.0), Pos2::new(x, lanes_top)],
                    Stroke::new(1.0, PALETTE_GRID),
                );
                painter.line_segment(
                    [
                        Pos2::new(x, lanes_top),
                        Pos2::new(x, rect.bottom() - TIMELINE_PAD),
                    ],
                    Stroke::new(1.0, PALETTE_GRID.gamma_multiply(0.5)),
                );
                painter.text(
                    Pos2::new(x + 4.0, rect.top() + 4.0),
                    Align2::LEFT_TOP,
                    ruler_label(tick_ms, step),
                    FontId::proportional(11.0),
                    PALETTE_MUTED,
                );
            }
            tick_ms = tick_ms.saturating_add(step);
        }

        // Active drag geometry, recomputed each frame so the model is untouched
        // until the pointer is released.
        let active_drag = self.timeline_drag;
        let pointer = ui.input(|input| input.pointer.interact_pos());
        let snap_threshold = px_to_ms(SNAP_PX, pps).max(1);
        let mut move_preview: Option<(usize, u64, openconvert_core::TrackId)> = None;
        let mut trim_preview: Option<ClipPlacement> = None;
        if let (Some(drag), Some(p)) = (active_drag, pointer) {
            if let Some((track_id, clip_id)) = drag.target() {
                if let Some(clip) = self.editor.find_clip(track_id, clip_id).cloned() {
                    let candidates = snap_candidates(&self.editor, track_id, clip_id);
                    match drag {
                        TimelineDrag::Move { grab_offset_ms, .. } => {
                            let raw_start = x_to_ms(p.x).saturating_sub(grab_offset_ms);
                            let start = snap_move_start_ms(
                                raw_start,
                                clip.duration_ms,
                                &candidates,
                                snap_threshold,
                            );
                            let dest_index =
                                track_index_at_y(p.y, lanes_top, TRACK_HEIGHT, track_count)
                                    .unwrap_or_else(|| {
                                        original_track_index(&self.editor, track_id)
                                    });
                            let dest_track = self.editor.project.timeline.tracks()[dest_index].id;
                            move_preview = Some((dest_index, start, dest_track));
                        }
                        TimelineDrag::TrimStart { .. } => {
                            let desired = snap_ms(x_to_ms(p.x), &candidates, snap_threshold);
                            let (floor, _) = clip.source_bounds();
                            trim_preview = Some(resolve_trim_start(
                                clip.timeline_start_ms,
                                clip.source_start_ms,
                                clip.duration_ms,
                                floor,
                                desired,
                            ));
                        }
                        TimelineDrag::TrimEnd { .. } => {
                            let desired = snap_ms(x_to_ms(p.x), &candidates, snap_threshold);
                            let (_, ceiling) = clip.source_bounds();
                            trim_preview = Some(resolve_trim_end(
                                clip.timeline_start_ms,
                                clip.source_start_ms,
                                ceiling,
                                desired,
                            ));
                        }
                        TimelineDrag::Playhead => {}
                    }
                }
            }
        }

        let mut pending_track_move: Option<(openconvert_core::TrackId, usize)> = None;

        // Track lanes and headers.
        for index in 0..track_count {
            let row_top = lanes_top + index as f32 * TRACK_HEIGHT;
            let lane_rect = Rect::from_min_max(
                Pos2::new(content_left, row_top),
                Pos2::new(rect.right(), row_top + TRACK_HEIGHT),
            );
            let header_rect = Rect::from_min_max(
                Pos2::new(rect.left(), row_top),
                Pos2::new(content_left, row_top + TRACK_HEIGHT),
            );

            let track_id = self
                .editor
                .project
                .timeline
                .tracks()
                .get(index)
                .map(|t| t.id);
            let selected = track_id.is_some() && self.editor.selected_track == track_id;
            let lane_fill = if selected {
                PALETTE_TRACK_SELECTED
            } else if index % 2 == 0 {
                PALETTE_TRACK_A
            } else {
                PALETTE_TRACK_B
            };
            painter.rect_filled(lane_rect, 0.0, lane_fill);
            painter.rect_filled(header_rect, 0.0, PALETTE_SURFACE_RAISED);
            painter.line_segment(
                [header_rect.right_top(), header_rect.right_bottom()],
                Stroke::new(1.0, PALETTE_BORDER_SOFT),
            );

            let clip_count = self
                .editor
                .project
                .timeline
                .tracks()
                .get(index)
                .map(|t| t.clips().len())
                .unwrap_or(0);
            painter.text(
                header_rect.left_top() + Vec2::new(12.0, 10.0),
                Align2::LEFT_TOP,
                format!("V{}", index + 1),
                FontId::proportional(14.0),
                if selected {
                    PALETTE_TEXT
                } else {
                    PALETTE_MUTED
                },
            );
            painter.text(
                header_rect.left_top() + Vec2::new(12.0, 30.0),
                Align2::LEFT_TOP,
                format!(
                    "{clip_count} clip{}",
                    if clip_count == 1 { "" } else { "s" }
                ),
                FontId::proportional(10.5),
                PALETTE_MUTED,
            );

            // Up/down controls change the track's compositing layer (z-order).
            if track_count > 1 {
                if let Some(track_id) = track_id {
                    let size = Vec2::splat(16.0);
                    let bx = header_rect.right() - 22.0;
                    let up_rect = Rect::from_min_size(Pos2::new(bx, row_top + 8.0), size);
                    let down_rect = Rect::from_min_size(Pos2::new(bx, row_top + 30.0), size);

                    let up_enabled = index > 0;
                    let up = ui.interact(
                        up_rect,
                        ui.id().with(("track_up", track_id.0)),
                        Sense::click(),
                    );
                    paint_track_arrow(&painter, up_rect, true, up_enabled, up.hovered());
                    if up_enabled && up.clicked() {
                        pending_track_move = Some((track_id, index - 1));
                    }

                    let down_enabled = index + 1 < track_count;
                    let down = ui.interact(
                        down_rect,
                        ui.id().with(("track_down", track_id.0)),
                        Sense::click(),
                    );
                    paint_track_arrow(&painter, down_rect, false, down_enabled, down.hovered());
                    if down_enabled && down.clicked() {
                        pending_track_move = Some((track_id, index + 1));
                    }
                }
            }
        }

        // Clips. Collected interactions are applied after the immutable borrow.
        let mut pending_select: Option<(openconvert_core::TrackId, openconvert_core::ClipId)> =
            None;
        let mut pending_drag_start: Option<(
            openconvert_core::TrackId,
            openconvert_core::ClipId,
            ClipZone,
            u64,
        )> = None;
        let mut pending_context: Option<(
            openconvert_core::TrackId,
            openconvert_core::ClipId,
            TimelineClipAction,
            u64,
        )> = None;
        let mut hover_cursor: Option<CursorIcon> = None;
        let mut thumb_requests: Vec<PreviewKey> = Vec::new();
        let thumb_painter = painter.with_clip_rect(Rect::from_min_max(
            Pos2::new(content_left, lanes_top),
            rect.right_bottom(),
        ));

        for (index, track) in self.editor.project.timeline.tracks().iter().enumerate() {
            for clip in track.clips() {
                let dragged = active_drag
                    .and_then(|d| d.target())
                    .is_some_and(|(t, c)| t == track.id && c == clip.id);

                // Resolve the geometry and source window this clip shows this frame.
                let (row_index, start_ms, duration_ms, source_start_ms) = if dragged {
                    if let Some((dest_index, start, _)) = move_preview {
                        (dest_index, start, clip.duration_ms, clip.source_start_ms)
                    } else if let Some(placement) = trim_preview {
                        (
                            index,
                            placement.timeline_start_ms,
                            placement.duration_ms,
                            placement.source_start_ms,
                        )
                    } else {
                        (
                            index,
                            clip.timeline_start_ms,
                            clip.duration_ms,
                            clip.source_start_ms,
                        )
                    }
                } else {
                    (
                        index,
                        clip.timeline_start_ms,
                        clip.duration_ms,
                        clip.source_start_ms,
                    )
                };

                let clip_left = ms_to_x(start_ms);
                let clip_width = ms_to_px(duration_ms, pps).max(6.0);
                let clip_right = clip_left + clip_width;
                if clip_right < content_left || clip_left > rect.right() {
                    continue;
                }

                let row_top = lanes_top + row_index as f32 * TRACK_HEIGHT;
                let clip_rect = Rect::from_min_max(
                    Pos2::new(clip_left.max(content_left), row_top + CLIP_VPAD),
                    Pos2::new(clip_right, row_top + TRACK_HEIGHT - CLIP_VPAD),
                );

                let selected = self.editor.selected_clip == Some((track.id, clip.id));
                let label = clip_label(&clip.source_path, duration_ms);
                let fill = clip_fill_color(selected, clip.muted, dragged);
                painter.rect_filled(clip_rect, 7.0, fill);
                let strip = Rect::from_min_max(
                    Pos2::new(clip_left + 2.0, clip_rect.top() + 2.0),
                    Pos2::new(clip_right - 2.0, clip_rect.bottom() - 2.0),
                );
                self.draw_clip_thumbnails(
                    &thumb_painter,
                    strip,
                    &clip.source_path,
                    source_start_ms,
                    duration_ms,
                    &mut thumb_requests,
                );
                paint_clip_overlay(&painter, clip_rect, &label, selected, dragged, clip.muted);

                let id = ui.make_persistent_id(("oc_clip", clip.id.0));
                let clip_response = ui.interact(clip_rect, id, Sense::click_and_drag());

                if clip_response.hovered() {
                    if let Some(p) = pointer {
                        hover_cursor = Some(
                            match clip_zone(clip_left, clip_width, p.x, TRIM_HANDLE_PX) {
                                ClipZone::TrimStart | ClipZone::TrimEnd => {
                                    CursorIcon::ResizeHorizontal
                                }
                                ClipZone::Body => CursorIcon::Grab,
                            },
                        );
                    }
                }

                let pointer_context = clip_response
                    .interact_pointer_pos()
                    .map(|pos| (pos.x, pos.y, x_to_ms(pos.x)));
                let context_ms = clip_context_ms(
                    pointer_context,
                    (
                        clip_rect.left(),
                        clip_rect.right(),
                        clip_rect.top(),
                        clip_rect.bottom(),
                    ),
                    self.editor.playhead_ms,
                    (clip.timeline_start_ms, clip.timeline_end_ms()),
                );

                if clip_response.drag_started() {
                    if let Some(p) = clip_response.interact_pointer_pos() {
                        let zone = clip_zone(clip_left, clip_width, p.x, TRIM_HANDLE_PX);
                        let grab = x_to_ms(p.x).saturating_sub(clip.timeline_start_ms);
                        pending_drag_start = Some((track.id, clip.id, zone, grab));
                    }
                } else if clip_response.clicked() {
                    pending_select = Some((track.id, clip.id));
                } else if clip_response.secondary_clicked() {
                    self.editor.playhead_ms = context_ms;
                    pending_select = Some((track.id, clip.id));
                }

                clip_response.context_menu(|ui| {
                    if ui.button("Cut here").clicked() {
                        pending_context =
                            Some((track.id, clip.id, TimelineClipAction::Split, context_ms));
                        ui.close();
                    }
                    if ui.button("Trim start here").clicked() {
                        pending_context =
                            Some((track.id, clip.id, TimelineClipAction::TrimStart, context_ms));
                        ui.close();
                    }
                    if ui.button("Trim end here").clicked() {
                        pending_context =
                            Some((track.id, clip.id, TimelineClipAction::TrimEnd, context_ms));
                        ui.close();
                    }
                    ui.separator();
                    let mute_label = if clip.muted {
                        "Unmute audio"
                    } else {
                        "Mute audio"
                    };
                    if ui.button(mute_label).clicked() {
                        pending_context = Some((
                            track.id,
                            clip.id,
                            TimelineClipAction::ToggleMute,
                            context_ms,
                        ));
                        ui.close();
                    }
                    ui.menu_button("Fill mode", |ui| {
                        for (label, mode) in [
                            ("Contain (fit)", openconvert_core::FitMode::Contain),
                            ("Cover (crop)", openconvert_core::FitMode::Cover),
                            ("Stretch", openconvert_core::FitMode::Stretch),
                        ] {
                            if ui.selectable_label(clip.fit_mode == mode, label).clicked() {
                                pending_context = Some((
                                    track.id,
                                    clip.id,
                                    TimelineClipAction::SetFit(mode),
                                    context_ms,
                                ));
                                ui.close();
                            }
                        }
                    });
                    if ui.button("Remove clip").clicked() {
                        pending_context =
                            Some((track.id, clip.id, TimelineClipAction::Delete, context_ms));
                        ui.close();
                    }
                });
            }
        }

        // Playback already repaints at 30 FPS. Do not start thumbnail FFmpeg
        // workers during playback; they compete with audio/playhead updates and
        // make preview playback look like a CPU spike.
        if !self.playback.is_playing() {
            for key in thumb_requests {
                self.request_thumbnail(key);
            }
        }

        if self.editor.project.timeline.is_empty() {
            painter.text(
                Pos2::new(
                    content_left + visible_width * 0.5,
                    lanes_top + TRACK_HEIGHT * 0.5,
                ),
                Align2::CENTER_CENTER,
                "Import media to start editing",
                FontId::proportional(15.0),
                PALETTE_MUTED,
            );
        }

        // Playhead line and grab handle.
        let playhead_x = ms_to_x(self.editor.playhead_ms);
        if playhead_x >= content_left - 1.0 && playhead_x <= rect.right() {
            painter.line_segment(
                [
                    Pos2::new(playhead_x, rect.top()),
                    Pos2::new(playhead_x, rect.bottom() - TIMELINE_PAD),
                ],
                Stroke::new(2.0, PALETTE_PLAYHEAD),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(playhead_x - 6.0, rect.top()),
                    Pos2::new(playhead_x + 6.0, rect.top()),
                    Pos2::new(playhead_x, rect.top() + 9.0),
                ],
                PALETTE_PLAYHEAD,
                Stroke::NONE,
            ));
        }

        let cursor = match active_drag {
            Some(TimelineDrag::Move { .. }) => Some(CursorIcon::Grabbing),
            Some(TimelineDrag::TrimStart { .. })
            | Some(TimelineDrag::TrimEnd { .. })
            | Some(TimelineDrag::Playhead) => Some(CursorIcon::ResizeHorizontal),
            None => hover_cursor,
        };
        if let Some(icon) = cursor {
            ui.ctx().set_cursor_icon(icon);
        }

        let (primary_down, primary_released) = ui.input(|input| {
            (
                input.pointer.primary_down(),
                input.pointer.primary_released(),
            )
        });
        let clamp_seek = |ms: u64| ms.min(real_duration);

        // Begin a drag.
        if let Some((track_id, clip_id, zone, grab)) = pending_drag_start {
            self.editor.selected_track = Some(track_id);
            self.editor.selected_clip = Some((track_id, clip_id));
            self.timeline_drag = Some(match zone {
                ClipZone::Body => TimelineDrag::Move {
                    track: track_id,
                    clip: clip_id,
                    grab_offset_ms: grab,
                },
                ClipZone::TrimStart => TimelineDrag::TrimStart {
                    track: track_id,
                    clip: clip_id,
                },
                ClipZone::TrimEnd => TimelineDrag::TrimEnd {
                    track: track_id,
                    clip: clip_id,
                },
            });
        } else if response.drag_started() && self.timeline_drag.is_none() {
            if let Some(p) = pointer {
                if p.x >= content_left {
                    self.timeline_drag = Some(TimelineDrag::Playhead);
                }
            }
        }

        // Live scrub while dragging the playhead.
        if primary_down {
            if let (Some(TimelineDrag::Playhead), Some(p)) = (self.timeline_drag, pointer) {
                let ms = clamp_seek(x_to_ms(p.x));
                self.seek_to(PlaybackTarget::Timeline, ms, false);
            }
        }

        // Commit on release.
        if primary_released {
            match self.timeline_drag.take() {
                Some(TimelineDrag::Playhead) => {
                    if let Some(p) = pointer {
                        let ms = clamp_seek(x_to_ms(p.x));
                        self.seek_to(PlaybackTarget::Timeline, ms, true);
                    }
                }
                Some(TimelineDrag::Move { track, clip, .. }) => {
                    if let Some((_, start, dest)) = move_preview {
                        self.commit_clip_move(track, clip, dest, start);
                    }
                }
                Some(TimelineDrag::TrimStart { track, clip })
                | Some(TimelineDrag::TrimEnd { track, clip }) => {
                    if let Some(placement) = trim_preview {
                        self.commit_clip_trim(track, clip, placement);
                    }
                }
                None => {}
            }
        }

        // Clicks: select a clip, select a track header, or seek.
        if pending_drag_start.is_none() {
            if let Some((track_id, clip_id)) = pending_select {
                self.editor.selected_track = Some(track_id);
                self.editor.selected_clip = Some((track_id, clip_id));
            } else if response.clicked() {
                if let Some(p) = pointer {
                    if p.x < content_left {
                        if let Some(index) =
                            track_index_at_y(p.y, lanes_top, TRACK_HEIGHT, track_count)
                        {
                            if let Some(track) = self.editor.project.timeline.tracks().get(index) {
                                let id = track.id;
                                self.editor.select_track(id);
                            }
                        }
                    } else {
                        let ms = clamp_seek(x_to_ms(p.x));
                        self.seek_to(PlaybackTarget::Timeline, ms, true);
                    }
                }
            }
        }

        if let Some((track_id, to_index)) = pending_track_move {
            let _ = self.editor.project.timeline.move_track(track_id, to_index);
            self.editor.select_track(track_id);
        }

        if let Some((track_id, clip_id, action, context_ms)) = pending_context {
            self.editor.select_clip(track_id, clip_id);
            match action {
                TimelineClipAction::Split => {
                    self.editor.playhead_ms = context_ms;
                    self.split_at_playhead();
                }
                TimelineClipAction::TrimStart => {
                    self.editor.playhead_ms = context_ms;
                    self.trim_start_to_playhead();
                }
                TimelineClipAction::TrimEnd => {
                    self.editor.playhead_ms = context_ms;
                    self.trim_end_to_playhead();
                }
                TimelineClipAction::ToggleMute => self.toggle_selected_clip_mute(),
                TimelineClipAction::SetFit(mode) => self.set_selected_clip_fit(mode),
                TimelineClipAction::Delete => self.delete_selected_clip(),
            }
        }
    }

    /// Tiles cached thumbnails across a clip's filmstrip and records the source
    /// positions still missing so a worker can extract them. `painter` is clipped
    /// to the lanes area, so a clip scrolled past the left edge never paints over
    /// the track headers.
    fn draw_clip_thumbnails(
        &self,
        painter: &egui::Painter,
        strip: Rect,
        source_path: &str,
        source_start_ms: u64,
        duration_ms: u64,
        requests: &mut Vec<PreviewKey>,
    ) {
        if strip.width() < 8.0 || strip.height() < 8.0 || duration_ms == 0 {
            return;
        }
        let path = Path::new(source_path);
        let slot_width = (strip.height() * 16.0 / 9.0).clamp(44.0, 220.0);
        let slots = (strip.width() / slot_width).ceil() as usize;
        for slot in 0..slots {
            let left = strip.left() + slot as f32 * slot_width;
            let right = (left + slot_width).min(strip.right());
            let center_fraction =
                (((left + slot_width * 0.5) - strip.left()) / strip.width()).clamp(0.0, 1.0);
            let source_ms = source_start_ms + (center_fraction * duration_ms as f32) as u64;
            let key = thumb_key(path, source_ms);
            match self.thumb_cache.get(&key) {
                Some(texture) => {
                    let uv =
                        Rect::from_min_max(Pos2::ZERO, Pos2::new((right - left) / slot_width, 1.0));
                    let destination = Rect::from_min_max(
                        Pos2::new(left, strip.top()),
                        Pos2::new(right, strip.bottom()),
                    );
                    painter.image(texture.id(), destination, uv, Color32::WHITE);
                }
                None => {
                    if !requests.contains(&key) {
                        requests.push(key);
                    }
                }
            }
        }
    }
}

fn snap_candidates(
    editor: &crate::editor::EditorState,
    skip_track: openconvert_core::TrackId,
    skip_clip: openconvert_core::ClipId,
) -> Vec<u64> {
    let mut candidates = vec![0, editor.playhead_ms];
    for track in editor.project.timeline.tracks() {
        for clip in track.clips() {
            if track.id == skip_track && clip.id == skip_clip {
                continue;
            }
            candidates.push(clip.timeline_start_ms);
            candidates.push(clip.timeline_end_ms());
        }
    }
    candidates
}

fn original_track_index(
    editor: &crate::editor::EditorState,
    track_id: openconvert_core::TrackId,
) -> usize {
    editor
        .project
        .timeline
        .tracks()
        .iter()
        .position(|track| track.id == track_id)
        .unwrap_or(0)
}

fn clip_fill_color(selected: bool, muted: bool, dragged: bool) -> Color32 {
    let fill = if muted {
        PALETTE_CLIP_MUTED
    } else if selected {
        PALETTE_CLIP_SELECTED
    } else {
        PALETTE_CLIP
    };
    if dragged {
        fill.gamma_multiply(0.85)
    } else {
        fill
    }
}

/// Draws the clip frame, selection handles, mute badge, and name on top of the
/// fill and any thumbnails already painted into the clip.
fn paint_clip_overlay(
    painter: &egui::Painter,
    clip_rect: Rect,
    label: &str,
    selected: bool,
    dragged: bool,
    muted: bool,
) {
    let muted_stroke = Color32::from_rgb(255, 184, 82);
    let stroke_color = if muted {
        muted_stroke
    } else if selected || dragged {
        PALETTE_ACCENT
    } else {
        PALETTE_CLIP_STROKE
    };
    let stroke_width = if selected || dragged || muted {
        1.8
    } else {
        1.0
    };
    painter.rect_stroke(
        clip_rect,
        7.0,
        Stroke::new(stroke_width, stroke_color),
        StrokeKind::Inside,
    );

    if selected && clip_rect.width() > 24.0 {
        // Draw a grab bar at each edge sized to the trim hit zone, so the
        // draggable region is visible rather than an invisible margin.
        let handle_w = TRIM_HANDLE_PX.min(clip_rect.width() / 3.0);
        let edges = [
            (clip_rect.left() + 2.0, clip_rect.left() + 2.0 + handle_w),
            (clip_rect.right() - 2.0 - handle_w, clip_rect.right() - 2.0),
        ];
        for (min_x, max_x) in edges {
            let handle_rect = Rect::from_min_max(
                Pos2::new(min_x, clip_rect.top() + 5.0),
                Pos2::new(max_x, clip_rect.bottom() - 5.0),
            );
            painter.rect_filled(handle_rect, 4.0, PALETTE_ACCENT);
            let grip_x = handle_rect.center().x;
            painter.line_segment(
                [
                    Pos2::new(grip_x, handle_rect.top() + 4.0),
                    Pos2::new(grip_x, handle_rect.bottom() - 4.0),
                ],
                Stroke::new(1.5, Color32::from_black_alpha(130)),
            );
        }
    }

    if muted && clip_rect.width() > 76.0 {
        let badge_rect = Rect::from_min_size(
            Pos2::new(clip_rect.right() - 58.0, clip_rect.top() + 6.0),
            Vec2::new(48.0, 16.0),
        );
        painter.rect_filled(badge_rect, 6.0, Color32::from_black_alpha(150));
        painter.rect_stroke(
            badge_rect,
            6.0,
            Stroke::new(1.0, muted_stroke),
            StrokeKind::Inside,
        );
        painter.text(
            badge_rect.center(),
            Align2::CENTER_CENTER,
            "MUTED",
            FontId::proportional(9.5),
            muted_stroke,
        );
    }

    if clip_rect.width() > 36.0 {
        let text_rect = clip_rect.shrink2(Vec2::new(9.0, 0.0));
        let text = elide_label(label, text_rect.width());
        let pos = text_rect.left_top() + Vec2::new(0.0, 8.0);
        // A shadow keeps the name readable over bright thumbnails.
        painter.text(
            pos + Vec2::splat(1.0),
            Align2::LEFT_TOP,
            text.as_str(),
            FontId::proportional(12.0),
            Color32::from_black_alpha(180),
        );
        painter.text(
            pos,
            Align2::LEFT_TOP,
            text.as_str(),
            FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}

fn toolbar_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized([72.0, 32.0], button(label).corner_radius(RADIUS_BUTTON))
}

fn toolbar_icon_button(ui: &mut egui::Ui, icon: PlayerIcon) -> egui::Response {
    icon_button(ui, icon, 32.0)
}

fn toolbar_divider() -> impl egui::Widget {
    move |ui: &mut egui::Ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(9.0, 24.0), Sense::hover());
        ui.painter().vline(
            rect.center().x,
            rect.top()..=rect.bottom(),
            Stroke::new(1.0, PALETTE_BORDER_SOFT),
        );
        response
    }
}

fn clip_label(source_path: &str, duration_ms: u64) -> String {
    let name = Path::new(source_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source_path);
    let seconds = duration_ms / 1_000;
    let millis = duration_ms % 1_000;
    format!("{name}  {seconds}.{millis:03}s")
}

/// Trims a label to roughly fit `width` pixels, appending an ellipsis.
fn elide_label(label: &str, width: f32) -> String {
    let max_chars = (width / 7.0).floor() as usize;
    if label.chars().count() <= max_chars || max_chars == 0 {
        return label.to_owned();
    }
    let keep = max_chars.saturating_sub(1).max(1);
    let mut out: String = label.chars().take(keep).collect();
    out.push('…');
    out
}

fn ruler_label(milliseconds: u64, step_ms: u64) -> String {
    if step_ms >= 1_000 {
        return format_time(milliseconds);
    }

    let total_seconds = milliseconds / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let millis = milliseconds % 1_000;
    format!("{minutes}:{seconds:02}.{millis:03}")
}

/// Paints a small up/down triangle used by the track-reorder controls.
fn paint_track_arrow(painter: &egui::Painter, rect: Rect, up: bool, enabled: bool, hot: bool) {
    let color = if !enabled {
        PALETTE_BORDER_SOFT
    } else if hot {
        PALETTE_TEXT
    } else {
        PALETTE_MUTED
    };
    let center = rect.center();
    let half = 4.0;
    let points = if up {
        vec![
            Pos2::new(center.x, center.y - half),
            Pos2::new(center.x - half, center.y + half),
            Pos2::new(center.x + half, center.y + half),
        ]
    } else {
        vec![
            Pos2::new(center.x, center.y + half),
            Pos2::new(center.x - half, center.y - half),
            Pos2::new(center.x + half, center.y - half),
        ]
    };
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

#[derive(Debug, Clone, Copy)]
enum TimelineClipAction {
    Split,
    TrimStart,
    TrimEnd,
    ToggleMute,
    SetFit(openconvert_core::FitMode),
    Delete,
}
