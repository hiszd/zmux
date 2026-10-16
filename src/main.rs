mod client;
mod clipboard;
mod events;
mod input;
mod lua;
mod pane;
mod server;
mod terminal_emulator;
mod trace;
mod ui;

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
  protocol::{
    AdminRequest,
    AdminResponse,
    ClientMessage,
    ServerMessage,
    read_frame,
    write_frame,
  },
  transport::{
    self,
    Endpoint,
  },
};

enum Command {
  New {
    session_name: Option<String>,
    strict: bool,
    path: Option<PathBuf>,
  },
  Attach(String),
  List {
    attach: bool,
  },
  Kill(String),
  ServerStop,
  ServerStatus,
}

struct CliOptions {
  command: Command,
  socket_instance: String,
  server_process: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let arguments: Vec<String> = std::env::args().skip(1).collect();
  let options = parse_args(arguments.clone().into_iter())?;
  let endpoint = Endpoint::new(Some(&options.socket_instance))?;
  if options.server_process {
    return server::run(endpoint);
  }

  match options.command {
    Command::New {
      session_name,
      strict,
      path,
    } => launch_client(&endpoint, &arguments, lua::LaunchState {
      command: "new".to_owned(),
      session_name,
      create_if_missing: true,
      fail_if_exists: strict,
      require_session: false,
      working_directory: path.unwrap_or(std::env::current_dir()?),
    }),
    Command::Attach(session_name) => launch_client(&endpoint, &arguments, lua::LaunchState {
      command: "attach".to_owned(),
      session_name: Some(session_name),
      create_if_missing: false,
      fail_if_exists: false,
      require_session: true,
      working_directory: std::env::current_dir()?,
    }),
    Command::List { attach: true } => launch_client(&endpoint, &arguments, lua::LaunchState {
      command: "list".to_owned(),
      session_name: None,
      create_if_missing: false,
      fail_if_exists: false,
      require_session: true,
      working_directory: std::env::current_dir()?,
    }),
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
    let socket_instance = arguments
      .next()
      .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "server instance is required"))?;
    return Ok(CliOptions {
      command: Command::New {
        session_name: None,
        strict: false,
        path: None,
      },
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
      let mut session_name = None;
      let mut strict = false;
      let mut path = None;
      while let Some(argument) = arguments.next() {
        match argument.as_str() {
          "--strict" => strict = true,
          "--path" => {
            path = Some(
              arguments
                .next()
                .ok_or_else(|| {
                  io::Error::new(io::ErrorKind::InvalidInput, "--path requires a path")
                })?
                .into(),
            );
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
          other if !other.starts_with('-') && session_name.is_none() => {
            session_name = Some(parse_session_name(other)?);
          }
          other => return Err(invalid_argument(other)),
        }
      }
      Command::New {
        session_name,
        strict,
        path,
      }
    }
    "attach" => {
      let value = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "attach requires a session name")
      })?;
      let session_name = parse_session_name(&value)?;
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
      Command::Attach(session_name)
    }
    "list" => {
      let mut attach = false;
      while let Some(argument) = arguments.next() {
        match argument.as_str() {
          "--attach" | "-a" => attach = true,
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
      Command::List { attach }
    }
    "kill" => {
      let value = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "kill requires a session name")
      })?;
      let session_name = parse_session_name(&value)?;
      reject_command_arguments(&mut arguments, "kill", &mut socket_instance)?;
      Command::Kill(session_name)
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
      *socket_instance = arguments
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name"))?;
    } else {
      return Err(invalid_argument(argument.as_str()));
    }
  }
  let _ = command;
  Ok(())
}

fn parse_session_name(value: &str) -> Result<String, io::Error> {
  if value.is_empty() || value.chars().any(char::is_control) {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "session name must be non-empty and contain no control characters",
    ));
  }
  Ok(value.to_owned())
}

fn invalid_argument(argument: &str) -> io::Error {
  io::Error::new(io::ErrorKind::InvalidInput, format!("unknown argument or command: {argument}"))
}

