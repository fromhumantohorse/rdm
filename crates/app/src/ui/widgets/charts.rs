//! Charts: the speed area chart, a ring, a spinner.

use super::*;

/// Area chart of `samples` (oldest first): gradient line, fill fading downwards, a glowing head.
pub fn area_chart(ui: &Ui, rect: Rect, samples: &[f32]) {
    let p = Palette::of(ui);
    let painter = ui.painter_at(rect.expand(6.0));
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0_f32, p.border.gamma_multiply(0.6)));
    }
    let n = samples.len();
    if n < 2 {
        return;
    }
    // Light smoothing: sampled every 0.5 s, raw speeds are jagged.
    let smooth: Vec<f32> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            (samples[a] + 2.0 * samples[i] + samples[b]) / 4.0
        })
        .collect();
    let max = smooth.iter().copied().fold(0.0f32, f32::max);
    let scale = if max > 0.0 { max * 1.2 } else { 1.0 };
    let points: Vec<Pos2> = smooth
        .iter()
        .enumerate()
        .map(|(i, &s)| pos2(rect.left() + rect.width() * i as f32 / (n - 1) as f32, rect.bottom() - rect.height() * s / scale))
        .collect();

    let mut fill = Mesh::default();
    for (i, point) in points.iter().enumerate() {
        let t = i as f32 / (n - 1) as f32;
        let top = p.accent.lerp_to_gamma(p.accent2, t).gamma_multiply(if p.dark { 0.42 } else { 0.30 });
        fill.colored_vertex(*point, top);
        fill.colored_vertex(pos2(point.x, rect.bottom()), Color32::TRANSPARENT);
        if i > 0 {
            let k = (i * 2) as u32;
            fill.add_triangle(k - 2, k - 1, k);
            fill.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(Shape::mesh(fill));
    let (a, b) = (p.accent, p.accent2);
    let stroke = PathStroke::new_uv(2.4_f32, move |r: Rect, pos: Pos2| a.lerp_to_gamma(b, ((pos.x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0)));
    painter.add(Shape::line(points.clone(), stroke));
    if let Some(&head) = points.last()
        && max > 0.0
    {
        glow(&painter, head, Vec2::splat(14.0), p.accent2.gamma_multiply(0.5));
        painter.circle_filled(head, 4.0, p.accent2);
        painter.circle_filled(head, 1.8, Color32::WHITE);
    }
}

/// Ring chart: `segments` are (value, colour), drawn clockwise from the top.
pub fn donut(painter: &Painter, center: Pos2, radius: f32, width: f32, segments: &[(f32, Color32)], track: Color32) {
    painter.circle_stroke(center, radius, Stroke::new(width, track));
    let total: f32 = segments.iter().map(|(v, _)| v).sum();
    if total <= 0.0 {
        return;
    }
    let mut start = -TAU / 4.0;
    for &(value, color) in segments {
        if value <= 0.0 {
            continue;
        }
        let sweep = TAU * value / total;
        let steps = ((sweep * radius / 3.0) as usize).max(2);
        let points: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = start + sweep * i as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(Shape::line(points, Stroke::new(width, color)));
        start += sweep;
    }
}

/// A rotating three-quarter ring.
pub fn spinner(painter: &Painter, center: Pos2, radius: f32, color: Color32, time: f32) {
    let start = time * TAU * 1.2;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + TAU * 0.72 * i as f32 / 24.0;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(2.0_f32, color)));
}
