//! The sidebar's summary of the extension: connected, installed, or to install.

use super::*;

/// Sidebar summary: which browsers are connected; opens the window. `true` when clicked.
pub(in crate::ui) fn card(ui: &mut Ui, p: &Palette, (connected, installed): (Vec<Browser>, Vec<Browser>)) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let painter = ui.painter();
    painter.rect(
        rect,
        14,
        p.surface.lerp_to_gamma(p.raised, hover),
        Stroke::new(theme::HAIRLINE, p.border),
        eframe::egui::StrokeKind::Inside,
    );
    let tile = eframe::egui::Rect::from_center_size(eframe::egui::pos2(rect.left() + 28.0, rect.center().y), Vec2::splat(34.0));
    let color: Color32 = match (connected.is_empty(), installed.is_empty()) {
        (false, _) => p.success,
        (true, false) => p.muted,
        (true, true) => p.warning,
    };
    painter.rect_filled(tile, 10, p.tint(color, 0.16));
    painter.text(tile.center(), eframe::egui::Align2::CENTER_CENTER, icon::PUZZLE_PIECE, theme::regular(18.0), color);
    let x = tile.right() + 12.0;
    painter.text(eframe::egui::pos2(x, rect.top() + 19.0), eframe::egui::Align2::LEFT_CENTER, tr!("Extension navigateur", "Browser extension"), theme::regular(12.0), p.muted);
    let names = |list: &[Browser]| match list {
        [] => String::new(),
        [one] => one.name().to_owned(),
        [one, rest @ ..] => format!("{} +{}", one.name(), rest.len()),
    };
    let value = match (connected.is_empty(), installed.is_empty()) {
        (false, _) => trf!("Connectée · {}", "Connected · {}", names(&connected)),
        (true, false) => trf!("Installée · {}", "Installed · {}", names(&installed)),
        (true, true) => tr!("À installer", "Not installed").to_owned(),
    };
    let galley = painter.layout_no_wrap(value, theme::semibold(13.0), p.text);
    let clip = eframe::egui::Rect::from_min_max(eframe::egui::pos2(x, rect.top()), eframe::egui::pos2(rect.right() - 8.0, rect.bottom()));
    painter.with_clip_rect(clip).galley(eframe::egui::pos2(x, rect.top() + 39.0 - galley.size().y / 2.0), galley, p.text);
    response.on_hover_text(tr!("Installer ou vérifier l'extension", "Install or check the extension")).clicked()
}

/// Browsers heard from recently (the extension checks in every few minutes while the browser
/// runs), and browsers heard from at all, for the sidebar card.
pub(in crate::ui) fn connected(manager: &Manager) -> (Vec<Browser>, Vec<Browser>) {
    let now = unix_now();
    let mut seen = manager.browsers_seen();
    seen.sort_by_key(|(b, _)| b.name()); // a stable order in the sidebar
    let live = seen.iter().filter(|(_, t)| now.saturating_sub(*t) < LIVE_SECS).map(|(b, _)| *b).collect();
    (live, seen.into_iter().map(|(b, _)| b).collect())
}
