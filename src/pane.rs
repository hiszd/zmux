use crate::{events::AppEvent, trace::Trace};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::sync::mpsc::Sender;
use std::thread;
use std::{
    ffi::OsString,
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
}

impl Pane {
    pub fn shell(
        rows: u16,
        cols: u16,
        tab_id: u64,
        working_directory: &Path,
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
            .spawn_command(shell_command_builder(working_directory))?;
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
        })
    }

    pub fn process_output(&mut self, bytes: &[u8]) -> Option<String> {
        let previous_title = self.parser.screen().title().to_owned();
        self.parser.process(bytes);
        self.trace.mark("pty-output-parsed");
        let current_title = self.parser.screen().title();
        (current_title != previous_title).then(|| current_title.to_owned())
    }

    pub fn poll_exit(&mut self) -> std::io::Result<Option<u32>> {
        self.child
            .try_wait()
            .map(|status| status.map(|status| status.exit_code()))
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
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

fn shell_command_builder(working_directory: &Path) -> CommandBuilder {
    #[cfg(windows)]
    let shell = std::env::var_os("COMSPEC").unwrap_or_else(|| OsString::from("cmd.exe"));
    #[cfg(not(windows))]
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("sh"));

    let mut command = CommandBuilder::new(shell);
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
    use std::path::Path;

    use super::shell_command_builder;

    #[test]
    fn shell_command_starts_in_the_requested_working_directory() {
        let directory = Path::new("/workspace/project");
        let command = shell_command_builder(directory);

        assert_eq!(
            command.get_cwd().map(|cwd| cwd.as_os_str()),
            Some(directory.as_os_str())
        );
    }
}
