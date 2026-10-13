mod binding_trie;
mod client;
mod events;
mod input;
mod key_notation;
mod lua;
mod pane;
mod server;
mod trace;
mod ui;
mod which_key;

use std::{
  io,
  path::PathBuf,
  process::{
    Command as ProcessCommand,
    Stdio,
  },
  thread,
  time::Duration,
};

use zmux::{
  protocol::{AdminRequest, AdminResponse, ClientMessage, ServerMessage, read_frame, write_frame},
  transport::{self, Endpoint},
};

enum Command {
  New { attach: Option<u64> },
  Attach(u64),
  List,
  Kill(u64),
  ServerStop,
  ServerStatus,
}

struct CliOptions {
  command: Command,
  socket_instance: String,
  server_process: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let options = parse_args(std::env::args().skip(1))?;
  let endpoint = Endpoint::new(Some(&options.socket_instance))?;
  if options.server_process {
    return server::run(endpoint);
  }

  match options.command {
    Command::New { attach } => {
      ensure_server(&endpoint)?;
      client::run(endpoint, attach, attach.is_some(), configuration())
    }
    Command::Attach(session_number) => {
      ensure_server(&endpoint)?;
      client::run(endpoint, Some(session_number), false, configuration())
    }
    command => run_admin_command(endpoint, command),
  }
}

fn parse_args(mut arguments: impl Iterator<Item = String>) -> Result<CliOptions, io::Error> {
  let Some(command) = arguments.next() else {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "a command is required: use 'new', 'attach', 'list', 'kill', or 'server'",
    ));
  };
  if command == "--_server" {
    let socket_instance = arguments.next().ok_or_else(|| {
      io::Error::new(io::ErrorKind::InvalidInput, "server instance is required")
    })?;
    return Ok(CliOptions {
      command: Command::New { attach: None },
      socket_instance,
      server_process: true,
    });
  }
  if command == "--help" || command == "-h" {
    print_help();
    std::process::exit(0);
  }

  let mut socket_instance = "default".to_owned();
  let command = match command.as_str() {
    "new" => {
      let mut attach = None;
      while let Some(argument) = arguments.next() {
        match argument.as_str() {
          "--attach" => {
            let value = arguments.next().ok_or_else(|| {
              io::Error::new(io::ErrorKind::InvalidInput, "--attach requires a session number")
            })?;
            attach = Some(parse_session_number(&value)?);
          }
          "--socket" => {
            socket_instance = arguments.next().ok_or_else(|| {
              io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name")
            })?;
          }
          "--help" | "-h" => {
            print_help();
            std::process::exit(0);
          }
          other => return Err(invalid_argument(other)),
        }
      }
      Command::New { attach }
    }
    "attach" => {
      let value = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "attach requires a session number")
      })?;
      let session_number = parse_session_number(&value)?;
      while let Some(argument) = arguments.next() {
        match argument.as_str() {
          "--socket" => {
            socket_instance = arguments.next().ok_or_else(|| {
              io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name")
            })?;
          }
          "--help" | "-h" => {
            print_help();
            std::process::exit(0);
          }
          other => return Err(invalid_argument(other)),
        }
      }
      Command::Attach(session_number)
    }
    "list" => {
      reject_command_arguments(&mut arguments, "list", &mut socket_instance)?;
      Command::List
    }
    "kill" => {
      let value = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "kill requires a session number")
      })?;
      let session_number = parse_session_number(&value)?;
      reject_command_arguments(&mut arguments, "kill", &mut socket_instance)?;
      Command::Kill(session_number)
    }
    "server" => {
      let subcommand = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "server requires stop or status")
      })?;
      let command = match subcommand.as_str() {
        "stop" => Command::ServerStop,
        "status" => Command::ServerStatus,
        other => return Err(invalid_argument(other)),
      };
      reject_command_arguments(&mut arguments, "server", &mut socket_instance)?;
      command
    }
    other => return Err(invalid_argument(other)),
  };
  Ok(CliOptions {
    command,
    socket_instance,
    server_process: false,
  })
}

fn reject_command_arguments(
  arguments: &mut impl Iterator<Item = String>,
  command: &str,
  socket_instance: &mut String,
) -> Result<(), io::Error> {
  while let Some(argument) = arguments.next() {
    if argument == "--socket" {
      *socket_instance = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name")
      })?;
    } else {
      return Err(invalid_argument(argument.as_str()));
    }
  }
  let _ = command;
  Ok(())
}

