use eframe::egui::{self, Layout, RichText, Vec2};

use crate::app::OpenConvertApp;
use crate::messages::Panel;
use crate::theme::{
    action_button, primary_button, tab_chip, PALETTE_BORDER_SOFT, PALETTE_MUTED, PALETTE_TEXT,
};

impl OpenConvertApp {
    pub(crate) fn draw_topbar(&mut self, ui: &mut egui::Ui) {
        let previous_panel = self.panel;

        ui.allocate_ui_with_layout(
            egui::Vec2::new(ui.available_width(), 40.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new("OpenConvert")
                        .size(22.0)
                        .strong()
                        .color(PALETTE_TEXT),
                );
                ui.add_space(20.0);
                tab_button(ui, "Edit", &mut self.panel, Panel::Edit);
                tab_button(ui, "Convert", &mut self.panel, Panel::Convert);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, is_convert) = match self.panel {
                        Panel::Edit => ("Export ▶", false),
                        Panel::Convert => ("Convert ▶", true),
                    };
                    if primary_button(ui, label).clicked() {
                        if is_convert {
                            self.convert_input_file();
                        } else {
                            self.export_media();
                        }
                    }
                });
            },
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.allocate_ui_with_layout(
            egui::Vec2::new(ui.available_width(), 32.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| match self.panel {
                Panel::Edit => self.draw_edit_action_row(ui),
                Panel::Convert => self.draw_convert_action_row(ui),
            },
        );

        if self.panel != previous_panel {
            self.on_panel_changed();
        }
    }

    fn draw_edit_action_row(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new(&self.editor.project.name)
                .size(13.5)
                .color(PALETTE_MUTED),
        );
        ui.add_space(12.0);
        if action_button(ui, "New").clicked() {
            self.new_project();
        }
        if action_button(ui, "Open").clicked() {
            self.open_project();
        }
        if action_button(ui, "Save").clicked() {
            self.save_project();
        }
        ui.add(vertical_divider());
        if action_button(ui, "Import media").clicked() {
            self.import_media();
        }
        if action_button(ui, "+ Track").clicked() {
            self.add_track();
        }
    }

    fn draw_convert_action_row(&mut self, ui: &mut egui::Ui) {
        let label = self
            .editor
            .convert_input
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "No input selected".to_owned());
        ui.label(RichText::new(label).size(13.5).color(PALETTE_MUTED));

        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0);
            if action_button(ui, "Choose file").clicked() {
                self.select_convert_input();
            }
        });
    }
}

fn tab_button(ui: &mut egui::Ui, label: &str, panel: &mut Panel, target: Panel) {
    if tab_chip(ui, *panel == target, label).clicked() {
        *panel = target;
    }
}

fn vertical_divider() -> impl egui::Widget {
    move |ui: &mut egui::Ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(1.0, 24.0), egui::Sense::hover());
        ui.painter().vline(
            rect.center().x,
            rect.top()..=rect.bottom(),
            egui::Stroke::new(1.0, PALETTE_BORDER_SOFT),
        );
        response
    }
}
