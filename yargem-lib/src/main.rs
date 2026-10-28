use yargem_lib::system::{Settings, System};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let f_rom = std::fs::read(&args[1]);
    let rom = match f_rom {
        Ok(r) => r,
        Err(e) => panic!("Unable to open file {}: {}", args[1], e),
    };
    let mut boot_rom = None;
    if args.len() > 2 {
        let f_boot_rom = std::fs::read(&args[2]);
        boot_rom = f_boot_rom.ok();
    }
    let mut sys = System::new(boot_rom, rom, Settings::default());
    loop {
        let _ = sys.run_frame();
    }
}
