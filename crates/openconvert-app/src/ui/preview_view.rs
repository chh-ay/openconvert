use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use crate::app::OpenConvertApp;
use crate::player::PlaybackTarget;
use crate::theme::{
    format_time, icon_button, PlayerIcon, PALETTE_BORDER, PALETTE_BORDER_SOFT, PALETTE_MUTED,
    PALETTE_PREVIEW_BG, PALETTE_SURFACE_RAISED, PALETTE_TEXT, RADIUS_CARD,
};

const PREVIEW_ASPECT: f32 = 16.0 / 9.0;
const PREVIEW_MIN_HEIGHT: f32 = 260.0;
const PREVIEW_MAX_HEIGHT: f32 = 460.0;

const SPEEDS: &[f32] = &[0.5, 1.0, 1.25, 1.5, 2.0];

impl OpenConvertApp {
    pub(crate) fn draw_preview_frame(&mut self, ui: &mut egui::Ui, empty_message: &str) {
        let available_width = ui.available_width();
        let height =
            (available_width / PREVIEW_ASPECT).clamp(PREVIEW_MIN_HEIGHT, PREVIEW_MAX_HEIGHT);

        let (rect, _) = ui.allocate_exact_size(Vec2::new(available_width, height), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, RADIUS_CARD, PALETTE_PREVIEW_BG);
        painter.rect_stroke(
            rect,
            RADIUS_CARD,
            Stroke::new(1.0, PALETTE_BORDER),
            StrokeKind::Inside,
        );

        if let Some(texture) = &self.preview {
            let image_size = texture.size_vec2();
            let scale = (rect.width() / image_size.x).min(rect.height() / image_size.y);
            let size = image_size * scale;
            let image_rect = Rect::from_center_size(rect.center(), size);
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                empty_message,
                FontId::proportional(16.0),
                PALETTE_MUTED,
            );
        }
    }

    pub(crate) fn draw_player_bar(&mut self, ui: &mut egui::Ui, target: PlaybackTarget) {
        let (mut position_ms, duration_ms) = match target {
            PlaybackTarget::Timeline => (
                self.editor.playhead_ms,
                self.editor.timeline_duration_ms().max(1),
            ),
            PlaybackTarget::Convert => (
                self.editor.convert_preview_ms,
                self.editor.convert_duration_ms.unwrap_or(1).max(1),
            ),
        };
        position_ms = position_ms.min(duration_ms);

        let show_seek = matches!(target, PlaybackTarget::Convert);

        let mut new_position_ms = position_ms;
        let mut seek_changed = false;
        let mut volume = self.playback.volume();
        let mut volume_changed = false;
        let mut speed_changed: Option<f32> = None;
        let mut mute_clicked = false;
        let mut play_clicked = false;
        let mut stop_clicked = false;

        egui::Frame::new()
            .fill(PALETTE_SURFACE_RAISED)
            .stroke(Stroke::new(1.0, PALETTE_BORDER_SOFT))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let active = self.playback.is_playing() && self.playback.target() == target;
                        let toggle_icon = if active {
                            PlayerIcon::Pause
                        } else {
                            PlayerIcon::Play
                        };
                        if icon_button(ui, toggle_icon, 36.0)
                            .on_hover_text(if active {
                                "Pause (Space)"
                            } else {
                                "Play (Space)"
                            })
                            .clicked()
                        {
                            play_clicked = true;
                        }
                        if icon_button(ui, PlayerIcon::Stop, 36.0)
                            .on_hover_text("Stop")
                            .clicked()
                        {
                            stop_clicked = true;
                        }

                        ui.add_space(10.0);

                        if show_seek {
                            ui.label(
                                RichText::new(format_time(position_ms))
                                    .color(PALETTE_TEXT)
                                    .monospace(),
                            );
                            // Reserve room for the right-hand speed/volume cluster.
                            let reserved = 300.0;
                            let slider_width =
                                (ui.available_width() - reserved).clamp(80.0, 1_400.0);
                            seek_changed = ui
                                .add_sized(
                                    [slider_width, 32.0],
                                    egui::Slider::new(&mut new_position_ms, 0..=duration_ms)
                                        .show_value(false),
                                )
                                .changed();
                            ui.label(
                                RichText::new(format_time(duration_ms))
                                    .color(PALETTE_MUTED)
                                    .monospace(),
                            );
                        } else {
                            ui.label(
                                RichText::new(format!(
                                    "{} / {}",
                                    format_time(position_ms),
                                    format_time(duration_ms)
                                ))
                                .color(PALETTE_TEXT)
                                .monospace(),
                            );
                        }

                        // Speed and volume sit on the right; scrubbing happens on
                        // the timeline ruler so the transport stays uncluttered.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:>3}%", (volume * 100.0).round() as i32))
                                    .size(12.5)
                                    .color(PALETTE_TEXT)
                                    .monospace(),
                            );
                            volume_changed = ui
                                .add_sized(
                                    [100.0, 32.0],
                                    egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false),
                                )
                                .changed();
                            let muted = self.playback.is_muted();
                            let volume_icon = if muted {
                                PlayerIcon::VolumeOff
                            } else {
                                PlayerIcon::VolumeOn
                            };
                            if icon_button(ui, volume_icon, 32.0)
                                .on_hover_text(if muted { "Unmute" } else { "Mute" })
                                .clicked()
                            {
                                mute_clicked = true;
                            }
                            ui.add_space(8.0);
                            let current_speed = self.playback.speed();
                            egui::ComboBox::from_id_salt((target, "speed"))
                                .width(64.0)
                                .selected_text(format!("{current_speed:.2}x"))
                                .show_ui(ui, |ui| {
                                    for &speed in SPEEDS {
                                        if ui
                                            .selectable_label(
                                                (current_speed - speed).abs() < f32::EPSILON,
                                                format!("{speed:.2}x"),
                                            )
                                            .clicked()
                                        {
                                            speed_changed = Some(speed);
                                        }
                                    }
                                });
                        });
                    },
                );
            });

        if play_clicked {
            self.toggle_playback(target);
        }
        if stop_clicked {
            self.stop_playback();
        }
        if mute_clicked {
            self.playback.set_muted(!self.playback.is_muted());
        }
        if let Some(speed) = speed_changed {
            self.set_playback_speed(target, speed);
        }
        if volume_changed {
            self.playback.set_volume(volume);
        }
        if seek_changed {
            let commit = !ui.input(|input| input.pointer.any_down());
            self.seek_to(target, new_position_ms, commit);
        }
    }
}
