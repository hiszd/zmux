use std::{
  fs::OpenOptions,
  io::{
    BufWriter,
    Write,
  },
  path::PathBuf,
  sync::{
    Arc,
    Mutex,
  },
  time::{
    Duration,
    Instant,
  },
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

  pub fn mark(&self, label: &str) { self.write_line(format_args!("{label}")); }

  pub fn duration(&self, label: &str, duration: Duration, detail: &str) {
    self.write_line(format_args!("{label} duration={duration:?} {detail}"));
  }

  pub fn error(&self, context: &str, error: &dyn std::error::Error) {
    self
      .mark(&format!("error context={context:?} message={:?} debug={error:?}", error.to_string()));
    self.flush();
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

#[cfg(test)]
mod tests {
  use std::time::SystemTime;

  use super::*;

  #[test]
  fn error_records_context_and_flushes_details() {
    let unique = SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .expect("system clock after Unix epoch")
      .as_nanos();
    let path =
      std::env::temp_dir().join(format!("zmux-trace-test-{}-{unique}.log", std::process::id()));
    let file = OpenOptions::new()
      .create_new(true)
      .write(true)
      .open(&path)
      .expect("create trace file");
    let trace = Trace {
      started: Arc::new(Instant::now()),
      log: Some(Arc::new(Mutex::new(BufWriter::new(file)))),
    };
    let error = std::io::Error::other("StackError");

    trace.error("client.event_loop", &error);

    let contents = std::fs::read_to_string(&path).expect("read flushed trace file");
    std::fs::remove_file(path).expect("remove trace file");
    assert!(contents.contains("error context=\"client.event_loop\""));
    assert!(contents.contains("StackError"));
  }
}
