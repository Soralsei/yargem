//! A fused, segmented radio group for egui ("segmented control").
//!
//! ```ignore
//! #[derive(Clone, Copy, PartialEq)]
//! enum Mode { Read, Write }
//!
//! ui.add(
//!     Segmented::new(&mut self.mode)
//!         .segment(Mode::Read, "R").tooltip("Read")
//!         .segment(Mode::Write, "W").tooltip("Write"),
//! );
//! ```
//!
//! Behaves like a set of `radio_value`s: clicking a segment assigns its value,
//! the returned `Response` reports `changed()`, each segment is focusable
//! (Tab, Space/Enter), Left/Right arrows move the selection inside the group,
//! and AccessKit sees a `RadioGroup` containing `RadioButton`s.

use std::sync::Arc;

use egui::{
    pos2, vec2, AsIdSalt, Color32, CornerRadius, FocusDirection, FontSelection, Galley, IdSalt,
    Key, Modifiers, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, TextWrapMode, Ui,
    UiBuilder, Vec2, Widget, WidgetInfo, WidgetText, WidgetType,
};

struct Segment<T> {
    value: T,
    text: WidgetText,
    tooltip: Option<WidgetText>,
}

#[must_use = "You should put this widget in a ui with `ui.add(widget);`"]
pub struct Segmented<'a, T> {
    current: &'a mut T,
    segments: Vec<Segment<T>>,
    id_salt: Option<IdSalt>,
    group_label: Option<String>,
    font: FontSelection,
    padding: Option<Vec2>,
    min_segment_size: Vec2,
    uniform: bool,
    inset: f32,
    corner_radius: u8,
    stroke: Option<Stroke>,
    track_fill: Option<Color32>,
    selected_fill: Option<Color32>,
    selected_text: Option<Color32>,
    text_color: Option<Color32>,
    separators: bool,
    animate: bool,
}

impl<'a, T: PartialEq + Clone> Segmented<'a, T> {
    pub fn new(current: &'a mut T) -> Self {
        Self {
            current,
            segments: Vec::new(),
            id_salt: None,
            group_label: None,
            font: TextStyle::Button.into(),
            padding: None,
            min_segment_size: Vec2::ZERO,
            uniform: true,
            inset: 2.0,
            corner_radius: 4,
            stroke: None,
            track_fill: None,
            selected_fill: None,
            selected_text: None,
            text_color: None,
            separators: true,
            animate: true,
        }
    }

    /// Add a segment that selects `value`.
    pub fn segment(mut self, value: T, text: impl Into<WidgetText>) -> Self {
        self.segments.push(Segment {
            value,
            text: text.into(),
            tooltip: None,
        });
        self
    }

    /// Add several `(value, text)` segments at once.
    pub fn segments<S: Into<WidgetText>>(mut self, iter: impl IntoIterator<Item = (T, S)>) -> Self {
        for (value, text) in iter {
            self = self.segment(value, text);
        }
        self
    }

    /// Hover text for the most recently added segment.
    pub fn tooltip(mut self, text: impl Into<WidgetText>) -> Self {
        if let Some(last) = self.segments.last_mut() {
            last.tooltip = Some(text.into());
        }
        self
    }

    /// Stable id (needed for the slide animation when several groups share a parent
    /// and the layout changes between frames).
    pub fn id_salt(mut self, salt: impl AsIdSalt) -> Self {
        self.id_salt = Some(IdSalt::new(salt));
        self
    }

    /// Accessible name of the whole group (screen readers).
    pub fn group_label(mut self, label: impl Into<String>) -> Self {
        self.group_label = Some(label.into());
        self
    }

    /// Fallback font for segment labels (`TextStyle::Monospace`, a `FontId`, …).
    pub fn font(mut self, font: impl Into<FontSelection>) -> Self {
        self.font = font.into();
        self
    }

