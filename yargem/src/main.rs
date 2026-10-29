use yargem::app::App;

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "Yargem",
        eframe::NativeOptions::default(),
        Box::new(|ctx| Ok(Box::new(App::new(ctx)))),
    )
}
