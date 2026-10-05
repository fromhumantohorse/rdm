//! Progress bars: one fill, or one piece per connection.

use super::*;

/// Rounded bar: gradient fill, a light sweeping across while `moving`; `None` = unknown length
/// (a segment travelling back and forth).
pub fn progress(ui: &Ui, rect: Rect, fraction: Option<f32>, (from, to): (Color32, Color32), moving: bool) {
    let p = Palette::of(ui);
    let painter = ui.painter();
    let radius = CornerRadius::same((rect.height() / 2.0) as u8);
    painter.rect_filled(rect, radius, p.border);
    let time = ui.input(|i| i.time) as f32;
    let fill = match fraction {
        Some(f) if f > 0.0 => {
            let mut fill = rect;
            fill.set_width((rect.width() * f.clamp(0.0, 1.0)).max(rect.height()));
            fill
        }
        Some(_) => return,
        None => {
            let t = (time * 0.7).fract();
            let width = rect.width() * 0.28;
            let left = rect.left() - width + (rect.width() + width) * t;
            Rect::from_min_max(pos2(left.max(rect.left()), rect.top()), pos2((left + width).min(rect.right()), rect.bottom()))
        }
    };
    if fill.width() <= 0.0 {
        return;
    }
    painter.add(gradient(ui, fill, radius, from, to, Vec2::X));
    if moving && fraction.is_some() {
        // A soft highlight sweeping left to right, clipped to the filled part.
        let band = 90.0;
        let x = fill.left() - band + (fill.width() + band * 2.0) * (time * 0.55).fract();
        let clip = painter.with_clip_rect(fill);
        let shine = Color32::from_white_alpha(if p.dark { 70 } else { 110 });
        let half = |l: f32, a: Color32, b: Color32| gradient(ui, Rect::from_min_size(pos2(l, fill.top()), vec2(band / 2.0, fill.height())), 0, a, b, Vec2::X);
        clip.add(half(x, Color32::TRANSPARENT, shine));
        clip.add(half(x + band / 2.0, shine, Color32::TRANSPARENT));
    }
}

/// The bar of a download split across connections: each piece at its place in the file, filled
/// as far as its connection got, with a thin gap between pieces and a glow at the head of each
/// piece still downloading.
pub fn pieces_progress(ui: &Ui, rect: Rect, pieces: &[domain::Segment], total: u64, (from, to): (Color32, Color32)) {
    let p = Palette::of(ui);
    let painter = ui.painter();
    let radius = CornerRadius::same((rect.height() / 2.0) as u8);
    painter.rect_filled(rect, radius, p.border);
    let x_of = |byte: u64| rect.left() + rect.width() * (byte.min(total) as f64 / total as f64) as f32;
    let time = ui.input(|i| i.time) as f32;
    let clip = painter.with_clip_rect(rect);
    for piece in pieces {
        let (left, right) = (x_of(piece.start), x_of(piece.end.saturating_add(1)));
        // A 1.5 px gap on the right: the pieces read as separate connections.
        let right = if right - left > 4.0 { right - 1.5 } else { right };
        let head = x_of(piece.pos).clamp(left, right);
        if head > left {
            // The colour of the whole bar at this place: the pieces together show one gradient.
            let t = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let fill = Rect::from_min_max(pos2(left, rect.top()), pos2(head, rect.bottom()));
            clip.add(gradient(ui, fill, 1, from.lerp_to_gamma(to, t(left)), from.lerp_to_gamma(to, t(head)), Vec2::X));
        }
        if piece.pos <= piece.end && head < right {
            // Still downloading: a soft pulse at the head of the piece.
            let alpha = 110.0 + 90.0 * (time * 3.0 + left * 0.05).sin();
            let glow = Rect::from_min_max(pos2((head - 3.0).max(left), rect.top()), pos2(head + 1.0, rect.bottom()));
            clip.rect_filled(glow, 1, Color32::from_white_alpha(alpha as u8));
        }
    }
}
