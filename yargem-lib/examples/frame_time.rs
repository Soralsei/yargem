use std::time::Instant;

use yargem_lib::system::System;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let rom = std::fs::read(&args[1]).expect("rom");
    let frames: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(120);

    let mut sys = System::new(None, rom, false);
    let mut frame_times = Vec::with_capacity(frames);
    for _ in 0..frames {
        let t1 = Instant::now();
        sys.run_frame();
        frame_times.push((Instant::now() - t1).as_nanos() as f64 / 1_000_000.0)
    }

    println!(
        "Average frame time : {:.2} ms, min: {:.2}, max: {:.2}",
        frame_times.iter().sum::<f64>() / frames as f64,
        frame_times.iter().min_by(|a, b| a.total_cmp(b)).unwrap(),
        frame_times.iter().max_by(|a, b| a.total_cmp(b)).unwrap()
    )
}
