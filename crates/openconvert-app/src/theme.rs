use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

pub const PALETTE_BG: Color32 = Color32::from_rgb(11, 15, 22);
pub const PALETTE_SURFACE: Color32 = Color32::from_rgb(18, 25, 35);
pub const PALETTE_SURFACE_RAISED: Color32 = Color32::from_rgb(24, 33, 45);
pub const PALETTE_PREVIEW_BG: Color32 = Color32::from_rgb(7, 10, 16);
pub const PALETTE_TIMELINE_BG: Color32 = Color32::from_rgb(13, 19, 28);
pub const PALETTE_RULER_BG: Color32 = Color32::from_rgb(16, 23, 33);
pub const PALETTE_GRID: Color32 = Color32::from_rgb(40, 53, 70);
pub const PALETTE_TRACK_A: Color32 = Color32::from_rgb(28, 40, 55);
pub const PALETTE_TRACK_B: Color32 = Color32::from_rgb(22, 33, 46);
pub const PALETTE_TRACK_SELECTED: Color32 = Color32::from_rgb(38, 58, 80);
pub const PALETTE_BORDER: Color32 = Color32::from_rgb(46, 61, 80);
pub const PALETTE_BORDER_SOFT: Color32 = Color32::from_rgb(34, 46, 62);
pub const PALETTE_BUTTON: Color32 = Color32::from_rgb(36, 47, 62);
pub const PALETTE_BUTTON_HOVER: Color32 = Color32::from_rgb(54, 70, 90);
pub const PALETTE_ACCENT: Color32 = Color32::from_rgb(69, 143, 255);
pub const PALETTE_CLIP: Color32 = Color32::from_rgb(45, 117, 199);
pub const PALETTE_CLIP_SELECTED: Color32 = Color32::from_rgb(65, 151, 255);
pub const PALETTE_CLIP_MUTED: Color32 = Color32::from_rgb(83, 95, 112);
pub const PALETTE_CLIP_STROKE: Color32 = Color32::from_rgb(169, 218, 255);
pub const PALETTE_PLAYHEAD: Color32 = Color32::from_rgb(255, 92, 92);
pub const PALETTE_TEXT: Color32 = Color32::from_rgb(241, 246, 251);
pub const PALETTE_MUTED: Color32 = Color32::from_rgb(160, 170, 185);

pub const RADIUS_CARD: f32 = 14.0;
pub const RADIUS_BUTTON: f32 = 10.0;

pub fn configure_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(10.0, 10.0);
        style.spacing.button_padding = Vec2::new(14.0, 8.0);
        style.spacing.interact_size = Vec2::new(60.0, 32.0);

        style
            .text_styles
            .insert(egui::TextStyle::Heading, FontId::proportional(22.0));
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(14.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(13.5));

        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = PALETTE_BG;
        style.visuals.window_fill = PALETTE_SURFACE;
        style.visuals.extreme_bg_color = PALETTE_BG;
        style.visuals.faint_bg_color = PALETTE_SURFACE;
        style.visuals.override_text_color = Some(PALETTE_TEXT);

        style.visuals.widgets.noninteractive.bg_fill = PALETTE_SURFACE;
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, PALETTE_TEXT);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, PALETTE_BORDER_SOFT);

        style.visuals.widgets.inactive.bg_fill = PALETTE_BUTTON;
        style.visuals.widgets.inactive.weak_bg_fill = PALETTE_BUTTON;
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, PALETTE_TEXT);
        style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;

        style.visuals.widgets.hovered.bg_fill = PALETTE_BUTTON_HOVER;
        style.visuals.widgets.hovered.weak_bg_fill = PALETTE_BUTTON_HOVER;
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, PALETTE_TEXT);
        style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;

        style.visuals.widgets.active.bg_fill = PALETTE_ACCENT;
        style.visuals.widgets.active.weak_bg_fill = PALETTE_ACCENT;
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);

        style.visuals.selection.bg_fill = PALETTE_ACCENT;
        style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    });
}

pub fn button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(14.0)).corner_radius(RADIUS_BUTTON)
}

pub fn action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized([130.0, 32.0], button(text))
}

pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_sized([150.0, 36.0], button(text).fill(PALETTE_ACCENT))
}

/// Transport glyphs drawn as crisp vector shapes. Icon fonts mis-center these
/// symbols (the play triangle and speaker drift off the optical center), so the
/// player controls paint them directly and stay perfectly aligned at any size.
#[derive(Clone, Copy)]
pub enum PlayerIcon {
    Play,
    Pause,
    Previous,
    Next,
    Stop,
    VolumeOn,
    VolumeOff,
}

/// A square control that paints `icon` centered in a rounded background and
/// reacts to hover/press like a regular button.
pub fn icon_button(ui: &mut egui::Ui, icon: PlayerIcon, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let (bg, fg) = {
        let visuals = ui.style().interact(&response);
        (visuals.weak_bg_fill, visuals.fg_stroke.color)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS_BUTTON, bg);
    paint_player_icon(painter, rect.center(), size, icon, fg);
    response
}

