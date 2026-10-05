//! The sidebar: brand, filters with their counts, named queues, and the cards at its foot (speed
//! limit, extension, update).

use super::*;

impl App<'_> {
    pub(in crate::ui) fn sidebar(&mut self, root: &mut Ui, actions: &mut Vec<Action>) {
        let p = Palette::of(root);
        // One pass over the whole history per frame, whatever its length.
        let counts = self.manager.view(|es| {
            let mut counts = [0usize; NAV.len()];
            for e in es {
                for (n, (filter, _)) in counts.iter_mut().zip(NAV) {
                    *n += usize::from(filter.accepts(e));
                }
            }
            counts
        });
        let frame = Frame::new()
            .fill(p.sidebar)
            .inner_margin(Margin { left: 14, right: 14, top: 18, bottom: 16 })
            .stroke(Stroke::new(theme::HAIRLINE, p.border));
        let named = self.manager.with_settings(|s| s.queues.clone());
        // Named queues: shown once there is more than the main one.
        let queues: Vec<(Filter, usize)> = if named.is_empty() {
            Vec::new()
        } else {
            let ids: Vec<u32> = std::iter::once(0).chain(named.iter().map(|q| q.id)).collect();
            self.manager.view(|es| {
                ids.iter()
                    .map(|&q| (Filter::Queue(q), es.iter().filter(|e| Filter::Queue(q).accepts(e) && e.download.status() != &domain::Status::Completed).count()))
                    .collect()
            })
        };
        if matches!(self.memo.filter, Filter::Queue(q) if q != 0 && !named.iter().any(|x| x.id == q)) {
            self.memo.filter = Filter::All; // its queue was deleted
        }
        Panel::left("nav").exact_size(240.0).resizable(false).frame(frame).show(root, |ui| {
            self.brand(ui, &p);
            ui.add_space(24.0);
            // The filters scroll if the window is short; the cards below always stay visible.
            let update = self.manager.update_state();
            let reserved = if update.release().is_some() || update.busy() { 250.0 } else { 170.0 };
            let height = (ui.available_height() - reserved).max(120.0);
            eframe::egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                for (i, ((filter, glyph), n)) in NAV.into_iter().zip(counts).enumerate() {
                    if i == STATUS_FILTERS {
                        section_caption(ui, tr!("CATÉGORIES", "CATEGORIES"));
                    }
                    if matches!(filter, Filter::Kind(_) | Filter::Failed) && n == 0 && self.memo.filter != filter {
                        continue;
                    }
                    let hue = match filter {
                        Filter::Kind(c) => Some(p.category(c)),
                        Filter::Failed => Some(p.danger),
                        _ => None,
                    };
                    if nav_item(ui, &p, glyph, hue, &filter.title_short(&named), n, self.memo.filter == filter) {
                        self.memo.filter = filter;
                    }
                }
                if !queues.is_empty() {
                    section_caption(ui, tr!("FILES D'ATTENTE", "QUEUES"));
                    for (filter, n) in queues {
                        if nav_item(ui, &p, icon::QUEUE, None, &filter.title(&named), n, self.memo.filter == filter) {
                            self.memo.filter = filter;
                        }
                    }
                }
            });
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.label(RichText::new(concat!("RDM ", env!("CARGO_PKG_VERSION"))).font(theme::regular(11.0)).color(p.faint));
                ui.add_space(6.0);
                if self.limit_card(ui, &p).clicked() {
                    actions.push(Action::OpenSettings);
                }
                ui.add_space(2.0);
                if crate::ui::browsers::card(ui, &p, crate::ui::browsers::connected(&self.manager)) {
                    actions.push(Action::OpenBrowsers);
                }
                if let Some(action) = self.update_card(ui, &p) {
                    actions.push(action);
                }
            });
        });
    }

    fn brand(&self, ui: &mut Ui, p: &Palette) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.0), Sense::hover());
            widgets::glow(ui.painter(), rect.center() + vec2(0.0, 4.0), Vec2::splat(34.0), p.accent2.gamma_multiply(0.35));
            Image::new(&self.logo).paint_at(ui, rect);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("RDM").font(theme::bold(19.0)).color(p.text));
                ui.add_space(-6.0);
                ui.label(RichText::new("Download Manager").font(theme::regular(11.5)).color(p.muted));
            });
        });
    }

    /// A newer release is out (or being downloaded): brand-gradient card with one button.
    fn update_card(&mut self, ui: &mut Ui, p: &Palette) -> Option<Action> {
        // (title, detail, clickable, moving, tooltip)
        let (title, detail, clickable, moving, tip) = match self.manager.update_state() {
            update::State::Available(r) => {
                let detail = if update::installs_itself(&r) { tr!("Cliquer pour installer", "Click to install") } else { tr!("Voir la nouvelle version", "See the new version") };
                let version = &r.version;
                (trf!("Version {version} disponible", "Version {version} available", version = version), detail.to_owned(), true, false, None)
            }
            update::State::Downloading(f) => {
                let pct = (f * 100.0) as u32;
                (tr!("Mise à jour…", "Updating…").to_owned(), trf!("téléchargement {pct} %", "downloading {pct} %", pct = pct), false, true, None)
            }
            update::State::Installing => (tr!("Installation…", "Installing…").to_owned(), tr!("RDM redémarre tout seul", "RDM restarts by itself").to_owned(), false, true, None),
            update::State::InstallFailed(r, reason) => {
                let version = &r.version;
                (trf!("Version {version} : échec", "Version {version}: failed", version = version), tr!("Cliquer pour réessayer", "Click to retry").to_owned(), true, false, Some(reason))
            }
            _ => return None,
        };
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::click());
        ui.add_space(8.0);
        let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
        let painter = ui.painter();
        widgets::soft_shadow(painter, rect, 14, 0.6 + 0.4 * hover, p.dark);
        let lift = |c: Color32| c.lerp_to_gamma(Color32::WHITE, 0.08 * hover);
        painter.add(widgets::gradient(ui, rect, 14, lift(p.accent), lift(p.accent2), vec2(0.9, 0.4)));
        painter.text(pos2(rect.left() + 16.0, rect.top() + 22.0), Align2::LEFT_CENTER, icon::ROCKET_LAUNCH, theme::regular(18.0), Color32::WHITE);
        painter.text(pos2(rect.left() + 42.0, rect.top() + 22.0), Align2::LEFT_CENTER, title, theme::semibold(13.5), Color32::WHITE);
        painter.text(pos2(rect.left() + 42.0, rect.top() + 46.0), Align2::LEFT_CENTER, detail, theme::regular(12.0), Color32::from_white_alpha(210));
        if moving {
            self.animating = true;
        }
        let response = match tip {
            Some(reason) => response.on_hover_text(reason),
            None => response,
        };
        (response.clicked() && clickable).then_some(Action::InstallUpdate)
    }

    /// Current speed limit; opens the settings.
    fn limit_card(&self, ui: &mut Ui, p: &Palette) -> eframe::egui::Response {
        let limit = self.manager.with_settings(|s| s.speed_limit_kib);
        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
        let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
        let painter = ui.painter();
        painter.rect(
            rect,
            14,
            p.surface.lerp_to_gamma(p.raised, hover),
            Stroke::new(theme::HAIRLINE, p.border),
            StrokeKind::Inside,
        );
        let tile = Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), Vec2::splat(34.0));
        let color = if limit > 0 { p.warning } else { p.success };
        painter.rect_filled(tile, 10, p.tint(color, 0.16));
        painter.text(tile.center(), Align2::CENTER_CENTER, icon::GAUGE, theme::regular(18.0), color);
        painter.text(pos2(tile.right() + 12.0, rect.top() + 19.0), Align2::LEFT_CENTER, tr!("Limite de vitesse", "Speed limit"), theme::regular(12.0), p.muted);
        let value = if limit == 0 { tr!("Illimitée", "Unlimited").to_owned() } else { speed(f64::from(limit) * 1024.0) };
        painter.text(pos2(tile.right() + 12.0, rect.top() + 39.0), Align2::LEFT_CENTER, value, theme::semibold(14.0), p.text);
        response.on_hover_text(tr!("Modifier dans les paramètres", "Change in the settings"))
    }
}

