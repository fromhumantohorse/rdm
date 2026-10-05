//! Buttons: icon only, the primary action, secondary actions.

use super::*;

/// Room taken by an [`icon_button`].
pub const ICON_BUTTON: f32 = 32.0;
const ACCENT_PADDING: f32 = 34.0;

fn hover_anim(ui: &Ui, response: &Response) -> f32 {
    ui.ctx().animate_bool_with_time(response.id.with("hover"), response.hovered() || response.has_focus(), 0.12)
}

/// Round, frameless icon button; `tint` colours its hover halo and glyph.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str, tint: Option<Color32>) -> Response {
    let p = Palette::of(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(ICON_BUTTON), Sense::click());
    let hover = hover_anim(ui, &response);
    let tint = tint.unwrap_or(p.accent);
    let painter = ui.painter();
    if hover > 0.0 {
        painter.circle_filled(rect.center(), 16.0, tint.gamma_multiply(0.16 * hover));
    }
    let color = p.muted.lerp_to_gamma(if tint == p.accent { p.text } else { tint }, hover);
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, theme::regular(17.0), color);
    response.on_hover_text(tooltip)
}

fn accent_label(ui: &Ui, glyph: &str, label: &str) -> Arc<Galley> {
    ui.painter().layout_no_wrap(format!("{glyph}  {label}"), theme::semibold(14.0), Color32::WHITE)
}

/// Width an [`accent_button`] with this glyph and label takes.
pub fn accent_button_width(ui: &Ui, glyph: &str, label: &str) -> f32 {
    accent_label(ui, glyph, label).size().x + ACCENT_PADDING
}

/// The primary action: brand gradient, soft coloured glow.
pub fn accent_button(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let p = Palette::of(ui);
    let galley = accent_label(ui, glyph, label);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + ACCENT_PADDING, 38.0), Sense::click());
    let hover = hover_anim(ui, &response);
    let painter = ui.painter();
    let glow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: p.accent.gamma_multiply(0.30 + 0.25 * hover) };
    painter.add(glow.as_shape(rect, CornerRadius::same(12)));
    let lift = |c: Color32| c.lerp_to_gamma(Color32::WHITE, 0.10 * hover);
    let pressed = if response.is_pointer_button_down_on() { 0.92 } else { 1.0 };
    painter.add(gradient(ui, rect, 12, lift(p.accent).gamma_multiply(pressed), lift(p.accent2).gamma_multiply(pressed), vec2(0.96, 0.28)));
    painter.galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    response
}

/// Secondary action: quiet until hovered.
pub fn ghost_button(ui: &mut Ui, glyph: &str, label: &str) -> Response {
    let p = Palette::of(ui);
    let text = if label.is_empty() { glyph.to_owned() } else { format!("{glyph}  {label}") };
    let galley = ui.painter().layout_no_wrap(text, theme::semibold(13.0), p.text);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 34.0), Sense::click());
    let hover = hover_anim(ui, &response);
    let painter = ui.painter();
    painter.rect(
        rect,
        10,
        p.surface.lerp_to_gamma(p.raised, hover),
        Stroke::new(theme::HAIRLINE, p.border.lerp_to_gamma(p.border_strong, hover)),
        eframe::egui::StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, p.muted.lerp_to_gamma(p.text, 0.55 + 0.45 * hover));
    response
}
