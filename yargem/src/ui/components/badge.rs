pub enum BadgeType {
    Outline(egui::Color32),
    Filled(egui::Color32),
}

pub fn badge(
    ui: &mut egui::Ui,
    label: impl Into<egui::WidgetText>,
    badge_type: BadgeType,
    inner_margin: impl Into<egui::Margin>,
) {
    match badge_type {
        BadgeType::Outline(color) => egui::Frame::new().stroke(egui::Stroke::new(1.0, color)),
        BadgeType::Filled(color) => egui::Frame::new().fill(color),
    }
    .corner_radius(ui.available_height() / 2.0)
    .inner_margin(inner_margin)
    .show(ui, |ui| {
        ui.label(label);
    });
}