fn parse_session_number(value: &str) -> Result<u64, io::Error> {
  let number = value.parse::<u64>().map_err(|_| {
    io::Error::new(io::ErrorKind::InvalidInput, "Session Number must be an integer")
  })?;
  if number == 0 {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "Session Numbers start at 1",
    ));
  }
  Ok(number)
}

fn invalid_argument(argument: &str) -> io::Error {
  io::Error::new(
    io::ErrorKind::InvalidInput,
    format!("unknown argument or command: {argument}"),
  )
}

fn print_help() {
  println!("Usage:");
  println!("  zmux new [--attach SESSION] [--socket NAME]");
  println!("  zmux attach SESSION [--socket NAME]");
  println!("  zmux list [--socket NAME]");
  println!("  zmux kill SESSION [--socket NAME]");
  println!("  zmux server stop [--socket NAME]");
  println!("  zmux server status [--socket NAME]");
  println!();
  println!("new creates a session; --attach attaches to SESSION or creates it if missing.");
  println!("attach requires SESSION to already exist.");
}

fn run_admin_command(
  endpoint: Endpoint,
  command: Command,
) -> Result<(), Box<dyn std::error::Error>> {
  let is_status = matches!(&command, Command::ServerStatus);
  let request = match command {
    Command::List => AdminRequest::ListSessions,
    Command::Kill(number) => AdminRequest::KillSession(number),
    Command::ServerStop => AdminRequest::StopServer,
    Command::ServerStatus => AdminRequest::Status,
    Command::New { .. } | Command::Attach(_) => unreachable!(),
  };
  let mut stream = match transport::connect(&endpoint) {
    Ok(stream) => stream,
    Err(_error) if is_status => {
      println!("stopped");
      return Ok(());
    }
    Err(error) => {
      return Err(format!(
        "could not connect to server for '{}': {error}",
        endpoint.instance()
      )
      .into());
    }
  };
  write_frame(&mut stream, &ClientMessage::Admin(request))?;
  let ServerMessage::AdminResult { result } = read_frame(&mut stream)? else {
    return Err("server returned an unexpected administrative response".into());
  };
  match result? {
    AdminResponse::Sessions(sessions) => {
      if sessions.is_empty() {
        println!("no sessions");
      } else {
        println!("SESSION  CLIENTS  WORKSPACES  TABS");
        for session in sessions {
          println!(
            "{:<8} {:<8} {:<10} {}",
            session.number, session.clients, session.workspaces, session.tabs
          );
        }
      }
    }
    AdminResponse::Status { running, sessions } => {
      println!("{} ({} sessions)", if running { "running" } else { "stopped" }, sessions);
    }
    AdminResponse::Success => println!("ok"),
  }
  Ok(())
}

fn ensure_server(endpoint: &Endpoint) -> Result<(), Box<dyn std::error::Error>> {
  if let Ok(stream) = transport::connect(endpoint) {
    drop(stream);
    return Ok(());
  }

  let executable = std::env::current_exe()?;
  let mut command = ProcessCommand::new(executable);
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
  Err(
    last_error
      .map(|error| Box::new(error) as Box<dyn std::error::Error>)
      .unwrap_or_else(|| "background Server did not start".into()),
  )
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
  use super::{Command, parse_args};

  #[test]
  fn cli_requires_a_command() {
    assert!(parse_args(std::iter::empty()).is_err());
  }

  #[test]
  fn new_can_create_or_attach_a_numbered_session() {
    let options = parse_args(
      ["new", "--attach", "12", "--socket", "project"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid new command");

    assert!(matches!(options.command, Command::New { attach: Some(12) }));
    assert_eq!(options.socket_instance, "project");
  }

  #[test]
  fn attach_requires_an_existing_session_target() {
    let options = parse_args(
      ["attach", "12", "--socket", "project"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid attach command");

    assert!(matches!(options.command, Command::Attach(12)));
    assert_eq!(options.socket_instance, "project");
  }

  #[test]
  fn cli_rejects_invalid_session_numbers_and_arguments() {
    assert!(parse_args(["new", "--attach", "0"].map(str::to_owned).into_iter()).is_err());
    assert!(parse_args(["attach", "abc"].map(str::to_owned).into_iter()).is_err());
    assert!(parse_args(["new", "--socket"].map(str::to_owned).into_iter()).is_err());
  }
}
