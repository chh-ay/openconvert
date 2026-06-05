//! Keyboard shortcuts modelled on DaVinci Resolve's edit page, plus a help
//! overlay that documents them.
//!
//! All chorded shortcuts use egui's [`Modifiers::COMMAND`], which resolves to ⌘
//! on macOS and Ctrl elsewhere, so one table covers both platforms.

use eframe::egui::{self, Align, Id, Key, Layout, Modifiers, RichText, Stroke};

use crate::app::OpenConvertApp;
use crate::messages::Panel;
use crate::player::PlaybackTarget;
use crate::theme::{
    self, PALETTE_BORDER_SOFT, PALETTE_MUTED, PALETTE_SURFACE_RAISED, PALETTE_TEXT, RADIUS_CARD,
};
use crate::timeline_geo::ZOOM_STEP;

/// Fine playhead step for the arrow keys, in milliseconds.
const FINE_STEP_MS: i64 = 100;
/// Coarse playhead step for shifted arrow keys, in milliseconds.
const COARSE_STEP_MS: i64 = 1_000;

/// A resolved keyboard action, decoupled from how egui reports the key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shortcut {
    PlayPause,
    ToggleHelp,
    OpenExport,
    Save,
    Undo,
    Redo,
    Split,
    Delete,
    TrimIn,
    TrimOut,
    Mute,
    StepBack,
    StepForward,
    StepBackFar,
    StepForwardFar,
    PreviousClip,
    NextClip,
    GoToStart,
    GoToEnd,
    ZoomIn,
    ZoomOut,
    Fit,
}

/// Drains the frame's key events into the shortcuts they trigger.
///
/// [`egui::InputState::consume_key`] ignores *extra* Shift/Alt, so the most
/// specific modifier combination for a given key is checked first (e.g.
/// `Cmd+Shift+Z` before `Cmd+Z`, `Shift+←` before `←`).
fn collect_shortcuts(ctx: &egui::Context) -> Vec<Shortcut> {
    let cmd = Modifiers::COMMAND;
    let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    let none = Modifiers::NONE;
    let shift = Modifiers::SHIFT;

    ctx.input_mut(|input| {
        let mut hit = Vec::new();
        let mut take = |fired: bool, shortcut: Shortcut| {
            if fired {
                hit.push(shortcut);
            }
        };

        // Chorded first, then shifted-vs-bare pairs in specific→bare order.
        take(
            input.consume_key(cmd_shift, Key::Z) || input.consume_key(cmd, Key::Y),
            Shortcut::Redo,
        );
        take(input.consume_key(cmd, Key::Z), Shortcut::Undo);
        take(input.consume_key(cmd, Key::S), Shortcut::Save);
        take(input.consume_key(cmd, Key::E), Shortcut::OpenExport);
        take(input.consume_key(cmd, Key::B), Shortcut::Split);

        take(
            input.consume_key(shift, Key::ArrowLeft),
            Shortcut::StepBackFar,
        );
        take(
            input.consume_key(shift, Key::ArrowRight),
            Shortcut::StepForwardFar,
        );
        take(input.consume_key(none, Key::ArrowLeft), Shortcut::StepBack);
        take(
            input.consume_key(none, Key::ArrowRight),
            Shortcut::StepForward,
        );
        take(
            input.consume_key(shift, Key::Z) || input.consume_key(none, Key::F),
            Shortcut::Fit,
        );

        take(input.consume_key(none, Key::Space), Shortcut::PlayPause);
        take(
            input.consume_key(none, Key::Questionmark),
            Shortcut::ToggleHelp,
        );
        take(
            input.consume_key(none, Key::Backspace) || input.consume_key(none, Key::Delete),
            Shortcut::Delete,
        );
        take(input.consume_key(none, Key::I), Shortcut::TrimIn);
        take(input.consume_key(none, Key::O), Shortcut::TrimOut);
        take(input.consume_key(none, Key::M), Shortcut::Mute);
        take(
            input.consume_key(none, Key::ArrowUp),
            Shortcut::PreviousClip,
        );
        take(input.consume_key(none, Key::ArrowDown), Shortcut::NextClip);
        take(input.consume_key(none, Key::Home), Shortcut::GoToStart);
        take(input.consume_key(none, Key::End), Shortcut::GoToEnd);
        take(
            input.consume_key(none, Key::Plus) || input.consume_key(none, Key::Equals),
            Shortcut::ZoomIn,
        );
        take(input.consume_key(none, Key::Minus), Shortcut::ZoomOut);

        hit
    })
}

impl OpenConvertApp {
    pub(crate) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // A modal owns the keyboard while open; it closes itself on Esc.
        if self.export_dialog_open {
            return;
        }
        // Never steal keys from a focused widget such as a text field.
        if ctx.memory(|memory| memory.focused().is_some()) {
            return;
        }

        let shortcuts = collect_shortcuts(ctx);
        if shortcuts.is_empty() {
            return;
        }

        // While the help overlay is up, only its own toggle is live.
        if self.show_shortcuts {
            if shortcuts.contains(&Shortcut::ToggleHelp) {
                self.show_shortcuts = false;
            }
            return;
        }

