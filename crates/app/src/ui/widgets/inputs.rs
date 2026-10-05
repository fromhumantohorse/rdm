//! Inputs: a text field with its icon, a switch, a segmented choice, tabs.

use super::*;

/// Single-line field with a leading icon; `Ctrl+F`-style focus goes through `id`.
pub fn field(ui: &mut Ui, id: Id, text: &mut String, hint: &str, glyph: &str, width: f32) -> Response {
    let p = Palette::of(ui);
    let response = ui.add(
        TextEdit::singleline(text)
            .id(id)
            .hint_text(RichText::new(hint).color(p.faint))
            .desired_width(width)
            .margin(Margin { left: 38, right: 12, top: 10, bottom: 10 })
            .font(theme::regular(14.0)),
    );
    let color = if response.has_focus() { p.accent } else { p.muted };
    ui.painter().text(response.rect.left_center() + vec2(19.0, 0.0), Align2::CENTER_CENTER, glyph, theme::regular(17.0), color);
    response
}

/// Label on the left, iOS-style switch on the right; the whole row toggles.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, detail: &str) -> Response {
    let p = Palette::of(ui);
    // The detail wraps before the switch (a long one would run under it and out of the window).
    let text_width = (ui.available_width() - 64.0).max(80.0);
    let detail_galley = (!detail.is_empty()).then(|| ui.painter().layout(detail.to_owned(), theme::regular(12.0), p.muted, text_width));
    let height = detail_galley.as_ref().map_or(34.0, |g| (34.0 + g.size().y).max(48.0));
    let (rect, mut response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(response.id, *on, 0.16);
    let painter = ui.painter();
    let text_x = rect.left();
    match detail_galley {
        None => {
            painter.text(pos2(text_x, rect.center().y), Align2::LEFT_CENTER, label, theme::regular(14.0), p.text);
        }
        Some(galley) => {
            painter.text(pos2(text_x, rect.top() + 14.0), Align2::LEFT_CENTER, label, theme::regular(14.0), p.text);
            painter.galley(pos2(text_x, rect.top() + 26.0), galley, p.muted);
        }
    }
    let track = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(42.0, 24.0));
    if t > 0.0 {
        painter.add(gradient(ui, track, 12, p.accent.gamma_multiply(t), p.accent2.gamma_multiply(t), Vec2::X));
    }
    if t < 1.0 {
        painter.rect_filled(track, 12, p.border_strong.gamma_multiply(1.0 - t));
    }
    let knob = pos2(track.left() + 12.0 + t * (track.width() - 24.0), track.center().y);
    painter.circle_filled(knob + vec2(0.0, 1.0), 9.0, Color32::from_black_alpha(40));
    painter.circle_filled(knob, 9.0, Color32::WHITE);
    response
}

/// Pick one of a few options (theme): a sunken track, the chosen option raised.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    let p = Palette::of(ui);
    let width = ui.available_width().min(140.0 * options.len() as f32);
    // Its own (automatic, unique) id: several selectors in one panel must not share their cells'.
    let (rect, own) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let track = if p.dark { p.bg } else { p.border.lerp_to_gamma(p.raised, 0.35) };
    ui.painter().rect_filled(rect, 12, track);
    let slot = (rect.width() - 8.0) / options.len() as f32;
    let mut changed = false;
    for (i, (option, glyph, label)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(rect.left() + 4.0 + slot * i as f32, rect.top() + 4.0), vec2(slot, rect.height() - 8.0));
        let response = ui.interact(cell, own.id.with(("segment", i)), Sense::click());
        if response.clicked() && *value != *option {
            *value = *option;
            changed = true;
        }
        let selected = *value == *option;
        let painter = ui.painter();
        if selected {
            soft_shadow(painter, cell, 9, 0.5, p.dark);
            painter.rect_filled(cell, 9, p.surface);
        } else if response.hovered() {
            painter.rect_filled(cell, 9, p.surface.gamma_multiply(0.5));
        }
        let color = if selected { p.text } else { p.muted };
        let font = if selected { theme::semibold(13.0) } else { theme::regular(13.0) };
        painter.text(cell.center(), Align2::CENTER_CENTER, format!("{glyph}  {label}"), font, color);
    }
    changed
}

/// Tabs: like `segmented`, but each tab as wide as its label (plus padding inside), with a gap
/// between tabs and around them; spare width is shared out so the bar fills its line.
pub fn tab_bar<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    const INSET: f32 = 5.0; // between the track's edge and the tabs
    const GAP: f32 = 6.0; // between two tabs
    const PAD: f32 = 16.0; // inside a tab, on each side of its label (at most)
    const MIN_PAD: f32 = 6.0;
    let p = Palette::of(ui);
    let label = |i: usize| format!("{}  {}", options[i].1, options[i].2);
    // The selected tab's (semibold) width for every tab: selecting one does not move the others.
    let text: Vec<f32> = (0..options.len()).map(|i| ui.painter().layout_no_wrap(label(i), theme::semibold(13.0), Color32::WHITE).size().x).collect();
    let gaps = INSET * 2.0 + GAP * options.len().saturating_sub(1) as f32;
    // Labels too long for the line (a long language): less padding first, overflow last.
    let room = (ui.available_width() - gaps - text.iter().sum::<f32>()) / (2 * options.len()) as f32;
    let pad = room.clamp(MIN_PAD, PAD);
    let natural: Vec<f32> = text.iter().map(|t| t + 2.0 * pad).collect();
    let needed = natural.iter().sum::<f32>() + gaps;
    let width = ui.available_width().max(needed);
    let extra = (width - needed) / options.len() as f32;
    let (rect, own) = ui.allocate_exact_size(vec2(width, 44.0), Sense::hover());
    let track = if p.dark { p.bg } else { p.border.lerp_to_gamma(p.raised, 0.35) };
    ui.painter().rect_filled(rect, 13, track);
    let mut changed = false;
    let mut left = rect.left() + INSET;
    for (i, (option, _, _)) in options.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(left, rect.top() + INSET), vec2(natural[i] + extra, rect.height() - 2.0 * INSET));
        left += cell.width() + GAP;
        let response = ui.interact(cell, own.id.with(("tab", i)), Sense::click());
        if response.clicked() && *value != *option {
            *value = *option;
            changed = true;
        }
        let selected = *value == *option;
        let painter = ui.painter();
        if selected {
            soft_shadow(painter, cell, 9, 0.5, p.dark);
            painter.rect_filled(cell, 9, p.surface);
        } else if response.hovered() {
            painter.rect_filled(cell, 9, p.surface.gamma_multiply(0.5));
        }
        let (color, font) = if selected { (p.text, theme::semibold(13.0)) } else { (p.muted, theme::regular(13.0)) };
        painter.text(cell.center(), Align2::CENTER_CENTER, label(i), font, color);
    }
    changed
}
