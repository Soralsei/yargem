use egui::Color32;
use yargem_lib::{SCREEN_H, SCREEN_W};

use crate::engine::EngineHandle;

const SCREEN_SIZE: egui::Vec2 = egui::vec2(SCREEN_W as f32, SCREEN_H as f32);

pub struct Screen {
    game_tex: egui::TextureHandle,
}

impl Screen {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            game_tex: ctx.load_texture(
                "screen",
                egui::ColorImage::filled([SCREEN_W, SCREEN_H], egui::Color32::BLACK),
                egui::TextureOptions::NEAREST,
            ),
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, engine: Option<&mut EngineHandle>) {
        if let Some(e) = engine {
            if e.frames.update() {
                let pixels = e
                    .frames
                    .output_buffer()
                    .iter()
                    .map(|pixel| {
                        let expand = |c: u16| ((c << 3) | (c >> 2)) as u8;
                        let r = pixel & 0x1F;
                        let g = pixel >> 5 & 0x1F;
                        let b = (pixel >> 10) & 0x1F;
                        Color32::from_rgb(expand(r), expand(g), expand(b))
                    })
                    .collect::<Vec<egui::Color32>>();
                let image = egui::ColorImage::new([SCREEN_W, SCREEN_H], pixels);
                self.game_tex.set(image, egui::TextureOptions::NEAREST);
            }
        } else {
            let image = egui::ColorImage::filled([SCREEN_W, SCREEN_H], egui::Color32::BLACK);
            self.game_tex.set(image, egui::TextureOptions::NEAREST);
        }

        let scale = (ui.available_size() / SCREEN_SIZE)
            .min_elem()
            .floor()
            .max(1.0);
        ui.centered_and_justified(|ui| {
            ui.add(egui::Image::from_texture(&self.game_tex).fit_to_exact_size(SCREEN_SIZE * scale))
        });
    }
}
