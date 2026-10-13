use crate::{events::AppEvent, trace::Trace};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::sync::mpsc::Sender;
use std::thread;
use std::{
    io::{Read, Write},
    path::Path,
};
use vt100::Parser;

pub struct Pane {
    parser: Parser,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send>,
    trace: Trace,
    terminal_query_buffer: Vec<u8>,


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
        let child = pair
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
            parser: Parser::new(rows, cols, 10_000),
            writer,
            master: pair.master,
            child,
            trace,
            terminal_query_buffer: Vec::new(),
        })
    }

    pub fn process_output(&mut self, bytes: &[u8]) -> Option<String> {
        let previous_title = self.parser.screen().title().to_owned();
        self.respond_to_terminal_queries(bytes);
        self.parser.process(bytes);
        self.trace.mark("pty-output-parsed");
        let current_title = self.parser.screen().title();
        (current_title != previous_title).then(|| current_title.to_owned())
    }

    fn respond_to_terminal_queries(&mut self, bytes: &[u8]) {
        let mut query_bytes = self.terminal_query_buffer.clone();
        query_bytes.extend_from_slice(bytes);
        if contains_primary_device_attribute_query(&query_bytes) {
            // Report a conservative VT100-compatible device identity. Fish
            // uses this response to enable terminal features safely.
            let _ = self.writer.write_all(b"\x1b[?1;2c");
            let _ = self.writer.flush();
            self.trace.mark("pty-primary-device-attribute-answered");
        }
        let keep_from = query_bytes.len().saturating_sub(3);
        self.terminal_query_buffer = query_bytes[keep_from..].to_vec();
    }

    pub fn poll_exit(&mut self) -> std::io::Result<Option<u32>> {
        self.child
            .try_wait()
            .map(|status| status.map(|status| status.exit_code()))
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

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
    pub fn foreground_process_name(&self) -> Option<String> {
        None
    }

    pub fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.trace.mark("pty-input-written");
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        self.parser.set_size(rows, cols);
        Ok(())
    }
}

fn contains_primary_device_attribute_query(bytes: &[u8]) -> bool {
    bytes.windows(3).any(|window| window == b"\x1b[c")
        || bytes.windows(4).any(|window| window == b"\x1b[0c")
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
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    use super::{contains_primary_device_attribute_query, shell_command_builder};

    #[test]
    fn primary_device_attribute_queries_are_detected() {
        assert!(contains_primary_device_attribute_query(b"prompt\x1b[c"));
        assert!(contains_primary_device_attribute_query(b"\x1b[0c"));
        assert!(!contains_primary_device_attribute_query(b"\x1b[?25h"));
    }

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
        let command = shell_command_builder(
            Path::new("/bin/sh"),
            directory,
            &[("SHELL".to_owned(), "/bin/sh".to_owned())],
        );

        assert_eq!(
            command.get_cwd().map(|cwd| cwd.as_os_str()),
            Some(directory.as_os_str())
        );
    }
}
