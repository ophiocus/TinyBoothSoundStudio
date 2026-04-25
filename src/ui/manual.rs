//! Manual viewer — non-modal floating window with a TOC and a rendered
//! markdown body. Opens via Help → Manual… or F1. Does not interfere
//! with recording, the visualizer, or any background work.

use crate::app::TinyBoothApp;
use crate::manual::{self, Category, PAGES};
use eframe::egui;
use egui_commonmark::CommonMarkViewer;

pub fn show(app: &mut TinyBoothApp, ctx: &egui::Context) {
    let mut open = app.show_manual;
    egui::Window::new("📖  Manual")
        .open(&mut open)
        .default_size([920.0, 640.0])
        .min_size([640.0, 420.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| body(app, ui));
    app.show_manual = open;
}

fn body(app: &mut TinyBoothApp, ui: &mut egui::Ui) {
    egui::SidePanel::left("manual_toc")
        .resizable(true)
        .default_width(220.0)
        .min_width(160.0)
        .show_inside(ui, |ui| {
            ui.add_space(2.0);
            ui.heading("Contents");
            ui.add_space(6.0);

            for cat in [Category::Welcome, Category::Reference, Category::Appendix] {
                ui.label(egui::RichText::new(cat.label()).strong().color(egui::Color32::from_rgb(230, 200, 80)));
                ui.add_space(2.0);
                for p in PAGES.iter().filter(|p| p.category == cat) {
                    let selected = app.manual_slug == p.slug;
                    if ui.selectable_label(selected, p.title).clicked() {
                        app.manual_slug = p.slug.to_string();
                    }
                }
                ui.add_space(10.0);
            }
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        let page = manual::find(&app.manual_slug)
            .or_else(|| PAGES.first())
            .expect("manual must contain at least one page");
        egui::ScrollArea::vertical()
            .id_source("manual_scroll")
            .show(ui, |ui| {
                CommonMarkViewer::new(format!("manual-{}", page.slug))
                    .max_image_width(Some(640))
                    .show(ui, &mut app.md_cache, page.markdown);
            });
    });
}
