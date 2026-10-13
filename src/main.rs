mod binding_trie;
mod client;
mod events;
mod input;
mod key_notation;
mod lua;
mod overlay;
mod pane;
mod server;
mod trace;
mod ui;
mod which_key;

use std::{
    io,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use zmux::transport::{self, Endpoint};

struct CliOptions {
    session_number: Option<u64>,
    socket_instance: String,
    server_process: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_args(std::env::args().skip(1))?;
    let endpoint = Endpoint::new(Some(&options.socket_instance))?;
    if options.server_process {
        return server::run(endpoint);
    }

    ensure_server(&endpoint)?;
    let config = configuration();
    client::run(endpoint, options.session_number, config)
}

fn parse_args(mut arguments: impl Iterator<Item = String>) -> Result<CliOptions, io::Error> {
    let mut session_number = None;
    let mut socket_instance = "default".to_owned();
    let mut server_process = false;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--_server" => {
                server_process = true;
                if let Some(instance) = arguments.next() {
                    socket_instance = instance;
                }
            }
            "--session" => {
                let value = arguments.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--session requires a number")
                })?;
                let number = value.parse::<u64>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Session Number must be an integer",
                    )
                })?;
                if number == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Session Numbers start at 1",
                    ));
                }
                session_number = Some(number);
            }
            "--socket" => {
                socket_instance = arguments.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name")
                })?;
            }
            "--help" | "-h" => {
                println!("Usage: zmux [--session N] [--socket NAME]");
                println!("Without --session, create a new Session.");
                println!("With --session N, attach to an existing Session.");
                std::process::exit(0);
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other}"),
                ));
            }
        }
    }
    Ok(CliOptions {
        session_number,
        socket_instance,
        server_process,
    })
}

fn ensure_server(endpoint: &Endpoint) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(stream) = transport::connect(endpoint) {
        drop(stream);
        return Ok(());
    }

    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("--_server")
        .arg(endpoint.instance())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x00000008 | 0x08000000);
    }
    let _child = command.spawn()?;

    let mut last_error = None;
    for _ in 0..100 {
        match transport::connect(endpoint) {
            Ok(stream) => {
                drop(stream);
                return Ok(());
            }
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(last_error
        .map(|error| Box::new(error) as Box<dyn std::error::Error>)
        .unwrap_or_else(|| "background Server did not start".into()))
}

fn configuration() -> lua::AppConfig {
    let path = std::env::var_os("ZMUX_CONFIG")
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from("zmux.lua")));
    let Some(path) = path else {
        return lua::AppConfig::default();
    };
    if !path.exists() {
        return lua::AppConfig::default();
    }
    match lua::load_file(&path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("could not load {}: {error}", path.display());
            lua::AppConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    #[test]
    fn cli_without_session_target_creates_a_new_session() {
        let options = parse_args(std::iter::empty()).expect("valid default invocation");

        assert_eq!(options.session_number, None);
        assert_eq!(options.socket_instance, "default");
        assert!(!options.server_process);
    }

    #[test]
    fn cli_can_target_a_numbered_session_and_named_server() {
        let options = parse_args(
            ["--session", "12", "--socket", "project"]
                .map(str::to_owned)
                .into_iter(),
        )
        .expect("valid explicit attachment");

        assert_eq!(options.session_number, Some(12));
        assert_eq!(options.socket_instance, "project");
    }

    #[test]
    fn cli_rejects_invalid_session_numbers_and_socket_arguments() {
        assert!(parse_args(["--session", "0"].map(str::to_owned).into_iter()).is_err());
        assert!(parse_args(["--session", "abc"].map(str::to_owned).into_iter()).is_err());
        assert!(parse_args(["--socket"].map(str::to_owned).into_iter()).is_err());
    }
}
