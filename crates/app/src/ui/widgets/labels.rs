//! Pills, captions, keys, an icon with its text; placing widgets in a hand-painted row.

use super::*;

/// Paints a status pill whose left edge is at `left`, vertically centred on `y`; returns its rect.
pub fn paint_pill(ui: &Ui, left: f32, y: f32, text: &str, color: Color32) -> Rect {
    let p = Palette::of(ui);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), theme::semibold(11.0), color);
    let rect = Rect::from_min_size(pos2(left, y - 10.0), vec2(galley.size().x + 16.0, 20.0));
    ui.painter().rect_filled(rect, 10, p.tint(color, 0.16));
    ui.painter().galley(pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color);
    rect
}

/// Adds widgets inside `rect` (left-aligned, vertically centred) without moving the parent's
/// layout cursor: for cards and rows painted by hand. (`Ui::put` would centre, and advance it.)
pub fn place<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    add(&mut ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center))))
}

/// An icon and its text, each in its own colour, on one baseline.
pub fn icon_text(ui: &mut Ui, glyph: &str, glyph_color: Color32, text: &str, text_color: Color32, size: f32) -> Response {
    let mut job = LayoutJob::default();
    let format = |font, color| TextFormat { font_id: font, color, valign: Align::Center, ..TextFormat::default() };
    job.append(glyph, 0.0, format(theme::regular(size + 2.0), glyph_color));
    job.append(text, 6.0, format(theme::regular(size), text_color));
    ui.label(job)
}

/// `CATEGORIES`-style section label.
pub fn caption(ui: &mut Ui, text: &str) {
    let p = Palette::of(ui);
    ui.label(RichText::new(text).font(theme::semibold(10.5)).color(p.faint).extra_letter_spacing(1.0));
}

/// A text naming the Ctrl key, as this computer's keyboard says it: Command on macOS.
pub fn shortcut(text: &str) -> String {
    if cfg!(target_os = "macos") { text.replace("Ctrl", "Cmd").replace("Strg", "Cmd") } else { text.to_owned() }
}

/// A keyboard key.
pub fn kbd(ui: &mut Ui, key: &str) {
    let p = Palette::of(ui);
    let galley = ui.painter().layout_no_wrap(key.to_owned(), theme::semibold(12.0), p.text);
    let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x + 16.0, 26.0), Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, 7, p.surface, Stroke::new(theme::HAIRLINE, p.border_strong), eframe::egui::StrokeKind::Inside);
    painter.line_segment([rect.left_bottom() + vec2(3.0, -1.5), rect.right_bottom() + vec2(-3.0, -1.5)], Stroke::new(2.0_f32, p.border));
    painter.galley(rect.center() - galley.size() / 2.0 - vec2(0.0, 1.0), galley, p.text);
}
