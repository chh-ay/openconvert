use eframe::egui;

use crate::app::OpenConvertApp;
use crate::player::PlaybackTarget;

impl OpenConvertApp {
    pub(crate) fn draw_edit_view(&mut self, ui: &mut egui::Ui) {
        self.draw_preview_frame(ui, "Import media to preview frames");
        ui.add_space(8.0);
        self.draw_player_bar(ui, PlaybackTarget::Timeline);
        ui.add_space(12.0);
        self.draw_timeline_toolbar(ui);
        ui.add_space(6.0);
        self.draw_timeline(ui);
    }
}
