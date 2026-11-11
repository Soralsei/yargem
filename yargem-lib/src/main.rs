use yargem_lib::system::{Settings, System};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let f_rom = std::fs::read(&args[1]);
    let rom = match f_rom {
        Ok(r) => r,
        Err(e) => panic!("Unable to open file {}: {}", args[1], e),
    };
    let boot_rom = args.get(2).and_then(|path| std::fs::read(&path).ok());
    let mut sys = System::new(boot_rom, rom, Settings::default()).unwrap();
    loop {
        let _ = sys.run_frame();
    }
}
