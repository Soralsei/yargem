fn dot_impl(ui: &mut egui::Ui, color: egui::Color32, radius: Option<f32>) -> egui::Response {
    let h = ui.available_height();

    let (rect, response) = ui.allocate_exact_size(egui::vec2(h * 0.6, h), egui::Sense::hover());

    ui.painter()
        .circle_filled(rect.center(), radius.unwrap_or(h * 0.2), color);

    response
}

pub fn dot_sized(color: egui::Color32, size: f32) -> impl egui::Widget {
    move |ui: &mut egui::Ui| dot_impl(ui, color, Some(size))
}

pub fn dot(color: egui::Color32) -> impl egui::Widget {
    move |ui: &mut egui::Ui| dot_impl(ui, color, None)
}
