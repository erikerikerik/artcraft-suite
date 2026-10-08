#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod model;
mod services;

use eframe::egui;

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("ArtCraft Suite")
        .with_inner_size([1040.0, 720.0])
        .with_min_inner_size([880.0, 600.0]);

    eframe::run_native(
        "ArtCraft Suite",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(app::ArtCraftSuite::new(cc)))),
    )
}
