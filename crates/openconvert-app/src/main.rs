use anyhow::Result;
use eframe::egui;

mod app;
mod editor;
mod messages;
mod native_dialog;
mod player;
mod preview;
mod preview_player;
mod theme;
mod timeline_geo;
mod ui;

use crate::app::OpenConvertApp;

fn main() -> Result<()> {
    configure_linux_desktop();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("OpenConvert")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([960.0, 640.0])
            .with_resizable(true)
            .with_decorations(true),
        ..Default::default()
    };

    eframe::run_native(
        "OpenConvert",
        native_options,
        Box::new(|cc| Ok(Box::new(OpenConvertApp::new(cc)))),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    Ok(())
}

fn configure_linux_desktop() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
            if std::env::var_os("WINIT_UNIX_BACKEND").is_none() {
                unsafe {
                    std::env::set_var("WINIT_UNIX_BACKEND", "wayland");
                }
            }
            if std::env::var_os("GDK_BACKEND").is_none() {
                unsafe {
                    std::env::set_var("GDK_BACKEND", "wayland,x11");
                }
            }
        }
    }
}
