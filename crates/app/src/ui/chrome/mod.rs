//! The frame around the list: sidebar (brand, filters, speed limit), header (add links, search)
//! and the dashboard (current speed, last minute's chart).

mod dashboard;
mod sidebar;

use eframe::egui::{
    Align, Align2, Color32, Context, Frame, Id, Image, Layout, Margin, Panel, Rect, RichText, Sense, Stroke, StrokeKind,
    Ui, UiBuilder, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;

use super::{
    ADD_ID, Action, App, Filter, NAV, SEARCH_ID, STATUS_FILTERS,
    theme::{self, Palette},
    widgets::{self, ICON_BUTTON, accent_button, accent_button_width, caption, field, icon_button},
};
use crate::{
    i18n::{duration, speed},
    manager::Stats,
    settings::Theme,
    tr, trf, update,
};

const HEADER_GAP: f32 = 10.0;
const SEARCH_WIDTH: f32 = 240.0;
const MIN_ADD_WIDTH: f32 = 200.0;

impl App<'_> {
    pub(super) fn header(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        Panel::top("header")
            .exact_size(74.0)
            .frame(Frame::new().inner_margin(Margin { left: 26, right: 22, top: 18, bottom: 14 }))
            .show_separator_line(false)
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    let gap = HEADER_GAP;
                    ui.spacing_mut().item_spacing.x = gap;
                    let download = tr!("Télécharger", "Download");
                    let search_block = SEARCH_WIDTH + 2.0 * (gap + ICON_BUTTON);
                    let button = accent_button_width(ui, icon::DOWNLOAD_SIMPLE, download);
                    let add_width = (ui.available_width() - button - search_block - 2.0 * gap).max(MIN_ADD_WIDTH);
                    let add = field(ui, Id::new(ADD_ID), &mut self.memo.url, tr!("Collez un ou plusieurs liens à télécharger…", "Paste one or more links to download…"), icon::LINK, add_width);
                    let enter = add.lost_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter));
                    if (accent_button(ui, icon::DOWNLOAD_SIMPLE, download).clicked() || enter) && !self.memo.url.trim().is_empty() {
                        let text = std::mem::take(&mut self.memo.url);
                        if self.submit(&text) == 0 {
                            self.memo.url = text;
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, icon::GEAR_SIX, tr!("Paramètres", "Settings"), None).clicked() {
                            self.open_settings();
                        }
                        let dark = ui.visuals().dark_mode;
                        let (glyph, tip, next) =
                            if dark { (icon::SUN, tr!("Thème clair", "Light theme"), Theme::Light) } else { (icon::MOON, tr!("Thème sombre", "Dark theme"), Theme::Dark) };
                        if icon_button(ui, glyph, tip, None).clicked() {
                            self.set_theme(&ctx, next);
                        }
                        let hint = widgets::shortcut(tr!("Rechercher  (Ctrl+F)", "Search  (Ctrl+F)"));
                        field(ui, Id::new(SEARCH_ID), &mut self.memo.search, &hint, icon::MAGNIFYING_GLASS, SEARCH_WIDTH);
                    });
                });
            });
    }

    pub(super) fn set_theme(&mut self, ctx: &Context, theme: Theme) {
        let mut settings = self.manager.settings();
        settings.theme = theme;
        ctx.set_theme(super::preference(theme));
        if let Some(draft) = &mut self.settings {
            draft.theme = theme;
        }
        self.manager.apply_settings(settings);
        self.manager.save_settings();
    }
}
