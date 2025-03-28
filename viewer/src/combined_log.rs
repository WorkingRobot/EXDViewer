use log::{Log, Metadata, Record};

pub struct CombinedLogger<L1: Log + 'static, L2: Log + 'static>(pub L1, pub L2);

impl<L1: Log + 'static, L2: Log + 'static> CombinedLogger<L1, L2> {
    pub fn init(self) {
        log::set_boxed_logger(Box::new(self)).expect("Failed to set logger");
    }
}

impl<L1: Log + 'static, L2: Log + 'static> Log for CombinedLogger<L1, L2> {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.0.enabled(metadata) || self.1.enabled(metadata)
    }

    fn log(&self, record: &Record<'_>) {
        self.0.log(record);
        self.1.log(record);
    }

    fn flush(&self) {
        self.0.flush();
        self.1.flush();
    }
}