impl Filter {
    fn title_short(self, queues: &[crate::settings::Queue]) -> String {
        match self {
            Self::All => tr!("Tous", "All").to_owned(),
            other => other.title(queues),
        }
    }
}

/// A caption between groups of sidebar entries.
fn section_caption(ui: &mut Ui, text: &str) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        caption(ui, text);
    });
    ui.add_space(2.0);
}

/// A sidebar entry: icon, label, count; the selected one glows with the brand gradient.
fn nav_item(ui: &mut Ui, p: &Palette, glyph: &str, hue: Option<Color32>, label: &str, count: usize, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let chosen = ui.ctx().animate_bool_with_time(response.id.with("selected"), selected, 0.18);
    let painter = ui.painter();
    if chosen > 0.0 {
        painter.add(widgets::gradient(
            ui,
            rect,
            11,
            p.accent.gamma_multiply(0.22 * chosen),
            p.accent2.gamma_multiply(0.10 * chosen),
            Vec2::X,
        ));
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 18.0 * chosen));
        painter.add(widgets::gradient(ui, bar, 2, p.accent, p.accent2, Vec2::Y));
    }
    if hover > 0.0 && chosen < 1.0 {
        painter.rect_filled(rect, 11, p.raised.gamma_multiply(hover * (1.0 - chosen)));
    }
    let icon_color = if selected { p.accent } else { hue.unwrap_or(p.muted) };
    painter.text(pos2(rect.left() + 20.0, rect.center().y), Align2::CENTER_CENTER, glyph, theme::regular(17.0), icon_color);
    let (font, color) = if selected { (theme::semibold(13.5), p.text) } else { (theme::regular(13.5), p.text.lerp_to_gamma(p.muted, 0.35)) };
    painter.text(pos2(rect.left() + 40.0, rect.center().y), Align2::LEFT_CENTER, label, font, color);
    if count > 0 {
        let galley = painter.layout_no_wrap(count.to_string(), theme::semibold(11.0), if selected { p.accent } else { p.muted });
        let badge = Rect::from_center_size(
            pos2(rect.right() - 12.0 - galley.size().x / 2.0 - 4.0, rect.center().y),
            vec2(galley.size().x + 14.0, 20.0),
        );
        painter.rect_filled(badge, 10, if selected { p.accent.gamma_multiply(0.16) } else { p.raised });
        painter.galley(badge.center() - galley.size() / 2.0, galley, p.muted);
    }
    response.clicked()
}
