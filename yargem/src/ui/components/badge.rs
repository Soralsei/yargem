pub enum BadgeType {
    Outline(egui::Color32),
    Filled(egui::Color32),
}

pub fn badge(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>, badge_type: BadgeType) {
    let frame = match badge_type {
        BadgeType::Outline(color) => egui::Frame::new().stroke(egui::Stroke::new(1.0, color)),
        BadgeType::Filled(color) => egui::Frame::new().fill(color),
    }
    .corner_radius(u8::MAX)
    .inner_margin(ui.spacing().button_padding);
    egui::AtomLayout::new(label).frame(frame).show(ui);
}
