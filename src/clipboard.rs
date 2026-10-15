use std::{
  io,
  process::{
    Command,
    Stdio,
  },
};

pub trait ClipboardProvider {
  fn read(&self) -> io::Result<String>;
  fn write(&self, text: &str) -> io::Result<()>;
}

#[derive(Clone, Copy, Default)]
pub struct SystemClipboard;

impl ClipboardProvider for SystemClipboard {
  fn read(&self) -> io::Result<String> {
    #[cfg(windows)]
    let output = Command::new("powershell")
      .args(["-NoProfile", "-Command", "Get-Clipboard"])
      .output()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let output = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
      Command::new("wl-paste").output()?
    } else {
      Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()?
    };
    #[cfg(target_os = "macos")]
    let output = Command::new("pbpaste").output()?;
    if output.status.success() {
      String::from_utf8(output.stdout)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    } else {
      Err(io::Error::other("clipboard read command failed"))
    }
  }

  fn write(&self, text: &str) -> io::Result<()> {
    #[cfg(windows)]
    let mut command = Command::new("clip");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
      Command::new("wl-copy")
    } else {
      let mut command = Command::new("xclip");
      command.args(["-selection", "clipboard"]);
      command
    };
    #[cfg(target_os = "macos")]
    let mut command = Command::new("pbcopy");

    let mut child = command.stdin(Stdio::piped()).spawn()?;
    let mut stdin = child
      .stdin
      .take()
      .ok_or_else(|| io::Error::other("clipboard stdin unavailable"))?;
    stdin.write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() {
      Ok(())
    } else {
      Err(io::Error::other("clipboard command failed"))
    }
  }
}

use std::io::Write;
