use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Trace {
    started: Arc<Instant>,
    log: Option<Arc<Mutex<BufWriter<std::fs::File>>>>,
}

impl Trace {
    pub fn from_environment() -> Self {
        let log = std::env::var_os("ZMUX_TRACE").and_then(|_| {
            let path = std::env::var_os("ZMUX_TRACE_LOG")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("zmux-trace.log"));
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(|file| Arc::new(Mutex::new(BufWriter::new(file))))
        });
        Self {
            started: Arc::new(Instant::now()),
            log,
        }
    }

    pub fn mark(&self, label: &str) {
        self.write_line(format_args!("{label}"));
    }

    pub fn duration(&self, label: &str, duration: Duration, detail: &str) {
        self.write_line(format_args!("{label} duration={duration:?} {detail}"));
    }

    pub fn flush(&self) {
        if let Some(log) = &self.log
            && let Ok(mut log) = log.lock()
        {
            let _ = log.flush();
        }
    }

    fn write_line(&self, message: std::fmt::Arguments<'_>) {
        let Some(log) = &self.log else {
            return;
        };
        if let Ok(mut log) = log.lock() {
            let _ = writeln!(
                log,
                "[zmux-trace pid={} +{:?}] {message}",
                std::process::id(),
                self.started.elapsed()
            );
        }
    }
}
