use log::{Level, Metadata, Record};
use tokio::sync::mpsc;

pub struct AppLogger {
    sender: mpsc::UnboundedSender<String>,
}

impl AppLogger {
    pub fn new(sender: mpsc::UnboundedSender<String>) -> Self {
        Self { sender }
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let log_entry = format!("[{}] {}", record.level(), record.args());
            let _ = self.sender.send(log_entry);
        }
    }

    fn flush(&self) {}
}

pub fn init(sender: mpsc::UnboundedSender<String>) -> Result<(), log::SetLoggerError> {
    let logger = AppLogger::new(sender);
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
