#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod browser;
#[cfg(target_os = "linux")]
mod desktop;
mod dialogs;
mod icons;
mod map;
mod search;
mod settings;
mod system;
mod text;

use std::path::PathBuf;

use eframe::egui;
use time::UtcOffset;

pub const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

fn main() -> eframe::Result {
    #[cfg(all(target_os = "linux", not(debug_assertions)))]
    desktop::register_unless_opted_out(ICON_PNG);

    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let dropped_on_launch: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let icon = eframe::icon_data::from_png_bytes(ICON_PNG).expect("assets/icon.png is a valid PNG");
    let logo =
        egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("GeoZip")
            .with_app_id("geozip")
            .with_icon(icon)
            .with_inner_size([1040.0, 680.0])
            .with_min_inner_size([820.0, 520.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "GeoZip",
        options,
        Box::new(move |cc| {
            walkers::install_renderer(cc.wgpu_render_state.as_ref());
            cc.egui_ctx.set_theme(egui::Theme::Light);
            Ok(Box::new(app::App::new(cc, dropped_on_launch, local_offset, logo)))
        }),
    )
}
