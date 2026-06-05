use eframe::egui::{self, Color32, RichText, Stroke};
use openconvert_media::{output_video_size, CompressionPreset, Container, VideoCodec};

use crate::app::OpenConvertApp;
use crate::messages::Panel;
use crate::theme::{
    self, chip, PALETTE_ACCENT, PALETTE_BORDER_SOFT, PALETTE_MUTED, PALETTE_SURFACE_RAISED,
    PALETTE_TEXT, RADIUS_BUTTON, RADIUS_CARD,
};

const CONTAINERS: &[Container] = &[
    Container::Mp4,
    Container::Mkv,
    Container::WebM,
    Container::Mov,
    Container::Mp3,
];
const VIDEO_CODECS: &[VideoCodec] = &[VideoCodec::H264, VideoCodec::H265];
const PRESETS: &[CompressionPreset] = &[
    CompressionPreset::HighQuality,
    CompressionPreset::Balanced,
    CompressionPreset::Small,
];

impl OpenConvertApp {
    /// Modal export/convert settings, opened from the top bar's primary button so
    /// the editing surface keeps its space instead of carrying an always-on
    /// settings strip. The same dialog drives timeline export and single-file
    /// conversion; the active panel selects which.
    pub(crate) fn draw_export_dialog(&mut self, ctx: &egui::Context) {
        let open = self.export_dialog_open;
        // Drive visibility from the animation so the dialog fades both in and out.
        let t = ctx.animate_bool_with_time(egui::Id::new("oc_export_fade"), open, 0.12);
        if t <= 0.001 {
            return;
        }

        let is_convert = matches!(self.panel, Panel::Convert);
        let can_run = if is_convert {
            self.editor.convert_input.is_some()
        } else {
            self.editor.timeline_duration_ms() > 0
        };

        let mut run = false;
        let mut cancel = false;

        let modal = egui::Modal::new(egui::Id::new("oc_export_modal"))
            .frame(
                egui::Frame::new()
                    .fill(PALETTE_SURFACE_RAISED)
                    .stroke(Stroke::new(1.0, PALETTE_BORDER_SOFT))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                ui.set_opacity(t);
                ui.set_width(460.0);

                ui.label(
                    RichText::new(if is_convert { "Convert" } else { "Export" })
                        .size(20.0)
                        .strong()
                        .color(PALETTE_TEXT),
                );
                ui.label(
                    RichText::new(if is_convert {
                        "Transcode the chosen file"
                    } else {
                        "Render the timeline to a single file"
                    })
                    .size(12.5)
                    .color(PALETTE_MUTED),
                );
                ui.add_space(16.0);

                self.export_dialog_body(ui, is_convert);

                ui.add_space(18.0);
                ui.separator();
                ui.add_space(12.0);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let run_label = if is_convert { "Convert" } else { "Export" };
                    let run_button = egui::Button::new(
                        RichText::new(run_label).size(14.0).color(Color32::WHITE),
                    )
                    .fill(PALETTE_ACCENT)
                    .corner_radius(RADIUS_BUTTON)
                    .min_size(egui::vec2(124.0, 36.0));
                    let response = ui.add_enabled(can_run, run_button);
                    if response.clicked() {
                        run = true;
                    }
                    response.on_disabled_hover_text(if is_convert {
                        "Choose a file to convert first"
                    } else {
                        "Add a clip to the timeline first"
                    });
                    if ui
                        .add(theme::button("Cancel").min_size(egui::vec2(96.0, 36.0)))
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });

        // While fading out (already closed) the dialog is inert: ignore input.
        if !open {
            return;
        }
        if run {
            self.export_dialog_open = false;
            if is_convert {
                self.convert_input_file();
            } else {
                self.export_media();
            }
        } else if cancel || modal.should_close() {
            self.export_dialog_open = false;
        }
    }

    fn export_dialog_body(&mut self, ui: &mut egui::Ui, is_convert: bool) {
        let container = self.editor.export_options.container;

        section_label(ui, "Format");
        ui.horizontal_wrapped(|ui| {
            for &candidate in CONTAINERS {
                if chip(ui, container == candidate, candidate.label()).clicked() {
                    self.editor.export_options.container = candidate;
                    self.editor.export_options.audio_bitrate_kbps = self
                        .editor
                        .export_options
                        .compression
                        .default_audio_bitrate_kbps(candidate);
                }
            }
        });

        if container.has_video() {
            ui.add_space(14.0);
            section_label(ui, "Video codec");
            ui.horizontal_wrapped(|ui| {
                if container.allows_video_codec_choice() {
                    for &codec in VIDEO_CODECS {
                        if chip(
                            ui,
                            self.editor.export_options.video_codec == codec,
                            codec.label(),
                        )
                        .clicked()
                        {
                            self.editor.export_options.video_codec = codec;
                        }
                    }
                } else {
                    ui.label(RichText::new("VP9").size(13.5).color(PALETTE_MUTED));
                }
            });
        }

        ui.add_space(14.0);
        section_label(ui, "Quality");
        ui.horizontal_wrapped(|ui| {
            for &preset in PRESETS {
                if chip(
                    ui,
                    self.editor.export_options.compression == preset,
                    preset.label(),
                )
                .clicked()
                {
                    self.editor.export_options.compression = preset;
                    self.editor.export_options.video_quality = preset.default_video_quality();
                    self.editor.export_options.audio_bitrate_kbps =
                        preset.default_audio_bitrate_kbps(container);
                }
            }
        });

        if container.has_video() {
            ui.add_space(14.0);
            section_label(ui, "CRF · lower is higher quality");
            ui.add_sized(
                [ui.available_width(), 24.0],
                egui::Slider::new(&mut self.editor.export_options.video_quality, 10..=40)
                    .show_value(true),
            );
        }

        ui.add_space(14.0);
        section_label(ui, "Audio bitrate");
        ui.add_sized(
            [ui.available_width(), 24.0],
            egui::Slider::new(&mut self.editor.export_options.audio_bitrate_kbps, 64..=320)
                .show_value(true)
                .suffix(" kbps"),
        );

        ui.add_space(18.0);
        let (width, height) = if is_convert {
            (self.editor.convert_width, self.editor.convert_height)
        } else {
            output_video_size(&self.editor.project.timeline)
        };
        let duration_ms = if is_convert {
            self.editor.convert_duration_ms.unwrap_or(0)
        } else {
            self.editor.timeline_duration_ms()
        };
        detail_row(ui, "Output", &self.editor.export_options.summary());
        if container.has_video() && width > 0 && height > 0 {
            detail_row(ui, "Resolution", &format!("{width} × {height}"));
        }
        detail_row(ui, "Duration", &theme::format_time(duration_ms));
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).strong().color(PALETTE_MUTED));
    ui.add_space(2.0);
}

fn detail_row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(key).size(12.5).color(PALETTE_MUTED));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.5).color(PALETTE_TEXT));
        });
    });
}
