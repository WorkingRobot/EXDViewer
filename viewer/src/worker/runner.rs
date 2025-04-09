use exdviewer::worker::{PreservingCodec, SqpackWorker};
use gloo_worker::Registrable;

fn main() {
    eframe::WebLogger::init(log::LevelFilter::Debug).ok();

    log::info!("Starting SqpackWorker");

    SqpackWorker::registrar()
        .encoding::<PreservingCodec>()
        .register();
}
