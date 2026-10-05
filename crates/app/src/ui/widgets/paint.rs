//! Painting helpers: gradients, glows, shadows.

use super::*;

/// A rounded rectangle filled with a gradient along `dir` (a unit vector): tessellated like any
/// filled shape — anti-aliased rounded corners — then recoloured vertex by vertex.
pub fn gradient(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius>, from: Color32, to: Color32, dir: Vec2) -> Shape {
    let mut tessellator = Tessellator::new(ui.ctx().pixels_per_point(), TessellationOptions::default(), [1, 1], Vec::new());
    let mut mesh = Mesh::default();
    tessellator.tessellate_rect(&RectShape::filled(rect, radius, Color32::WHITE), &mut mesh);
    let along = |p: Pos2| p.to_vec2().dot(dir);
    let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()].map(along);
    let (lo, hi) = corners.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &c| (lo.min(c), hi.max(c)));
    let span = (hi - lo).max(f32::EPSILON);
    for v in &mut mesh.vertices {
        let t = ((along(v.pos) - lo) / span).clamp(0.0, 1.0);
        let coverage = f32::from(v.color.a()) / 255.0; // anti-aliasing feather
        v.color = from.lerp_to_gamma(to, t).gamma_multiply(coverage);
    }
    Shape::mesh(mesh)
}

/// Soft radial light (the "aurora" behind the dashboard, halos behind icons).
pub fn glow(painter: &Painter, center: Pos2, radius: Vec2, color: Color32) {
    const STEPS: u32 = 64;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, color);
    for i in 0..=STEPS {
        let a = TAU * i as f32 / STEPS as f32;
        mesh.colored_vertex(center + vec2(a.cos() * radius.x, a.sin() * radius.y), Color32::TRANSPARENT);
    }
    for i in 1..=STEPS {
        mesh.add_triangle(0, i, i + 1);
    }
    painter.add(Shape::mesh(mesh));
}

pub fn soft_shadow(painter: &Painter, rect: Rect, radius: u8, strength: f32, dark: bool) {
    if strength <= 0.0 {
        return;
    }
    let alpha = if dark { 110.0 } else { 34.0 } * strength;
    let shadow = Shadow { offset: [0, 8], blur: 26, spread: 0, color: Color32::from_black_alpha(alpha as u8) };
    painter.add(shadow.as_shape(rect, CornerRadius::same(radius)));
}
