use gloo_worker::Registrable;
use viewer::worker::{PreservingCodec, SqpackWorker};

fn main() {
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    log::info!("Starting SqpackWorker");

    SqpackWorker::registrar()
        .encoding::<PreservingCodec>()
        .register();
}
