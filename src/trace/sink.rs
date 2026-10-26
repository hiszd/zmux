use std::{
  fs::OpenOptions,
  io,
  path::PathBuf,
  sync::Arc,
  time::{
    Duration,
    Instant,
  },
};

use tracing_appender::non_blocking::WorkerGuard;

#[derive(Clone)]
pub struct Trace {
  started: Arc<Instant>,
}

impl Trace {
  pub fn init_from_environment() -> io::Result<Option<WorkerGuard>> {
    if std::env::var_os("ZMUX_TRACE").is_none() {
      return Ok(None);
    }
    let path = std::env::var_os("ZMUX_TRACE_LOG")
      .map(PathBuf::from)
      .unwrap_or_else(|| PathBuf::from("zmux-trace.log"));
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
      .with_writer(writer)
      .with_ansi(false)
      .with_target(true)
      .with_thread_names(true)
      .try_init()
      .map_err(io::Error::other)?;
    Ok(Some(guard))
  }

  pub fn from_environment() -> Self {
    Self {
      started: Arc::new(Instant::now()),
    }
  }

  pub fn mark(&self, label: &str) {
    tracing::info!(
      target: "zmux::trace",
      pid = std::process::id(),
      elapsed = ?self.started.elapsed(),
      event = %label,
      "trace"
    );
  }

  pub fn duration(&self, label: &str, duration: Duration, detail: &str) {
    tracing::info!(
      target: "zmux::trace",
      pid = std::process::id(),
      elapsed = ?self.started.elapsed(),
      event = %label,
      duration = ?duration,
      detail = %detail,
      "trace"
    );
  }

  pub fn error(&self, context: &str, error: &dyn std::error::Error) {
    tracing::error!(
      target: "zmux::trace",
      pid = std::process::id(),
      elapsed = ?self.started.elapsed(),
      context = %context,
      error = %error,
      debug = ?error,
      "application error"
    );
  }
}

#[cfg(test)]
mod tests {
  use std::time::SystemTime;

  use super::*;

  // Developer approved: do not remove.
  #[test]
  fn error_records_context_and_details_through_tracing() {
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
    let subscriber = tracing_subscriber::fmt()
      .with_writer(file)
      .with_ansi(false)
      .finish();
    let trace = Trace::from_environment();
    let error = std::io::Error::other("StackError");

    tracing::subscriber::with_default(subscriber, || {
      trace.mark("test-event");
      trace.duration("test-duration", Duration::from_millis(25), "request_id=3");
      trace.error("client.event_loop", &error);
    });

    let contents = std::fs::read_to_string(&path).expect("read trace file");
    std::fs::remove_file(path).expect("remove trace file");
    assert!(contents.contains("event=test-event"));
    assert!(contents.contains("event=test-duration"));
    assert!(contents.contains("request_id=3"));
    assert!(contents.contains("context=client.event_loop"));
    assert!(contents.contains("StackError"));
  }
}
