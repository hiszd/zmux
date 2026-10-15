use std::{
  io::{
    Read,
    Write,
  },
  path::Path,
  sync::mpsc::Sender,
  thread,
};

use portable_pty::{
  Child,
  CommandBuilder,
  MasterPty,
  PtySize,
  native_pty_system,
};

use crate::{
  events::AppEvent,
  terminal_emulator::{
    EmulatorEvents,
    TerminalEmulator,
  },
  trace::Trace,
};

const MAX_REPLAY_BYTES: usize = 8 * 1024 * 1024;

pub struct Pane {
  emulator: TerminalEmulator,
  replay: Vec<u8>,
  writer: Box<dyn Write + Send>,
  master: Box<dyn MasterPty + Send>,
  child: Box<dyn Child + Send>,
  trace: Trace,
}

impl Pane {
  #[allow(clippy::too_many_arguments)]
  pub fn shell(
    rows: u16,
    cols: u16,
    tab_id: u64,
    working_directory: &Path,
    shell: &Path,
    environment: &[(String, String)],
    events: Sender<AppEvent>,
    trace: Trace,
  ) -> Result<Self, Box<dyn std::error::Error>> {
    let system = native_pty_system();
    let pair = system.openpty(PtySize {
      rows,
      cols,
      pixel_width: 0,
      pixel_height: 0,
    })?;
    let child =
      pair
        .slave
        .spawn_command(shell_command_builder(shell, working_directory, environment))?;
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let reader_trace = trace.clone();
    thread::Builder::new()
      .name("zmux-pty-reader".to_owned())
      .spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
          match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(size) => {
              reader_trace.mark("pty-output-received");
              if events
                .send(AppEvent::PtyOutput {
                  tab_id,
                  bytes: buffer[..size].to_vec(),
                })
                .is_err()
              {
                break;
              }
            }
          }
        }
      })?;

    Ok(Self {
      emulator: TerminalEmulator::new(rows, cols),
      replay: Vec::new(),
      writer,
      master: pair.master,
      child,
      trace,
    })
  }

  pub fn process_output(&mut self, bytes: &[u8]) -> std::io::Result<EmulatorEvents> {
    self.replay.extend_from_slice(bytes);
    let events = self.emulator.process(bytes, &mut self.writer)?;
    if self.replay.len() > MAX_REPLAY_BYTES {
      self.replay = self.emulator.snapshot_replay();
      self.replay.truncate(MAX_REPLAY_BYTES);
    }
    self.writer.flush()?;
    self.trace.mark("pty-output-parsed");
    Ok(events)
  }

  pub fn poll_exit(&mut self) -> std::io::Result<Option<u32>> {
    self
      .child
      .try_wait()
      .map(|status| status.map(|status| status.exit_code()))
  }

  pub fn size(&self) -> (u16, u16) { self.emulator.size() }

  pub fn replay(&self) -> &[u8] { &self.replay }

  /// Returns the executable name of the process group currently attached to
  /// this PTY, when the host platform exposes that information.
  ///
  /// On Linux, the PTY foreground process group identifies the application
  /// currently using the terminal: normally the shell while idle and an
  /// interactive program such as `nvim` while it is running.
  #[cfg(target_os = "linux")]
  pub fn foreground_process_name(&self) -> Option<String> {
    let process_group = self.master.process_group_leader()?;
    let name = std::fs::read_to_string(format!("/proc/{process_group}/comm")).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
  }

  /// Process inspection is unavailable on platforms without the Linux `/proc`
  /// process-group interface; terminal-reported titles remain the fallback.
  #[cfg(not(target_os = "linux"))]
  pub fn foreground_process_name(&self) -> Option<String> { None }

  pub fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
    self.writer.write_all(bytes)?;
    self.trace.mark("pty-input-written");
    Ok(())
  }

  pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
    self
      .master
      .resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
      })
      .map_err(|error| std::io::Error::other(error.to_string()))?;
    self.emulator.resize(rows, cols);
    Ok(())
  }
}

fn shell_command_builder(
  shell: &Path,
  working_directory: &Path,
  environment: &[(String, String)],
) -> CommandBuilder {
  let mut command = CommandBuilder::new(shell);
  for (key, value) in environment {
    command.env(key, value);
  }
  #[cfg(not(windows))]
  command.arg("-il");
  command.cwd(working_directory);
  command
}

impl Drop for Pane {
  fn drop(&mut self) { let _ = self.child.kill(); }
}

#[cfg(test)]
mod tests {
  use std::{
    ffi::OsStr,
    path::Path,
  };

  use super::shell_command_builder;

  #[cfg(not(windows))]
  #[test]
  fn shell_command_uses_requested_shell_environment_and_login_mode() {
    let command = shell_command_builder(
      Path::new("/usr/bin/fish"),
      Path::new("/workspace/project"),
      &[("SHELL".to_owned(), "/usr/bin/fish".to_owned())],
    );

    assert_eq!(command.get_argv()[0], OsStr::new("/usr/bin/fish"));
    assert_eq!(command.get_argv()[1], OsStr::new("-il"));
    assert_eq!(command.get_env("SHELL"), Some(OsStr::new("/usr/bin/fish")));
    assert_eq!(
      command.get_cwd().map(|cwd| cwd.as_os_str()),
      Some(Path::new("/workspace/project").as_os_str())
    );
  }

  #[test]
  fn shell_command_starts_in_the_requested_working_directory() {
    let directory = Path::new("/workspace/project");
    let command = shell_command_builder(Path::new("/bin/sh"), directory, &[(
      "SHELL".to_owned(),
      "/bin/sh".to_owned(),
    )]);

    assert_eq!(command.get_cwd().map(|cwd| cwd.as_os_str()), Some(directory.as_os_str()));
  }
}