        let edit = matches!(self.panel, Panel::Edit);
        for shortcut in shortcuts {
            match shortcut {
                Shortcut::PlayPause => {
                    let target = match self.panel {
                        Panel::Edit => PlaybackTarget::Timeline,
                        Panel::Convert => PlaybackTarget::Convert,
                    };
                    self.toggle_playback(target);
                }
                Shortcut::ToggleHelp => self.show_shortcuts = true,
                Shortcut::OpenExport => self.export_dialog_open = true,
                // Everything below acts on the timeline; ignore it in Convert.
                _ if !edit => {}
                Shortcut::Save => self.save_project(),
                Shortcut::Undo => self.undo(),
                Shortcut::Redo => self.redo(),
                Shortcut::Split => self.split_at_playhead(),
                Shortcut::Delete => self.delete_selected_clip(),
                Shortcut::TrimIn => self.trim_start_to_playhead(),
                Shortcut::TrimOut => self.trim_end_to_playhead(),
                Shortcut::Mute => self.toggle_selected_clip_mute(),
                Shortcut::StepBack => self.step_playhead(-FINE_STEP_MS),
                Shortcut::StepForward => self.step_playhead(FINE_STEP_MS),
                Shortcut::StepBackFar => self.step_playhead(-COARSE_STEP_MS),
                Shortcut::StepForwardFar => self.step_playhead(COARSE_STEP_MS),
                Shortcut::PreviousClip => {
                    self.editor.select_previous_clip();
                    self.request_preview_at_playhead(false);
                }
                Shortcut::NextClip => {
                    self.editor.select_next_clip();
                    self.request_preview_at_playhead(false);
                }
                Shortcut::GoToStart => self.seek_to(PlaybackTarget::Timeline, 0, true),
                Shortcut::GoToEnd => {
                    let end = self.editor.timeline_duration_ms();
                    self.seek_to(PlaybackTarget::Timeline, end, true);
                }
                Shortcut::ZoomIn => self.zoom_timeline(ZOOM_STEP),
                Shortcut::ZoomOut => self.zoom_timeline(1.0 / ZOOM_STEP),
                Shortcut::Fit => self.editor.request_fit(),
            }
        }
    }

    /// Nudges the playhead and refreshes the still preview, matching the
    /// timeline toolbar's step buttons.
    fn step_playhead(&mut self, delta_ms: i64) {
        self.editor.nudge_playhead(delta_ms);
        self.request_preview_at_playhead(false);
    }

    pub(crate) fn draw_shortcuts_overlay(&mut self, ctx: &egui::Context) {
        let t = ctx.animate_bool_with_time(Id::new("oc_help_fade"), self.show_shortcuts, 0.12);
        if t <= 0.001 {
            return;
        }

        let modifier = command_label(ctx);
        let mut close = false;

        let modal = egui::Modal::new(Id::new("oc_help_modal"))
            .frame(
                egui::Frame::new()
                    .fill(PALETTE_SURFACE_RAISED)
                    .stroke(Stroke::new(1.0, PALETTE_BORDER_SOFT))
                    .corner_radius(RADIUS_CARD)
                    .inner_margin(egui::Margin::same(22)),
            )
            .show(ctx, |ui| {
                ui.set_opacity(t);
                ui.set_width(420.0);

                ui.label(
                    RichText::new("Keyboard shortcuts")
                        .size(20.0)
                        .strong()
                        .color(PALETTE_TEXT),
                );
                ui.add_space(12.0);

                egui::ScrollArea::vertical()
                    .max_height(440.0)
                    .show(ui, |ui| {
                        for (section, rows) in SHORTCUT_HELP {
                            ui.label(
                                RichText::new(*section)
                                    .size(12.5)
                                    .strong()
                                    .color(PALETTE_MUTED),
                            );
                            ui.add_space(4.0);
                            for (keys, description) in *rows {
                                shortcut_row(ui, &keys.replace("Mod", modifier), description);
                            }
                            ui.add_space(12.0);
                        }
                    });

                ui.separator();
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(theme::button("Close").min_size(egui::vec2(96.0, 34.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });

        if self.show_shortcuts && (close || modal.should_close()) {
            self.show_shortcuts = false;
        }
    }
}

/// macOS shows ⌘; every other platform shows Ctrl. Mirrors egui's `COMMAND`.
fn command_label(ctx: &egui::Context) -> &'static str {
    if matches!(ctx.os(), egui::os::OperatingSystem::Mac) {
        "⌘"
    } else {
        "Ctrl"
    }
}

fn shortcut_row(ui: &mut egui::Ui, keys: &str, description: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(keys)
                .monospace()
                .size(12.5)
                .color(PALETTE_TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(description).size(12.5).color(PALETTE_MUTED));
        });
    });
}

type HelpSection = (&'static str, &'static [(&'static str, &'static str)]);

/// Living documentation for the bindings in [`collect_shortcuts`]. "Mod" is
/// substituted with the platform command key when shown.
const SHORTCUT_HELP: &[HelpSection] = &[
    (
        "Playback",
        &[
            ("Space", "Play / pause"),
            ("← / →", "Step playhead"),
            ("Shift + ← / →", "Step one second"),
            ("↑ / ↓", "Previous / next clip"),
            ("Home / End", "Jump to start / end"),
        ],
    ),
    (
        "Editing",
        &[
            ("Mod + B", "Split at playhead"),
            ("Backspace / Del", "Delete selected clip"),
            ("I / O", "Trim start / end to playhead"),
            ("M", "Mute / unmute clip"),
        ],
    ),
    (
        "Timeline",
        &[
            ("+ / -", "Zoom in / out"),
            ("F · Shift + Z", "Fit to view"),
            ("Mod + Z", "Undo"),
            ("Mod + Shift + Z", "Redo"),
        ],
    ),
    (
        "Project",
        &[
            ("Mod + S", "Save project"),
            ("Mod + E", "Export / convert"),
            ("?", "Toggle this help"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_help_documents_the_resolve_style_split_binding() {
        assert!(SHORTCUT_HELP
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .any(|(keys, description)| *keys == "Mod + B" && *description == "Split at playhead"));
    }
}