    /// Space between a label and the control's edges.
    /// Default: 10 horizontally, `spacing.button_padding.y` vertically (like a button).
    pub fn padding(mut self, padding: impl Into<Vec2>) -> Self {
        self.padding = Some(padding.into());
        self
    }

    pub fn min_segment_size(mut self, size: impl Into<Vec2>) -> Self {
        self.min_segment_size = size.into();
        self
    }

    /// All segments as wide as the widest one (default: true).
    pub fn uniform(mut self, uniform: bool) -> Self {
        self.uniform = uniform;
        self
    }

    /// Gap between the outer border and the segments (default: 2).
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset = inset.max(0.0);
        self
    }

    pub fn corner_radius(mut self, radius: u8) -> Self {
        self.corner_radius = radius;
        self
    }

    pub fn stroke(mut self, stroke: impl Into<Stroke>) -> Self {
        self.stroke = Some(stroke.into());
        self
    }

    pub fn track_fill(mut self, color: Color32) -> Self {
        self.track_fill = Some(color);
        self
    }

    pub fn selected_fill(mut self, color: Color32) -> Self {
        self.selected_fill = Some(color);
        self
    }

    pub fn selected_text_color(mut self, color: Color32) -> Self {
        self.selected_text = Some(color);
        self
    }

    pub fn text_color(mut self, color: Color32) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Thin dividers between unselected neighbours (default: true).
    pub fn separators(mut self, on: bool) -> Self {
        self.separators = on;
        self
    }

    /// Slide the highlight between segments (default: true).
    pub fn animate(mut self, on: bool) -> Self {
        self.animate = on;
        self
    }
}

impl<T: PartialEq + Clone> Widget for Segmented<'_, T> {
    fn ui(self, ui: &mut Ui) -> Response {
        let mut builder = UiBuilder::new();
        if let Some(salt) = self.id_salt {
            builder = builder.id_salt(salt);
        }
        let group_label = self.group_label.clone();
        ui.scope_builder(builder, |ui| {
            // egui 0.36 has no UiBuilder::accessibility_role/label: the child Ui is
            // created as a GenericContainer, so overwrite its AccessKit node here.
            ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
                node.set_role(egui::accesskit::Role::RadioGroup);
                if let Some(label) = group_label {
                    node.set_label(label);
                }
            });
            self.show_inner(ui)
        })
        .inner
    }
}

