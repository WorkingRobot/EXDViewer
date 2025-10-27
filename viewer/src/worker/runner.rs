#[cfg(target_arch = "wasm32")]
fn main() {
    use gloo_worker::Registrable;
    use viewer::worker::{PreservingCodec, SqpackWorker};

    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    log::info!("Starting SqpackWorker");

    SqpackWorker::registrar()
        .encoding::<PreservingCodec>()
        .register();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    compile_error!("This runner is only for wasm32");
}
