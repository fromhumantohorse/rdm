//! The browser-extension window: which browsers are on this computer, which ones the extension
//! is connected from, and one-click installation into each of them.

mod steps;
mod summary;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{Align, Color32, Context, Frame, Id, Label, Layout, Margin, Modal, RichText, ScrollArea, Sense, Stroke, Ui, Vec2, vec2};
use egui_phosphor::regular as icon;

use super::{
    App, reveal_file,
    dialogs::{backdrop, dialog_frame, dialog_header},
    theme::{self, Palette},
    widgets::{self, accent_button, ghost_button},
};
use crate::{
    extension::{self, Browser, Flavour},
    manager::{Install, Installed, Manager, unix_now},
    tr, trf,
};
use steps::steps;
pub(super) use summary::{card, connected};

/// Heard from within this long: shown as connected.
const LIVE_SECS: u64 = 15 * 60;

pub(super) struct Browsers {
    /// Each browser and its executable, if found (looked up when the window opens).
    found: Vec<(Browser, Option<PathBuf>)>,
    /// The browser whose instructions are unfolded.
    open: Option<Browser>,
}

impl App<'_> {
    pub(super) fn open_browsers(&mut self) {
        // Every Chromium- or Firefox-based browser of the computer, known to RDM or not.
        let mut found: Vec<_> = self.manager.browsers().into_iter().map(|b| (b, b.find())).collect();
        // Browsers found here or already connected first (a portable browser is not "found").
        found.sort_by_key(|(b, exe)| exe.is_none() && self.manager.browser_last_seen(*b).is_none());
        self.browsers = Some(Browsers { found, open: None });
    }

    pub(super) fn browsers_dialog(&mut self, ctx: &Context) {
        let Some(mut state) = self.browsers.take() else { return };
        let p = Palette::from_ctx(ctx);
        let mut copied = None;
        let mut add = false;
        let modal = Modal::new(Id::new("browsers")).frame(dialog_frame(&p)).backdrop_color(backdrop(&p)).show(ctx, |ui| {
            ui.set_width(660.0);
            let close = dialog_header(
                ui,
                &p,
                icon::PUZZLE_PIECE,
                p.accent,
                tr!("Extension du navigateur", "Browser extension"),
                tr!("Capture les téléchargements et les vidéos des pages, et les envoie à RDM.", "Captures downloads and videos from pages, and sends them to RDM."),
            );
            ui.add_space(14.0);
            // As tall as the window allows (see the settings dialog: without the minimum, the area
            // keeps the height it had when the dialog first appeared).
            let height = (ctx.content_rect().height() - 220.0).max(260.0);
            ScrollArea::vertical().min_scrolled_height(height).max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                for (browser, exe) in &state.found {
                    self.browser_row(ui, &p, *browser, exe.as_deref(), &mut state.open, &mut copied);
                    ui.add_space(10.0);
                }
                // A browser Windows does not list (a portable one): picked by its executable, as in IDM.
                if ghost_button(ui, icon::PLUS, tr!("Ajouter un navigateur…", "Add a browser…")).clicked() {
                    add = true;
                }
                ui.add_space(8.0);
                widgets::icon_text(
                    ui,
                    icon::INFO,
                    p.faint,
                    &trf!(
                        "Extension {} · une pour tous les navigateurs basés sur Chromium (Chrome, Edge, Brave, Opera, Vivaldi…), une pour tous ceux basés sur Firefox (Waterfox, LibreWolf, Floorp, Zen…).",
                        "Extension {} · one for every Chromium-based browser (Chrome, Edge, Brave, Opera, Vivaldi…), one for every Firefox-based browser (Waterfox, LibreWolf, Floorp, Zen…).",
                        extension::version()
                    ),
                    p.faint,
                    12.0,
                );
            });
            close
        });
        if let Some(text) = copied {
            ctx.copy_text(text);
            self.toasts.info(icon::COPY, tr!("Chemin copié : collez-le avec Ctrl+V", "Path copied: paste it with Ctrl+V"));
        }
        if add {
            self.add_browser(&mut state);
        }
        if self.manager.installing() {
            self.animating = true; // spinner
        }
        if !(modal.inner || modal.should_close()) {
            self.browsers = Some(state);
        }
    }

    /// "Add a browser…": its executable, picked by the user; then listed like the others.
    fn add_browser(&mut self, state: &mut Browsers) {
        let mut dialog = rfd::FileDialog::new().set_title(tr!("Choisir le programme du navigateur", "Choose the browser's program"));
        if cfg!(windows) {
            dialog = dialog.add_filter(tr!("Programmes", "Programs"), &["exe"]);
        }
        let Some(exe) = dialog.pick_file() else { return };
        match self.manager.add_browser(exe.clone()) {
            Some(browser) => {
                state.found.retain(|(b, _)| *b != browser);
                state.found.insert(0, (browser, Some(exe)));
                state.open = None;
            }
            None => self.toasts.warn(
                icon::WARNING,
                tr!(
                    "Ce programme n'est pas un navigateur basé sur Chromium ou Firefox.",
                    "This program is not a Chromium- or Firefox-based browser."
                ),
            ),
        }
    }

    fn browser_row(
        &self,
        ui: &mut Ui,
        p: &Palette,
        browser: Browser,
        exe: Option<&Path>,
        open: &mut Option<Browser>,
        copied: &mut Option<String>,
    ) {
        let seen = self.manager.browser_last_seen(browser).map(|t| unix_now().saturating_sub(t));
        let install = self.manager.install_state(browser);
        let (status, color) = match (seen, exe) {
            (Some(ago), _) if ago < LIVE_SECS => (trf!("Connectée · dernier échange il y a {}", "Connected · last heard {} ago", ago_text(ago)), p.success),
            (Some(ago), _) => (trf!("Installée · dernier échange il y a {}", "Installed · last heard {} ago", ago_text(ago)), p.muted),
            (None, Some(_)) => (tr!("Détecté · extension pas encore installée", "Found · extension not installed yet").to_owned(), p.muted),
            (None, None) => (tr!("Non détecté sur cet ordinateur", "Not found on this computer").to_owned(), p.faint),
        };
        Frame::new()
            .fill(if p.dark { p.bg.lerp_to_gamma(p.surface, 0.55) } else { p.raised })
            .corner_radius(16)
            .inner_margin(Margin::same(14))
            .stroke(Stroke::new(theme::HAIRLINE, if seen.is_some_and(|a| a < LIVE_SECS) { p.tint(p.success, 0.45) } else { p.border }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (tile, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
                    let hue = if exe.is_some() || seen.is_some() { p.accent } else { p.faint };
                    ui.painter().add(widgets::gradient(ui, tile, 12, p.tint(hue, 0.30), p.tint(hue, 0.10), vec2(0.7, 0.7)));
                    let glyph = if matches!(browser.key(), "chrome" | "chromium") { icon::GOOGLE_CHROME_LOGO } else { icon::BROWSER };
                    ui.painter().text(tile.center(), eframe::egui::Align2::CENTER_CENTER, glyph, theme::regular(21.0), hue);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(2.0);
                        ui.label(RichText::new(browser.name()).font(theme::semibold(14.5)).color(p.text));
                        widgets::icon_text(ui, if color == p.success { icon::CHECK_CIRCLE } else { icon::CIRCLE }, color, &status, color, 12.5);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if matches!(install, Some(Install::Working)) {
                            let (r, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                            widgets::spinner(ui.painter(), r.center(), 8.0, p.accent, ui.input(|i| i.time) as f32);
                            ui.label(RichText::new(tr!("Préparation…", "Preparing…")).color(p.muted));
                        } else {
                            let label = if seen.is_some() { tr!("Réinstaller", "Reinstall") } else { tr!("Installer", "Install") };
                            let clicked = if seen.is_some() {
                                ghost_button(ui, icon::ARROW_CLOCKWISE, label).clicked()
                            } else {
                                accent_button(ui, icon::DOWNLOAD_SIMPLE, label).clicked()
                            };
                            if clicked {
                                self.manager.install_extension(browser);
                                *open = Some(browser);
                            }
                            // An icon: beside "Reinstall", a second labelled button left no room for the status line.
                            if seen.is_some()
                                && widgets::icon_button(ui, icon::TRASH, tr!("Supprimer l'extension", "Remove the extension"), Some(p.danger))
                                    .on_hover_text(tr!(
                                        "La désinstalle de ce navigateur à son prochain échange avec RDM (déjà supprimée : RDM l'oublie).",
                                        "Uninstalls it from this browser at its next exchange with RDM (already removed: RDM forgets it)."
                                    ))
                                    .clicked()
                            {
                                self.manager.remove_extension(browser);
                                if *open == Some(browser) {
                                    *open = None;
                                }
                            }
                            if install.is_some() && *open != Some(browser) && ghost_button(ui, icon::LIST_BULLETS, tr!("Étapes", "Steps")).clicked() {
                                *open = Some(browser);
                            }
                        }
                    });
                });
                if *open == Some(browser) {
                    match &install {
                        Some(Install::Done(done)) => {
                            ui.add_space(10.0);
                            steps(ui, p, &self.manager, (browser, exe), done, copied);
                        }
                        Some(Install::Failed(reason)) => {
                            ui.add_space(8.0);
                            widgets::icon_text(ui, icon::WARNING, p.danger, reason, p.danger, 13.0);
                        }
                        _ => {}
                    }
                }
            });
    }
}

/// "3 min", "2 h", "4 d".
fn ago_text(secs: u64) -> String {
    match secs {
        0..60 => tr!("moins d'une minute", "less than a minute").to_owned(),
        60..3600 => format!("{} min", secs / 60),
        3600..86_400 => format!("{} h", secs / 3600),
        _ => trf!("{} j", "{} d", secs / 86_400),
    }
}
