//! App-wide egui style. Sizes and colours belong here, so widgets can use
//! egui's defaults instead of setting their own.

use egui::{Color32, Context, CornerRadius, Stroke, TextStyle, Theme, Visuals};

pub const SECTION_GAP: f32 = 12.0;

pub fn install(ctx: &Context) {
    ctx.all_styles_mut(|style| {
        let spacing = &mut style.spacing;
        spacing.item_spacing = egui::vec2(6.0, 4.0);
        spacing.button_padding = egui::vec2(8.0, 1.0);
        spacing.interact_size = egui::vec2(16.0, 20.0);
        spacing.indent = 20.0;

        for (text_style, size) in [
            (TextStyle::Small, 11.0),
            (TextStyle::Body, 13.0),
            (TextStyle::Button, 13.0),
            (TextStyle::Monospace, 13.0),
        ] {
            if let Some(font) = style.text_styles.get_mut(&text_style) {
                font.size = size;
            }
        }
    });

    ctx.style_mut_of(Theme::Dark, |style| dark_visuals(&mut style.visuals));
}

/// egui's dark palette is already close, so this only sets what differs.
fn dark_visuals(visuals: &mut Visuals) {
    visuals.faint_bg_color = Color32::from_gray(34);
    visuals.code_bg_color = Color32::from_gray(60);
    visuals.error_fg_color = Color32::from_rgb(0xFF, 0x6B, 0x6B);
    visuals.indent_has_left_vline = false;

    let widgets = &mut visuals.widgets;
    // Weak text is this at `weak_text_alpha`, which lands on egui's usual grey
    widgets.noninteractive.fg_stroke.color = Color32::from_gray(220);
    widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(74));

    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(3);
    }
}
