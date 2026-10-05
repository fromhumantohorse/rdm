//! Atoms: small, stateless building blocks, painted to the design system (`theme`).

mod bars;
mod buttons;
mod charts;
mod inputs;
mod labels;
mod paint;

use std::{f32::consts::TAU, sync::Arc};

use eframe::egui::{
    Align, Align2, Color32, CornerRadius, Galley, Id, Layout, Margin, Mesh, Painter, Pos2, Rect, Response, RichText,
    Sense, Shadow, Shape, Stroke, TextEdit, TextFormat, Ui, UiBuilder, Vec2,
    epaint::{PathStroke, RectShape, TessellationOptions, Tessellator},
    pos2,
    text::LayoutJob,
    vec2,
};

use super::theme::{self, Palette};
pub use bars::{pieces_progress, progress};
pub use buttons::{ICON_BUTTON, accent_button, accent_button_width, ghost_button, icon_button};
pub use charts::{area_chart, donut, spinner};
pub use inputs::{field, segmented, tab_bar, toggle};
pub use labels::{caption, icon_text, kbd, paint_pill, place};
pub use paint::{glow, gradient, soft_shadow};
