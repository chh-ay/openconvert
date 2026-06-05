use eframe::egui::{self, RichText};

use crate::app::OpenConvertApp;
use crate::player::PlaybackTarget;
use crate::theme::PALETTE_MUTED;

impl OpenConvertApp {
    pub(crate) fn draw_convert_view(&mut self, ui: &mut egui::Ui) {
        self.draw_export_bar(ui);
        ui.add_space(12.0);
        self.draw_preview_frame(ui, "Choose a file to preview frames");
        ui.add_space(8.0);
        self.draw_player_bar(ui, PlaybackTarget::Convert);
        ui.add_space(8.0);

        if let Some(duration_ms) = self.editor.convert_duration_ms {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "Detected duration · {}.{:03}s",
                        duration_ms / 1_000,
                        duration_ms % 1_000
                    ))
                    .size(13.0)
                    .color(PALETTE_MUTED),
                );
            });
        }
    }
}
