use minifb::{Key, Scale, ScaleMode, Window, WindowOptions};
use yargem::audio::manager::AudioManager;
use yargem::engine::{spawn_engine, EngineCommand};
use yargem_lib::system::Settings;

use yargem_lib::input::Button;
use yargem_lib::{SCREEN_H, SCREEN_W};

const DMG: [u32; 4] = [0xFFE0F8D0, 0xFF88C070, 0xFF346856, 0xFF081820];

const KEYMAP: [(Key, Button); 8] = [
    (Key::Z, Button::A),
    (Key::X, Button::B),
    (Key::Backspace, Button::Select),
    (Key::Enter, Button::Start),
    (Key::Right, Button::Right),
    (Key::Left, Button::Left),
    (Key::Up, Button::Up),
    (Key::Down, Button::Down),
];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let rom = std::fs::read(&args[1]).expect("rom");
    let boot = args.get(2).and_then(|p| std::fs::read(p).ok());

    let mut win = Window::new(
        "yargem",
        SCREEN_W,
        SCREEN_H,
        WindowOptions {
            scale: Scale::X4,
            scale_mode: ScaleMode::AspectRatioStretch,
            resize: true,
            ..Default::default()
        },
    )
    .expect("window creation shouldn't fail");
    win.set_target_fps(60);

    let mut audio = AudioManager::new();
    let mut engine = spawn_engine(boot, rom, Settings::default());

    let sink = audio.open(audio.default_device())?;
    engine.send(EngineCommand::AudioSink(sink));

    let mut buf = [0u32; SCREEN_W * SCREEN_H];

    while win.is_open() && !win.is_key_down(Key::Escape) {
        if audio.failed() {
            engine.send(EngineCommand::AudioSink(
                audio.open(audio.default_device())?,
            ));
        }

        KEYMAP.iter().for_each(|(key, button)| {
            engine.send(EngineCommand::Button(*button, win.is_key_down(*key)));
        });

        let frame = engine.frames.read();

        for (px, &i) in buf.iter_mut().zip(frame.iter()) {
            *px = DMG[(i & 3) as usize];
        }
        win.update_with_buffer(&buf, SCREEN_W, SCREEN_H).unwrap();
    }

    Ok(())
}
