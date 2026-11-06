//! `Badge` (a pill) and `Dot` (a status indicator) for egui 0.36.
//!
//! ```ignore
//! ui.add(Badge::new("beta"));
//! ui.add(Badge::new("3").tone(Tone::Error));
//! ui.add(Badge::new("draft").tone(Tone::Warning).outline(true));
//!
//! ui.horizontal(|ui| {
//!     ui.add(Dot::new(Tone::Success).pulse(true));
//!     ui.label("Online");
//! });
//!
//! // Pinned to another widget's top-right corner:
//! let r = ui.button("Inbox");
//! Badge::new("12").tone(Tone::Error).overlay(ui, &r);
//! ```

use std::sync::Arc;

use egui::{
    vec2, Color32, FontSelection, Galley, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    TextStyle, TextWrapMode, Ui, Vec2, Visuals, Widget, WidgetInfo, WidgetText, WidgetType,
};

/// Semantic color, resolved against the current theme.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Tone {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Error,
    Custom(Color32),
}

impl Tone {
    pub fn color(self, v: &Visuals) -> Color32 {
        match self {
            Self::Neutral => v.widgets.inactive.bg_fill,
            Self::Accent => v.selection.bg_fill,
            Self::Success => Color32::from_rgb(0x2f, 0x9e, 0x44),
            Self::Warning => v.warn_fg_color,
            Self::Error => v.error_fg_color,
            Self::Custom(c) => c,
        }
    }
}

/// Black or white, whichever reads better on `bg`.
fn on_color(bg: Color32) -> Color32 {
    let luma = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if luma > 150.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

// ---- Badge -----------------------------------------------------------------

#[must_use = "Add it with `ui.add(badge)` or pin it with `badge.overlay(ui, &response)`"]
pub struct Badge {
    text: WidgetText,
    tone: Tone,
    outline: bool,
    text_color: Option<Color32>,
    font: FontSelection,
    sense: Sense,
}

impl Badge {
    pub fn new(text: impl Into<WidgetText>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Neutral,
            outline: false,
            text_color: None,
            font: TextStyle::Small.into(),
            sense: Sense::hover(),
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Shorthand for `.tone(Tone::Custom(color))`.
    pub fn color(mut self, color: Color32) -> Self {
        self.tone = Tone::Custom(color);
        self
    }

    /// Border + colored text instead of a filled pill.
    pub fn outline(mut self, outline: bool) -> Self {
        self.outline = outline;
        self
    }

    pub fn text_color(mut self, color: Color32) -> Self {
        self.text_color = Some(color);
        self
    }

    pub fn font(mut self, font: impl Into<FontSelection>) -> Self {
        self.font = font.into();
        self
    }

    /// e.g. `Sense::click()` for a clickable chip.
    pub fn sense(mut self, sense: Sense) -> Self {
        self.sense = sense;
        self
    }

    /// Paint centered on `center` without taking layout space.
    pub fn paint_at(self, ui: &Ui, center: Pos2) -> Rect {
        let p = self.prepare(ui);
        let rect = Rect::from_center_size(center, p.size);
        p.paint(ui, rect);
        rect
    }

    /// Paint on the top-right corner of another widget (notification counter).
    pub fn overlay(self, ui: &Ui, target: &Response) -> Rect {
        self.paint_at(ui, target.rect.right_top())
    }

    fn prepare(self, ui: &Ui) -> Prepared {
        let galley =
            self.text
                .into_galley(ui, Some(TextWrapMode::Extend), f32::INFINITY, self.font);
        let pad = vec2(6.0, 1.0);
        let h = galley.size().y + 2.0 * pad.y;
        // Never narrower than tall, so a single digit gives a circle.
        let size = vec2((galley.size().x + 2.0 * pad.x).max(h), h);

        let base = self.tone.color(ui.visuals());
        let (fill, stroke, text) = if self.outline {
            let text = match self.tone {
                Tone::Neutral => ui.visuals().text_color(),
                _ => base,
            };
            (Color32::TRANSPARENT, Stroke::new(1.0, base), text)
        } else {
            (base, Stroke::NONE, on_color(base))
        };

        Prepared {
            galley,
            size,
            fill,
            stroke,
            text: self.text_color.unwrap_or(text),
        }
    }
}

struct Prepared {
    galley: Arc<Galley>,
    size: Vec2,
    fill: Color32,
    stroke: Stroke,
    text: Color32,
}

impl Prepared {
    fn paint(&self, ui: &Ui, rect: Rect) {
        let radius = (rect.height() / 2.0).round().min(255.0) as u8;
        let painter = ui.painter();
        painter.rect(rect, radius, self.fill, self.stroke, StrokeKind::Inside);
        let pos = rect.center() - self.galley.size() / 2.0;
        painter.galley(pos, self.galley.clone(), self.text);
    }
}

impl Widget for Badge {
    fn ui(self, ui: &mut Ui) -> Response {
        let sense = self.sense;
        let label = self.text.text().to_owned();
        let prepared = self.prepare(ui);
        let (rect, response) = ui.allocate_exact_size(prepared.size, sense);
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Label, enabled, &label));
        if ui.is_rect_visible(rect) {
            prepared.paint(ui, rect);
        }
        response
    }
}

// ---- Dot -------------------------------------------------------------------

#[must_use = "Add it with `ui.add(dot)` or pin it with `dot.overlay(ui, &response)`"]
pub struct Dot {
    tone: Tone,
    radius: f32,
    hollow: bool,
    pulse: bool,
}

impl Dot {
    pub fn new(tone: Tone) -> Self {
        Self {
            tone,
            radius: 4.0,
            hollow: false,
            pulse: false,
        }
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }

    /// Ring instead of a filled disc (e.g. "offline").
    pub fn hollow(mut self, hollow: bool) -> Self {
        self.hollow = hollow;
        self
    }

    /// Expanding, fading halo ("live"). Repaints continuously while visible.
    pub fn pulse(mut self, pulse: bool) -> Self {
        self.pulse = pulse;
        self
    }

    pub fn paint_at(&self, ui: &Ui, center: Pos2) {
        let color = self.tone.color(ui.visuals());
        let painter = ui.painter();

        if self.pulse {
            const PERIOD: f64 = 1.6;
            let t = ((ui.input(|i| i.time) % PERIOD) / PERIOD) as f32;
            let halo = self.radius * (1.0 + 1.5 * t);
            painter.circle_filled(center, halo, color.gamma_multiply(0.4 * (1.0 - t)));
            ui.ctx().request_repaint();
        }

        if self.hollow {
            painter.circle_stroke(center, self.radius - 0.75, Stroke::new(1.5, color));
        } else {
            painter.circle_filled(center, self.radius, color);
        }
    }

    /// Paint on the top-right corner of another widget ("unread" marker).
    pub fn overlay(self, ui: &Ui, target: &Response) {
        self.paint_at(ui, target.rect.right_top());
    }
}

impl Widget for Dot {
    fn ui(self, ui: &mut Ui) -> Response {
        // As tall as a line of body text, so it centers next to a label.
        let h = ui
            .text_style_height(&TextStyle::Body)
            .max(2.0 * self.radius);
        let (rect, response) = ui.allocate_exact_size(vec2(2.0 * self.radius, h), Sense::hover());
        if ui.is_rect_visible(rect) {
            self.paint_at(ui, rect.center());
        }
        response
    }
}