fn paint_player_icon(
    painter: &egui::Painter,
    center: Pos2,
    size: f32,
    icon: PlayerIcon,
    color: Color32,
) {
    let half = size * 0.22;
    match icon {
        PlayerIcon::Play => {
            let width = half * 1.7;
            // Place the base so the triangle's centroid lands on `center.x`.
            let left = center.x - width / 3.0;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(left, center.y - half),
                    Pos2::new(left, center.y + half),
                    Pos2::new(left + width, center.y),
                ],
                color,
                Stroke::NONE,
            ));
        }
        PlayerIcon::Pause => {
            let bar = size * 0.12;
            let gap = size * 0.12;
            for sign in [-1.0_f32, 1.0] {
                let cx = center.x + sign * (gap + bar) * 0.5;
                painter.rect_filled(
                    Rect::from_center_size(Pos2::new(cx, center.y), Vec2::new(bar, half * 2.0)),
                    1.0,
                    color,
                );
            }
        }
        PlayerIcon::Stop => {
            painter.rect_filled(
                Rect::from_center_size(center, Vec2::splat(size * 0.4)),
                2.0,
                color,
            );
        }
        PlayerIcon::Previous | PlayerIcon::Next => {
            let direction = if matches!(icon, PlayerIcon::Previous) {
                -1.0
            } else {
                1.0
            };
            let triangle_half = size * 0.13;
            let triangle_width = size * 0.22;
            let center_x = center.x + direction * size * 0.04;
            let point_x = center_x + direction * triangle_width * 0.5;
            let base_x = center_x - direction * triangle_width * 0.5;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(point_x, center.y),
                    Pos2::new(base_x, center.y - triangle_half),
                    Pos2::new(base_x, center.y + triangle_half),
                ],
                color,
                Stroke::NONE,
            ));
            let bar_x = center.x - direction * size * 0.18;
            painter.line_segment(
                [
                    Pos2::new(bar_x, center.y - triangle_half),
                    Pos2::new(bar_x, center.y + triangle_half),
                ],
                Stroke::new(size * 0.06, color),
            );
        }
        PlayerIcon::VolumeOn | PlayerIcon::VolumeOff => {
            paint_speaker(
                painter,
                center,
                size,
                color,
                matches!(icon, PlayerIcon::VolumeOn),
            );
        }
    }
}

fn paint_speaker(painter: &egui::Painter, center: Pos2, size: f32, color: Color32, on: bool) {
    let base_half = size * 0.10;
    let mouth_half = size * 0.20;
    let base_left = center.x - size * 0.28;
    let base_right = center.x - size * 0.06;
    let mouth_x = center.x + size * 0.06;
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(base_left, center.y - base_half),
            Pos2::new(base_right, center.y + base_half),
        ),
        0.0,
        color,
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(base_right, center.y - base_half),
            Pos2::new(mouth_x, center.y - mouth_half),
            Pos2::new(mouth_x, center.y + mouth_half),
            Pos2::new(base_right, center.y + base_half),
        ],
        color,
        Stroke::NONE,
    ));
    let stroke = Stroke::new(size * 0.05, color);
    if on {
        for (offset, reach) in [(0.14, 0.08), (0.22, 0.13)] {
            let wx = center.x + size * offset;
            let span = size * reach;
            painter.line_segment(
                [
                    Pos2::new(wx, center.y - span),
                    Pos2::new(wx + span, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(wx + span, center.y),
                    Pos2::new(wx, center.y + span),
                ],
                stroke,
            );
        }
    } else {
        let mx = center.x + size * 0.18;
        let r = size * 0.09;
        painter.line_segment(
            [
                Pos2::new(mx - r, center.y - r),
                Pos2::new(mx + r, center.y + r),
            ],
            stroke,
        );
        painter.line_segment(
            [
                Pos2::new(mx - r, center.y + r),
                Pos2::new(mx + r, center.y - r),
            ],
            stroke,
        );
    }
}

pub fn chip(ui: &mut egui::Ui, selected: bool, text: impl Into<String>) -> egui::Response {
    let fill = if selected {
        PALETTE_ACCENT
    } else {
        PALETTE_BUTTON
    };
    ui.add(
        egui::Button::new(RichText::new(text.into()).size(13.5))
            .fill(fill)
            .corner_radius(RADIUS_BUTTON),
    )
}

pub fn tab_chip(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let fill = if selected {
        PALETTE_ACCENT
    } else {
        Color32::TRANSPARENT
    };
    ui.add_sized(
        [88.0, 32.0],
        egui::Button::new(RichText::new(text).size(14.5))
            .fill(fill)
            .corner_radius(RADIUS_BUTTON),
    )
}

pub fn format_time(milliseconds: u64) -> String {
    let total_seconds = milliseconds / 1_000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;

    format!("{minutes}:{seconds:02}")
}