fn print_help() {
  println!("Usage:");
  println!("  zmux new [SESSION] [--strict] [--path PATH] [--socket NAME]");
  println!("  zmux attach SESSION [--socket NAME]");
  println!("  zmux list [--attach | -a] [--socket NAME]");
  println!("  zmux kill SESSION [--socket NAME]");
  println!("  zmux server stop [--socket NAME]");
  println!("  zmux server status [--socket NAME]");
  println!();
  println!("new attaches to SESSION or creates it if missing; --strict fails if SESSION exists.");
  println!("attach requires SESSION to already exist.");
}

fn run_admin_command(
  endpoint: Endpoint,
  command: Command,
) -> Result<(), Box<dyn std::error::Error>> {
  let is_status = matches!(&command, Command::ServerStatus);
  let request = match command {
    Command::List { .. } => AdminRequest::ListSessions,
    Command::Kill(name) => AdminRequest::KillSession(name),
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
      return Err(
        format!("could not connect to server for '{}': {error}", endpoint.instance()).into(),
      );
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
            "{:<16} {:<8} {:<10} {}",
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

fn launch_client(
  endpoint: &Endpoint,
  arguments: &[String],
  launch: lua::LaunchState,
) -> Result<(), Box<dyn std::error::Error>> {
  let config = configuration(arguments, launch);
  let (launch, startup_commands) = client::run_pre_client(endpoint, &config)?;
  if launch.require_session && launch.session_name.is_none() {
    println!("no active sessions");
    return Ok(());
  }
  ensure_server(endpoint)?;
  client::run(endpoint.clone(), launch, startup_commands, config)
}

fn configuration(arguments: &[String], launch: lua::LaunchState) -> lua::AppConfig {
  let path = std::env::var_os("ZMUX_CONFIG")
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("zmux.lua"));
  if !path.exists() {
    return lua::load_config_with_context(include_str!("../zmux.lua"), arguments, launch)
      .expect("built-in Lua defaults are valid");
  }
  match lua::load_file_with_context(&path, arguments, launch.clone()) {
    Ok(config) => config,
    Err(error) => {
      eprintln!("could not load {}: {error}", path.display());
      lua::load_config_with_context(include_str!("../zmux.lua"), arguments, launch)
        .expect("built-in Lua defaults are valid")
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    Command,
    parse_args,
  };

  #[test]
  fn cli_requires_a_command() {
    assert!(parse_args(std::iter::empty()).is_err());
  }

  #[test]
  fn new_can_create_or_attach_a_named_session() {
    let options = parse_args(
      ["new", "12", "--strict", "--path", "/work/project", "--socket", "project"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid new command");

    assert!(
      matches!(&options.command, Command::New { session_name: Some(name), strict: true, .. } if name == "12")
    );
    assert_eq!(options.socket_instance, "project");
    assert!(
      matches!(&options.command, Command::New { path: Some(path), .. } if path == "/work/project")
    );
  }

  #[test]
  fn list_accepts_attach_long_and_short_flags() {
    let long = parse_args(["list", "--attach"].map(str::to_owned).into_iter())
      .expect("valid list attach command");
    let short =
      parse_args(["list", "-a"].map(str::to_owned).into_iter()).expect("valid list attach command");

    assert!(matches!(long.command, Command::List { attach: true }));
    assert!(matches!(short.command, Command::List { attach: true }));
  }

  #[test]
  fn attach_requires_an_existing_session_target() {
    let options = parse_args(
      ["attach", "12", "--socket", "project"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid attach command");

    assert!(matches!(options.command, Command::Attach(ref name) if name == "12"));
    assert_eq!(options.socket_instance, "project");
  }

  #[test]
  fn cli_rejects_invalid_session_names_and_arguments() {
    assert!(parse_args(["new", ""].map(str::to_owned).into_iter()).is_err());
    assert!(
      parse_args(
        ["new", "--attach", "project"]
          .map(str::to_owned)
          .into_iter()
      )
      .is_err()
    );
    assert!(parse_args(["attach", "project"].map(str::to_owned).into_iter()).is_ok());
    assert!(parse_args(["new", "--socket"].map(str::to_owned).into_iter()).is_err());
  }
}