impl<T: PartialEq + Clone> Segmented<'_, T> {
    fn show_inner(self, ui: &mut Ui) -> Response {
        let Self {
            current,
            segments,
            font,
            padding,
            min_segment_size,
            uniform,
            inset,
            corner_radius,
            stroke,
            track_fill,
            selected_fill,
            selected_text,
            text_color,
            separators,
            animate,
            ..
        } = self;

        let group_id = ui.unique_id();
        let n = segments.len();
        if n == 0 {
            return ui.allocate_response(Vec2::ZERO, Sense::hover());
        }

        // ---- Layout -------------------------------------------------------
        let labels: Vec<String> = segments.iter().map(|s| s.text.text().to_owned()).collect();
        let mut tooltips = Vec::with_capacity(n);
        let mut values = Vec::with_capacity(n);
        let galleys: Vec<Arc<Galley>> = segments
            .into_iter()
            .map(|s| {
                tooltips.push(s.tooltip);
                values.push(s.value);
                s.text
                    .into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, font.clone())
            })
            .collect();

        let padding = padding.unwrap_or_else(|| vec2(10.0, ui.spacing().button_padding.y));
        // Outer height follows the same rule as a button (text + padding, at least
        // `interact_size.y`); the inset is carved out of it rather than added on top.
        let outer_h = galleys
            .iter()
            .map(|g| g.size().y + 2.0 * padding.y)
            .fold(min_segment_size.y, f32::max)
            .max(ui.spacing().interact_size.y);
        let height = outer_h - 2.0 * inset; // segment height
        let mut widths: Vec<f32> = galleys
            .iter()
            .map(|g| (g.size().x + 2.0 * padding.x).max(min_segment_size.x))
            .collect();
        if uniform {
            let w = widths.iter().copied().fold(0.0, f32::max);
            widths.fill(w);
        }

        let total = vec2(widths.iter().sum::<f32>() + 2.0 * inset, outer_h);
        let (outer, outer_response) = ui.allocate_exact_size(total, Sense::hover());

        let mut rects = Vec::with_capacity(n);
        let mut x = outer.left() + inset;
        for w in &widths {
            rects.push(Rect::from_min_size(
                pos2(x, outer.top() + inset),
                vec2(*w, height),
            ));
            x += w;
        }

        // ---- Interaction --------------------------------------------------
        let enabled = ui.is_enabled();
        let mut changed = false;
        let mut responses = Vec::with_capacity(n);
        for (i, rect) in rects.iter().enumerate() {
            let mut r = ui.interact(*rect, group_id.with(i), Sense::click());
            if let Some(tip) = tooltips[i].take() {
                r = r.on_hover_text(tip);
            }
            if r.clicked() && *current != values[i] {
                *current = values[i].clone();
                changed = true;
            }
            responses.push(r);
        }

        // Radio-group keyboard behavior: Left/Right move the selection *and* the
        // focus inside the group. At either end we leave the key alone so egui's
        // spatial focus navigation can move out of the group as usual.
        if let Some(i) = responses.iter().position(|r| r.has_focus()) {
            let step = ui.input_mut(|inp| {
                if i > 0 && inp.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                    Some(i - 1)
                } else if i + 1 < n && inp.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    Some(i + 1)
                } else {
                    None
                }
            });
            if let Some(j) = step {
                ui.memory_mut(|m| {
                    m.move_focus(FocusDirection::None); // cancel egui's own arrow move
                    m.request_focus(responses[j].id);
                });
                if *current != values[j] {
                    *current = values[j].clone();
                    changed = true;
                }
            }
        }

        let selected = values.iter().position(|v| v == current);

        for (i, r) in responses.iter().enumerate() {
            let is_sel = selected == Some(i);
            let label = labels[i].clone();
            r.widget_info(|| {
                WidgetInfo::selected(WidgetType::RadioButton, enabled, is_sel, &label)
            });
        }

        // ---- Painting -----------------------------------------------------
        if ui.is_rect_visible(outer) {
            let visuals = ui.visuals();
            let stroke = stroke.unwrap_or_else(|| {
                let s = visuals.widgets.noninteractive.bg_stroke;
                Stroke::new(s.width.max(1.0), s.color)
            });
            let track_fill = track_fill.unwrap_or(visuals.extreme_bg_color);
            let selected_fill = selected_fill.unwrap_or(visuals.selection.bg_fill);
            let selected_text = selected_text.unwrap_or_else(|| visuals.strong_text_color());
            let text_color = text_color.unwrap_or_else(|| visuals.widgets.inactive.text_color());
            let hover_fill = visuals.widgets.hovered.weak_bg_fill;
            let active_fill = visuals.widgets.active.weak_bg_fill;
            let focus_stroke = visuals.selection.stroke;

            let inner_r = corner_radius.saturating_sub(inset.round() as u8);
            let seg_radius = |i: usize| CornerRadius {
                nw: if i == 0 { inner_r } else { 0 },
                sw: if i == 0 { inner_r } else { 0 },
                ne: if i + 1 == n { inner_r } else { 0 },
                se: if i + 1 == n { inner_r } else { 0 },
            };

            let painter = ui.painter();
            painter.rect_filled(outer, corner_radius, track_fill);

            // Animated (fractional) position of the highlight, in segment units.
            let pos = selected.map(|s| {
                if animate {
                    ui.ctx().animate_value_with_time(
                        group_id.with("seg_anim"),
                        s as f32,
                        ui.style().animation_time,
                    )
                } else {
                    s as f32
                }
            });

            // Hover / press feedback on non-selected segments.
            for (i, r) in responses.iter().enumerate() {
                if selected == Some(i) {
                    continue;
                }
                let fill = if r.is_pointer_button_down_on() {
                    Some(active_fill)
                } else if r.hovered() || r.has_focus() {
                    Some(hover_fill)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    painter.rect_filled(rects[i], seg_radius(i), fill);
                }
            }

            // Sliding highlight.
            if let Some(p) = pos {
                let i0 = (p.floor() as usize).min(n - 1);
                let i1 = (i0 + 1).min(n - 1);
                let t = p - i0 as f32;
                let rect = rects[i0].lerp_towards(&rects[i1], t);
                let (r0, r1) = (seg_radius(i0), seg_radius(i1));
                let mix = |a: u8, b: u8| egui::lerp(a as f32..=b as f32, t).round() as u8;
                let radius = CornerRadius {
                    nw: mix(r0.nw, r1.nw),
                    ne: mix(r0.ne, r1.ne),
                    sw: mix(r0.sw, r1.sw),
                    se: mix(r0.se, r1.se),
                };
                painter.rect_filled(rect, radius, selected_fill);
            }

            // Dividers fade out next to the highlight.
            if separators {
                for i in 0..n - 1 {
                    let alpha = match pos {
                        Some(p) => (p - i as f32)
                            .abs()
                            .min((p - (i + 1) as f32).abs())
                            .clamp(0.0, 1.0),
                        None => 1.0,
                    };
                    let hovered = responses[i].hovered() || responses[i + 1].hovered();
                    if alpha > 0.0 && !hovered {
                        let x = rects[i].right();
                        let y = rects[i].y_range().shrink(height * 0.25);
                        painter.vline(x, y, Stroke::new(1.0, stroke.color.gamma_multiply(alpha)));
                    }
                }
            }

            // Labels; text color follows the highlight.
            for (i, g) in galleys.iter().enumerate() {
                let t = pos.map_or(0.0, |p| (1.0 - (p - i as f32).abs()).clamp(0.0, 1.0));
                let color = text_color.lerp_to_gamma(selected_text, t);
                let text_pos = rects[i].center() - g.size() / 2.0;
                painter.galley(text_pos, g.clone(), color);
            }

            // Keyboard focus ring.
            for (i, r) in responses.iter().enumerate() {
                if r.has_focus() {
                    painter.rect_stroke(
                        rects[i].shrink(1.0),
                        seg_radius(i),
                        focus_stroke,
                        StrokeKind::Inside,
                    );
                }
            }

            painter.rect_stroke(outer, corner_radius, stroke, StrokeKind::Inside);
        }

        let mut response = responses.into_iter().fold(outer_response, |acc, r| acc | r);
        if changed {
            response.mark_changed();
        }
        response
    }
}

// ---- Enum convenience ------------------------------------------------------

/// Implement for a fieldless enum to get `Segmented::from_enum` / `ui.segmented(..)`.
pub trait SegmentedEnum: PartialEq + Clone + Sized + 'static {
    const VARIANTS: &'static [Self];
    fn label(&self) -> WidgetText;
    fn tooltip(&self) -> Option<WidgetText> {
        None
    }
}

impl<'a, T: SegmentedEnum> Segmented<'a, T> {
    pub fn from_enum(current: &'a mut T) -> Self {
        let mut s = Self::new(current);
        for v in T::VARIANTS {
            s = s.segment(v.clone(), v.label());
            if let Some(tip) = v.tooltip() {
                s = s.tooltip(tip);
            }
        }
        s
    }
}

pub trait SegmentedUiExt {
    fn segmented<T: SegmentedEnum>(&mut self, current: &mut T) -> Response;
}

impl SegmentedUiExt for Ui {
    fn segmented<T: SegmentedEnum>(&mut self, current: &mut T) -> Response {
        self.add(Segmented::from_enum(current))
    }
}
