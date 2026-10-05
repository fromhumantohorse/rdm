//! The dashboard: current speed in large type, the last minute as a chart, what runs and waits.

use super::*;

impl App<'_> {
    pub(in crate::ui) fn dashboard(&mut self, ui: &mut Ui, stats: Stats) {
        let p = Palette::of(ui);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
        let painter = ui.painter();
        widgets::soft_shadow(painter, rect, 22, 1.0, p.dark);
        let (from, to) = if p.dark {
            (p.surface.lerp_to_gamma(p.accent, 0.14), p.surface.lerp_to_gamma(p.accent2, 0.12))
        } else {
            (p.surface.lerp_to_gamma(p.accent, 0.06), p.surface.lerp_to_gamma(p.accent2, 0.07))
        };
        painter.add(widgets::gradient(ui, rect, 22, from, to, vec2(0.9, 0.44)));
        painter.rect_stroke(rect, 22, Stroke::new(theme::HAIRLINE, p.accent.gamma_multiply(0.28)), StrokeKind::Inside);

        let inner = rect.shrink2(vec2(28.0, 22.0));
        let left = Rect::from_min_size(inner.min, vec2(300.0, inner.height()));
        ui.scope_builder(UiBuilder::new().max_rect(left).layout(Layout::top_down(Align::Min)), |ui| {
            caption(ui, tr!("VITESSE DE TÉLÉCHARGEMENT", "DOWNLOAD SPEED"));
            ui.add_space(-2.0);
            let (value, unit) = split_speed(stats.speed);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(value).font(theme::bold(40.0)).color(p.text));
                ui.label(RichText::new(unit).font(theme::semibold(16.0)).color(p.muted));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                stat(ui, &p, icon::ARROW_CIRCLE_DOWN, &crate::i18n::count(stats.running as u64, ("actif", "actifs"), ("active", "active")), p.accent);
                stat(ui, &p, icon::HOURGLASS, &trf!("{} en attente", "{} waiting", stats.queued), p.muted);
                if let Some(eta) = (stats.speed > 1.0 && stats.total > stats.done).then(|| ((stats.total - stats.done) as f64 / stats.speed) as u64) {
                    stat(ui, &p, icon::TIMER, &trf!("reste {}", "{} left", duration(eta)), p.muted);
                }
            });
        });

        let chart = Rect::from_min_max(pos2(left.right() + 30.0, inner.top() + 22.0), inner.max);
        let history = self.manager.speed_history();
        let peak = history.iter().copied().fold(0.0f32, f32::max);
        let painter = ui.painter();
        painter.text(pos2(chart.left(), inner.top() + 6.0), Align2::LEFT_CENTER, tr!("Dernière minute", "Last minute"), theme::semibold(11.5), p.muted);
        if peak > 0.0 {
            painter.text(
                pos2(chart.right(), inner.top() + 6.0),
                Align2::RIGHT_CENTER,
                trf!("pic {}", "peak {}", speed(f64::from(peak))),
                theme::regular(11.5),
                p.muted,
            );
        }
        widgets::area_chart(ui, chart, &history);
        ui.add_space(22.0);
    }
}

/// `12,4` and `Mo/s`, set apart in the dashboard.
fn split_speed(bytes_per_sec: f64) -> (String, String) {
    let text = speed(bytes_per_sec);
    match text.split_once(' ') {
        Some((value, unit)) => (value.to_owned(), unit.to_owned()),
        None => (text, String::new()),
    }
}

fn stat(ui: &mut Ui, p: &Palette, glyph: &str, text: &str, color: Color32) {
    widgets::icon_text(ui, glyph, color, text, p.muted, 13.0);
}
