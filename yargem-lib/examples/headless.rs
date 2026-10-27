use yargem_lib::system::{Settings, System};
use yargem_lib::{SCREEN_H, SCREEN_W};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rom = std::fs::read(&args[1]).expect("rom");
    let frames: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);

    let mut sys = System::new(None, rom, Settings::default());
    for _ in 0..frames {
        sys.run_frame();
    }

    let fb = sys.get_framebuffer();
    let chars = [' ', '.', '+', '#'];
    let mut hist = [0usize; 4];
    for y in 0..SCREEN_H {
        let mut line = String::new();
        for x in 0..SCREEN_W {
            let p = (fb[y * SCREEN_W + x] & 3) as usize;
            hist[p] += 1;
            line.push(chars[p]);
        }
        println!("{}", line);
    }
    eprintln!("histogram: {:?}", hist);
}
