use fasthash::XXHasher;
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::{fs, hash::Hash};

use yargem_lib::{
    input::Button,
    system::{Settings, System},
};

const BASELINE_PATH: &str = "tests/fixtures/";

fn dump_rgb555_ppm(path: &std::path::Path, fb: &[u16]) {
    // CGB native resolution: 160x144
    let mut out = Vec::with_capacity(32 + fb.len() * 3);
    out.extend_from_slice(b"P6\n160 144\n255\n");

    for &px in fb {
        let r5 = (px & 0x1F) as u8;
        let g5 = ((px >> 5) & 0x1F) as u8;
        let b5 = ((px >> 10) & 0x1F) as u8;

        out.push((r5 << 3) | (r5 >> 2));
        out.push((g5 << 3) | (g5 >> 2));
        out.push((b5 << 3) | (b5 >> 2));
    }

    std::fs::write(path, out).expect("failed to write debug PPM");
}

#[test]
fn verify_frame_hashes() {
    let Ok(rom_file) = std::env::var("ROM_PATH") else {
        eprintln!("verify_frame_hashes needs ROM_PATH env");
        return;
    };
    let rom_path = Path::new(&rom_file);
    let baseline_path = std::env::var("BASELINE_PATH").unwrap_or(BASELINE_PATH.to_string());

    let rom = fs::read(rom_path).expect("missing test ROM");
    let mut emu = System::new(None, rom, Settings::default()).unwrap();

    let mut actual_hashes = Vec::with_capacity(1000);

    let mut hasher = XXHasher::default();

    for frame in 0..1000 {
        // Optional: inject deterministic controller inputs on specific frames
        if frame == 60 {
            emu.set_button(Button::Start, true);
        } else if frame == 65 {
            emu.set_button(Button::Start, false);
        }

        emu.run_frame();

        // Hash the raw framebuffer slice directly in memory—zero copies or disk I/O
        let fb_bytes = emu.get_framebuffer();
        fb_bytes.hash(&mut hasher);
        let hash = hasher.finish();

        actual_hashes.push(format!("{frame:04} {hash:X}"));
    }

    let baseline_path = Path::new(&baseline_path).join(
        rom_path
            .with_file_name(format!(
                "1000_frames_{}",
                rom_path.file_name().unwrap().to_str().unwrap()
            ))
            .with_extension("txt")
            .file_name()
            .unwrap(),
    );

    if std::env::var("RECORD_BASELINE").as_deref() == Ok("1") {
        if let Some(parent) = baseline_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(baseline_path, actual_hashes.join("\n") + "\n")
            .expect("failed to write baseline");
        println!("Recorded {} frames to {BASELINE_PATH}", actual_hashes.len());
        return;
    }
    let expected_raw = fs::read_to_string(baseline_path)
        .expect("baseline missing — run with RECORD_BASELINE=1 first");
    let expected_hashes: Vec<&str> = expected_raw.lines().collect();

    assert_eq!(
        actual_hashes.len(),
        expected_hashes.len(),
        "frame count mismatch"
    );

    for (frame, (actual, expected)) in actual_hashes.iter().zip(expected_hashes.iter()).enumerate()
    {
        assert_eq!(
            actual,
            expected,
            "Framebuffer diverged at frame {frame} (registers : {})",
            emu.registers().0
        );
    }
}
