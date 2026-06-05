use eframe::egui::{self, RichText, Stroke};
use openconvert_media::{CompressionPreset, Container, VideoCodec};

use crate::app::OpenConvertApp;
use crate::theme::{
    chip, PALETTE_BORDER_SOFT, PALETTE_MUTED, PALETTE_SURFACE_RAISED, PALETTE_TEXT, RADIUS_CARD,
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
    pub(crate) fn draw_export_bar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(PALETTE_SURFACE_RAISED)
            .stroke(Stroke::new(1.0, PALETTE_BORDER_SOFT))
            .corner_radius(RADIUS_CARD)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 32.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        label(ui, "Format");
                        for &container in CONTAINERS {
                            if chip(
                                ui,
                                self.editor.export_options.container == container,
                                container.label(),
                            )
                            .clicked()
                            {
                                self.editor.export_options.container = container;
                                self.editor.export_options.audio_bitrate_kbps = self
                                    .editor
                                    .export_options
                                    .compression
                                    .default_audio_bitrate_kbps(container);
                            }
                        }

                        divider(ui);

                        self.draw_codec_selector(ui);

                        label(ui, "Quality");
                        for &preset in PRESETS {
                            if chip(
                                ui,
                                self.editor.export_options.compression == preset,
                                preset.label(),
                            )
                            .clicked()
                            {
                                self.editor.export_options.compression = preset;
                                self.editor.export_options.video_quality =
                                    preset.default_video_quality();
                                self.editor.export_options.audio_bitrate_kbps = preset
                                    .default_audio_bitrate_kbps(
                                        self.editor.export_options.container,
                                    );
                            }
                        }

                        if self.editor.export_options.container.has_video() {
                            divider(ui);
                            label(ui, "CRF");
                            ui.add_sized(
                                [120.0, 32.0],
                                egui::Slider::new(
                                    &mut self.editor.export_options.video_quality,
                                    10..=40,
                                )
                                .show_value(true),
                            );
                        }

                        divider(ui);
                        label(ui, "Audio");
                        ui.add_sized(
                            [130.0, 32.0],
                            egui::Slider::new(
                                &mut self.editor.export_options.audio_bitrate_kbps,
                                64..=320,
                            )
                            .show_value(true),
                        );
                    },
                );
            });
    }

    /// Draws the video-codec picker. Containers that allow a choice (MP4/MKV/MOV)
    /// show selectable chips; WebM shows its fixed VP9 codec; MP3 is audio-only
    /// and shows nothing. A trailing divider keeps the bar layout consistent.
    fn draw_codec_selector(&mut self, ui: &mut egui::Ui) {
        let container = self.editor.export_options.container;
        if container.allows_video_codec_choice() {
            label(ui, "Codec");
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
            divider(ui);
        } else if matches!(container, Container::WebM) {
            label(ui, "Codec");
            label(ui, "VP9");
            divider(ui);
        }
    }
}

fn label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(PALETTE_MUTED));
}

fn divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(RichText::new("·").color(PALETTE_TEXT));
    ui.add_space(4.0);
}
