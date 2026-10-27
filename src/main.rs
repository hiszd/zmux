mod client;
mod clipboard;
mod events;
mod headless;
mod input;
mod lua;
mod pane;
mod server;
mod terminal_emulator;
mod trace;
mod ui;

use std::{
  fs,
  io::{
    self,
    IsTerminal,
    Write,
  },
  path::{
    Path,
    PathBuf,
  },
  process::{
    Command as ProcessCommand,
    Stdio,
  },
  thread,
  time::Duration,
};

use trace::Trace;
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
  WorkspaceList(String),
  Kill(String),
  ServerStop,
  ServerStatus,
  Run {
    source: headless::ScriptInput,
    path: Option<PathBuf>,
    arguments: Vec<String>,
  },
  Keybinds {
    json: bool,
  },
  Config(ConfigCommand),
}

#[derive(Clone, Copy)]
enum ConfigCommand {
  Init { replace: bool },
  Check,
}

struct CliOptions {
  command: Command,
  socket_instance: String,
  server_process: bool,
}

#[derive(Clone, Copy)]
enum ConfigPlatform {
  #[cfg(any(test, all(unix, not(target_os = "macos"))))]
  Unix,
  #[cfg(any(test, target_os = "macos"))]
  MacOs,
  #[cfg(any(test, target_os = "windows"))]
  Windows,
  #[cfg(any(test, not(any(unix, target_os = "windows"))))]
  Other,
}

fn config_path_for(
  platform: ConfigPlatform,
  _xdg_config_home: Option<&std::path::Path>,
  home: Option<&std::path::Path>,
  _appdata: Option<&std::path::Path>,
) -> Option<PathBuf> {
  let config_directory = match platform {
    #[cfg(any(test, all(unix, not(target_os = "macos"))))]
    ConfigPlatform::Unix => _xdg_config_home
      .filter(|path| path.is_absolute())
      .map(PathBuf::from)
      .or_else(|| home.map(|path| path.join(".config")))?,
    #[cfg(any(test, target_os = "macos"))]
    ConfigPlatform::MacOs => home?.join("Library/Application Support"),
    #[cfg(any(test, target_os = "windows"))]
    ConfigPlatform::Windows => _appdata
      .map(PathBuf::from)
      .or_else(|| home.map(|path| path.join("AppData/Roaming")))?,
    #[cfg(any(test, not(any(unix, target_os = "windows"))))]
    ConfigPlatform::Other => home?.join(".config"),
  };
  Some(config_directory.join("zmux").join("zmux.lua"))
}

fn resolve_config_path(explicit: Option<PathBuf>, default: Option<PathBuf>) -> Option<PathBuf> {
  explicit.or(default)
}

fn default_config_path() -> Option<PathBuf> {
  let home = std::env::var_os("HOME")
    .or_else(|| std::env::var_os("USERPROFILE"))
    .map(PathBuf::from);
  let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
  let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
  #[cfg(target_os = "windows")]
  let platform = ConfigPlatform::Windows;
  #[cfg(target_os = "macos")]
  let platform = ConfigPlatform::MacOs;
  #[cfg(all(unix, not(target_os = "macos")))]
  let platform = ConfigPlatform::Unix;
  #[cfg(not(any(unix, target_os = "windows")))]
  let platform = ConfigPlatform::Other;
  config_path_for(platform, xdg_config_home.as_deref(), home.as_deref(), appdata.as_deref())
}

fn main() {
  let trace_guard = match Trace::init_from_environment() {
    Ok(guard) => guard,
    Err(error) => {
      eprintln!("Could not initialize trace log: {error}");
      None
    }
  };
  let trace = Trace::from_environment();
  if let Err(error) = run() {
    trace.error("application", error.as_ref());
    eprintln!("Error: {error}");
    drop(trace_guard);
    std::process::exit(1);
  }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
  let arguments: Vec<String> = std::env::args().skip(1).collect();
  let options = parse_args(arguments.clone().into_iter())?;
  if options.server_process {
    return server::run(Endpoint::new(Some(&options.socket_instance))?);
  }
  if let Command::Keybinds { json } = &options.command {
    let config = keybinds_configuration(&arguments)?;
    return print_keybinds(&config, *json);
  }
  if let Command::Config(command) = &options.command {
    return run_config_command(*command, &arguments);
  }
  let endpoint = Endpoint::new(Some(&options.socket_instance))?;

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
    Command::WorkspaceList(session_name) => {
      let workspaces = headless::list_workspaces(&endpoint, session_name)?;
      if workspaces.is_empty() {
        println!("no workspaces");
      } else {
        println!("ACTIVE  WORKSPACE             WORKING DIRECTORY");
        for workspace in workspaces {
          println!(
            "{:<6} {:<21} {}",
            if workspace.active { "*" } else { "" },
            workspace.name,
            workspace.working_directory.display()
          );
        }
      }
      Ok(())
    }
    Command::Run {
      source,
      path,
      arguments: script_arguments,
    } => {
      ensure_server(&endpoint)?;
      let path = path.unwrap_or(std::env::current_dir()?);
      let path = if path.is_absolute() {
        path
      } else {
        std::env::current_dir()?.join(path)
      };
      headless::run(endpoint, source, &script_arguments, path)
    }
    command => run_admin_command(endpoint, command),
  }
}

fn parse_args(mut arguments: impl Iterator<Item = String>) -> Result<CliOptions, io::Error> {
  let Some(command) = arguments.next() else {
    return Err(io::Error::new(
      io::ErrorKind::InvalidInput,
      "a command is required; use 'zmux --help' to see available commands",
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
    "keybinds" => {
      let mut json = false;
      for argument in arguments.by_ref() {
        match argument.as_str() {
          "--json" => json = true,
          "--help" | "-h" => {
            print_help();
            std::process::exit(0);
          }
          other => return Err(invalid_argument(other)),
        }
      }
      Command::Keybinds { json }
    }
    "config" => {
      let subcommand = arguments.next().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config requires init or check")
      })?;
      match subcommand.as_str() {
        "init" => {
          let mut replace = false;
          for argument in arguments {
            match argument.as_str() {
              "--replace" => replace = true,
              "--help" | "-h" => {
                print_help();
                std::process::exit(0);
              }
              other => return Err(invalid_argument(other)),
            }
          }
          Command::Config(ConfigCommand::Init { replace })
        }
        "check" => {
          if let Some(argument) = arguments.next() {
            return Err(invalid_argument(&argument));
          }
          Command::Config(ConfigCommand::Check)
        }
        "--help" | "-h" => {
          print_help();
          std::process::exit(0);
        }
        other => return Err(invalid_argument(other)),
      }
    }
    "run" => {
      let mut path = None;
      let mut source = None;
      let mut script_arguments = Vec::new();
      while let Some(argument) = arguments.next() {
        if source.is_some() {
          script_arguments.push(argument);
          continue;
        }
        match argument.as_str() {
          "--help" | "-h" => {
            print_help();
            std::process::exit(0);
          }
          "--path" => {
            path = Some(
              arguments
                .next()
                .ok_or_else(|| {
                  io::Error::new(io::ErrorKind::InvalidInput, "--path requires a path")
                })?
                .into(),
            )
          }
          "--socket" => {
            socket_instance = arguments.next().ok_or_else(|| {
              io::Error::new(io::ErrorKind::InvalidInput, "--socket requires a name")
            })?
          }
          "--string" => {
            let value = arguments.next().ok_or_else(|| {
              io::Error::new(io::ErrorKind::InvalidInput, "--string requires Lua source")
            })?;
            source = Some(headless::ScriptInput::Source(value));
          }
          value if !value.starts_with('-') => {
            source = Some(headless::ScriptInput::File(value.into()))
          }
          other => return Err(invalid_argument(other)),
        }
      }
      Command::Run {
        source: source.ok_or_else(|| {
          io::Error::new(io::ErrorKind::InvalidInput, "run requires --string SOURCE or a Lua file")
        })?,
        path,
        arguments: script_arguments,
      }
    }
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
    "workspace" => {
      let subcommand = arguments
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "workspace requires list"))?;
      match subcommand.as_str() {
        "list" => {
          let session = arguments.next().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "workspace list requires a session name")
          })?;
          let session = parse_session_name(&session)?;
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
          Command::WorkspaceList(session)
        }
        "--help" | "-h" => {
          print_help();
          std::process::exit(0);
        }
        other => return Err(invalid_argument(other)),
      }
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
  println!("  zmux workspace list SESSION [--socket NAME]");
  println!("  zmux kill SESSION [--socket NAME]");
  println!("  zmux server stop [--socket NAME]");
  println!("  zmux server status [--socket NAME]");
  println!("  zmux keybinds [--json]");
  println!("  zmux config init [--replace]");
  println!("  zmux config check");
  println!(
    "  zmux run [--socket NAME] [--path DIRECTORY] [SCRIPT.lua | --string SOURCE] [SCRIPT_ARGS...]"
  );
  println!();
  println!("new attaches to SESSION or creates it if missing; --strict fails if SESSION exists.");
  println!("attach requires SESSION to already exist.");
  println!("run accepts a positional Lua file or inline source with --string.");
  println!("keybinds lists the configured key sequences; --json emits pretty JSON.");
  println!(
    "config init installs LuaLS support files and a sample config in the default config directory."
  );
  println!("config check validates the effective Lua config without starting the Server.");
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
    Command::New { .. }
    | Command::Attach(_)
    | Command::WorkspaceList(_)
    | Command::Run { .. }
    | Command::Keybinds { .. }
    | Command::Config(_) => unreachable!(),
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

struct LoadedConfiguration {
  config: lua::AppConfig,
  source: Option<PathBuf>,
  watch_path: Option<PathBuf>,
}

fn launch_client(
  endpoint: &Endpoint,
  arguments: &[String],
  launch: lua::LaunchState,
) -> Result<(), Box<dyn std::error::Error>> {
  let loaded = configuration_with_source(arguments, launch.clone())?;
  let config = loaded.config;
  let watch_path = loaded.watch_path;
  let (launch, startup_commands) = client::run_pre_client(endpoint, &config)?;
  if launch.require_session && launch.session_name.is_none() {
    println!("no active sessions");
    return Ok(());
  }
  ensure_server(endpoint)?;
  client::run(endpoint.clone(), launch, startup_commands, config, watch_path, arguments.to_vec())
}

#[derive(serde::Serialize)]
struct KeybindsOutput {
  prefix_key: String,
  bindings: Vec<KeybindOutput>,
}

#[derive(serde::Serialize)]
struct KeybindOutput {
  keys: String,
  description: Option<String>,
}

fn keybinds_json(config: &lua::AppConfig) -> Result<String, serde_json::Error> {
  let output = KeybindsOutput {
    prefix_key: input::key_notation::key_stroke_to_string(&config.prefix_key),
    bindings: config
      .keymap
      .entries()
      .into_iter()
      .map(|(keys, description)| KeybindOutput { keys, description })
      .collect(),
  };
  serde_json::to_string_pretty(&output)
}

fn print_keybinds(config: &lua::AppConfig, json: bool) -> Result<(), Box<dyn std::error::Error>> {
  if json {
    println!("{}", keybinds_json(config)?);
  } else {
    println!("Prefix: {}", input::key_notation::key_stroke_to_string(&config.prefix_key));
    println!("SEQUENCE  DESCRIPTION");
    for (keys, description) in config.keymap.entries() {
      println!("{keys:<9} {}", description.as_deref().unwrap_or("(no description)"));
    }
  }
  Ok(())
}

fn keybinds_configuration(
  arguments: &[String],
) -> Result<lua::AppConfig, Box<dyn std::error::Error>> {
  let launch = lua::LaunchState {
    command: "keybinds".to_owned(),
    working_directory: std::env::current_dir()?,
    ..lua::LaunchState::default()
  };
  configuration(arguments, launch)
}

fn configuration(
  arguments: &[String],
  launch: lua::LaunchState,
) -> Result<lua::AppConfig, Box<dyn std::error::Error>> {
  configuration_with_source(arguments, launch).map(|loaded| loaded.config)
}

fn configuration_with_source(
  arguments: &[String],
  launch: lua::LaunchState,
) -> Result<LoadedConfiguration, Box<dyn std::error::Error>> {
  let explicit_path = std::env::var_os("ZMUX_CONFIG").map(PathBuf::from);
  let path = resolve_config_path(explicit_path.clone(), default_config_path());
  let watch_path = path.clone();
  if let Some(path) = path {
    match fs::metadata(&path) {
      Ok(_) => {
        let config = lua::load_file_with_context(&path, arguments, launch)
          .map_err(|error| format!("could not load {}: {error}", path.display()))?;
        return Ok(LoadedConfiguration {
          config,
          source: Some(path.clone()),
          watch_path: Some(path),
        });
      }
      Err(error) if error.kind() == io::ErrorKind::NotFound && explicit_path.is_none() => {}
      Err(error) => {
        return Err(format!("could not load {}: {error}", path.display()).into());
      }
    }
    if explicit_path.is_some() {
      return Err(format!("could not load {}: file does not exist", path.display()).into());
    }
  }
  let config = lua::load_config_with_context(lua::DEFAULT_CONFIG_SOURCE, arguments, launch)?;
  Ok(LoadedConfiguration {
    config,
    source: None,
    watch_path,
  })
}

const LUA_LS_DEFINITIONS: &str = include_str!("../lua/zmux.lua");
const LUA_LS_CONFIG: &str = include_str!("../assets/luarc.json");
const CONFIG_EXAMPLE: &str = include_str!("../assets/zmux.lua.example");

fn run_config_command(
  command: ConfigCommand,
  arguments: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
  match command {
    ConfigCommand::Init { replace } => {
      let config_path = default_config_path()
        .ok_or("could not determine the default config directory; set HOME or XDG_CONFIG_HOME")?;
      let directory = config_path
        .parent()
        .ok_or("default config path has no parent directory")?;
      initialize_config_files(directory, replace)?;
    }
    ConfigCommand::Check => {
      let launch = lua::LaunchState {
        command: "config check".to_owned(),
        working_directory: std::env::current_dir()?,
        ..lua::LaunchState::default()
      };
      let loaded = configuration_with_source(arguments, launch)?;
      let version = loaded
        .config
        .config_api_version
        .unwrap_or(lua::CONFIG_API_VERSION);
      let source = loaded.source;
      if let Some(source) = source {
        println!("Configuration is valid (API version {version}): {}", source.display());
      } else {
        println!("Configuration is valid (API version {version}); using built-in defaults.");
      }
    }
  }
  Ok(())
}

fn initialize_config_files(
  directory: &Path,
  replace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
  fs::create_dir_all(directory)
    .map_err(|error| format!("could not create {}: {error}", directory.display()))?;

  let definitions_path = directory.join("defs.lua");
  let current_definitions = match fs::read(&definitions_path) {
    Ok(contents) => Some(contents),
    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
    Err(error) => {
      return Err(format!("could not read {}: {error}", definitions_path.display()).into());
    }
  };

  match current_definitions.as_deref() {
    Some(contents) if contents == LUA_LS_DEFINITIONS.as_bytes() => {
      set_read_only(&definitions_path, true)?;
      println!("LuaLS definitions are up to date: {}", definitions_path.display());
    }
    Some(contents) => {
      let confirmed = if replace {
        true
      } else if !io::stdin().is_terminal() {
        return Err(
          format!(
            "{} contains different definitions; rerun with --replace to update it",
            definitions_path.display()
          )
          .into(),
        );
      } else {
        confirm_definition_replacement(&definitions_path)?
      };
      if confirmed {
        replace_definitions(directory, &definitions_path, contents)?;
        println!("Updated LuaLS definitions; previous version saved as defs.old.");
      } else {
        println!("Kept existing definitions: {}", definitions_path.display());
      }
    }
    None => {
      install_managed_definitions(&definitions_path)?;
      println!("Installed LuaLS definitions: {}", definitions_path.display());
    }
  }

  install_file_if_missing(directory, ".luarc.json", LUA_LS_CONFIG.as_bytes())?;
  install_file_if_missing(directory, "zmux.lua.example", CONFIG_EXAMPLE.as_bytes())?;
  Ok(())
}

fn confirm_definition_replacement(path: &Path) -> io::Result<bool> {
  eprint!("Replace {}? The current file will be saved as defs.old [y/N] ", path.display());
  io::stderr().flush()?;
  let mut response = String::new();
  io::stdin().read_line(&mut response)?;
  Ok(matches!(response.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn install_managed_definitions(path: &Path) -> io::Result<()> {
  let staged = stage_file(path, LUA_LS_DEFINITIONS.as_bytes())?;
  if let Err(error) = replace_staged_file(&staged, path) {
    let _ = fs::remove_file(&staged);
    return Err(error);
  }
  set_read_only(path, true)
}

fn replace_definitions(directory: &Path, path: &Path, current_contents: &[u8]) -> io::Result<()> {
  let staged_definitions = stage_file(path, LUA_LS_DEFINITIONS.as_bytes())?;
  let backup_path = directory.join("defs.old");
  let staged_backup = match stage_file(&backup_path, current_contents) {
    Ok(path) => path,
    Err(error) => {
      let _ = fs::remove_file(&staged_definitions);
      return Err(error);
    }
  };

  if let Err(error) = replace_staged_file(&staged_backup, &backup_path) {
    let _ = fs::remove_file(&staged_backup);
    let _ = fs::remove_file(&staged_definitions);
    return Err(error);
  }
  if let Err(error) = replace_staged_file(&staged_definitions, path) {
    let _ = fs::remove_file(&staged_definitions);
    return Err(error);
  }
  set_read_only(path, true)
}

fn install_file_if_missing(directory: &Path, name: &str, contents: &[u8]) -> io::Result<()> {
  let path = directory.join(name);
  let mut file = match fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(&path)
  {
    Ok(file) => file,
    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
      println!("Preserved existing file: {}", path.display());
      return Ok(());
    }
    Err(error) => return Err(error),
  };
  if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
    drop(file);
    let _ = fs::remove_file(&path);
    return Err(error);
  }
  println!("Installed: {}", path.display());
  Ok(())
}

fn stage_file(destination: &Path, contents: &[u8]) -> io::Result<PathBuf> {
  for sequence in 0..100 {
    let file_name = destination
      .file_name()
      .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no filename"))?;
    let staged_name =
      format!(".{}.{}.{}.tmp", file_name.to_string_lossy(), std::process::id(), sequence);
    let staged = destination
      .parent()
      .unwrap_or_else(|| Path::new("."))
      .join(staged_name);
    let mut file = match fs::OpenOptions::new()
      .write(true)
      .create_new(true)
      .open(&staged)
    {
      Ok(file) => file,
      Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
      Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(contents).and_then(|()| file.sync_all()) {
      drop(file);
      let _ = fs::remove_file(&staged);
      return Err(error);
    }
    return Ok(staged);
  }
  Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not allocate a temporary config file"))
}

fn replace_staged_file(staged: &Path, destination: &Path) -> io::Result<()> {
  #[cfg(windows)]
  if destination.exists() {
    set_read_only(destination, false)?;
    fs::remove_file(destination)?;
  }
  fs::rename(staged, destination)
}

fn set_read_only(path: &Path, read_only: bool) -> io::Result<()> {
  let mut permissions = fs::metadata(path)?.permissions();
  permissions.set_readonly(read_only);
  fs::set_permissions(path, permissions)
}

#[cfg(test)]
mod tests {
  use super::{
    Command,
    ConfigPlatform,
    config_path_for,
    headless::ScriptInput,
    keybinds_json,
    lua,
    parse_args,
    resolve_config_path,
  };

  #[test]
  fn config_location_uses_standard_platform_directories() {
    let home = std::path::Path::new("/home/test");
    assert_eq!(
      config_path_for(ConfigPlatform::Unix, None, Some(home), None),
      Some(std::path::PathBuf::from("/home/test/.config/zmux/zmux.lua")),
    );
    assert_eq!(
      config_path_for(
        ConfigPlatform::Unix,
        Some(std::path::Path::new("/tmp/config")),
        Some(home),
        None,
      ),
      Some(std::path::PathBuf::from("/tmp/config/zmux/zmux.lua")),
    );
    assert_eq!(
      config_path_for(ConfigPlatform::MacOs, None, Some(home), None),
      Some(std::path::PathBuf::from("/home/test/Library/Application Support/zmux/zmux.lua")),
    );
    assert_eq!(
      config_path_for(ConfigPlatform::Other, None, Some(home), None),
      Some(std::path::PathBuf::from("/home/test/.config/zmux/zmux.lua")),
    );
    assert_eq!(
      config_path_for(
        ConfigPlatform::Windows,
        None,
        Some(home),
        Some(std::path::Path::new("C:/Users/test/AppData/Roaming")),
      ),
      Some(std::path::PathBuf::from("C:/Users/test/AppData/Roaming/zmux/zmux.lua")),
    );
  }

  #[test]
  fn explicit_config_path_overrides_platform_default() {
    assert_eq!(
      resolve_config_path(
        Some(std::path::PathBuf::from("/custom/zmux.lua")),
        Some(std::path::PathBuf::from("/home/test/.config/zmux/zmux.lua")),
      ),
      Some(std::path::PathBuf::from("/custom/zmux.lua")),
    );
  }

  #[test]
  fn relative_xdg_config_home_is_ignored() {
    assert_eq!(
      config_path_for(
        ConfigPlatform::Unix,
        Some(std::path::Path::new("relative/config")),
        Some(std::path::Path::new("/home/test")),
        None,
      ),
      Some(std::path::PathBuf::from("/home/test/.config/zmux/zmux.lua")),
    );
  }

  // Developer approved: do not remove.
  #[test]
  fn cli_requires_a_command() {
    let error = parse_args(std::iter::empty())
      .err()
      .expect("empty arguments must be rejected");

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("a command is required"));
    assert!(error.to_string().contains("zmux --help"));
  }

  // Developer approved: do not remove.
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
      matches!(&options.command, Command::New { path: Some(path), .. } if path == std::path::Path::new("/work/project"))
    );
  }

  // Developer approved: do not remove.
  #[test]
  fn workspace_list_accepts_session_and_socket_options() {
    let options = parse_args(
      ["workspace", "list", "project", "--socket", "named"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid workspace list command");

    assert!(matches!(options.command, Command::WorkspaceList(ref session) if session == "project"));
    assert_eq!(options.socket_instance, "named");
    assert!(parse_args(["workspace", "list"].map(str::to_owned).into_iter()).is_err());
  }

  // Developer approved: do not remove.
  #[test]
  fn run_accepts_script_arguments_and_launch_options() {
    let options = parse_args(
      ["run", "--path", "/work", "--socket", "named", "script.lua", "one", "two"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid run command");
    assert!(
      matches!(options.command, Command::Run { ref source, ref arguments, ref path } if source == &ScriptInput::File("script.lua".into()) && path.as_deref() == Some(std::path::Path::new("/work")) && arguments == &["one", "two"])
    );
    assert_eq!(options.socket_instance, "named");
  }

  // Developer approved: do not remove.
  #[test]
  fn run_accepts_inline_source_and_preserves_following_arguments() {
    let options = parse_args(
      ["run", "--socket", "script-socket", "--string", "print(args[1])", "first", "--flag"]
        .map(str::to_owned)
        .into_iter(),
    )
    .expect("valid inline script command");
    assert!(
      matches!(options.command, Command::Run { ref source, ref arguments, .. } if source == &ScriptInput::Source("print(args[1])".to_owned()) && arguments == &["first", "--flag"])
    );
    assert_eq!(options.socket_instance, "script-socket");
  }

  // Developer approved: do not remove.
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

  // Developer approved: do not remove.
  #[test]
  fn keybinds_accepts_optional_json_output_flag() {
    let plain = parse_args(["keybinds"].map(str::to_owned).into_iter()).expect("keybind command");
    let json = parse_args(["keybinds", "--json"].map(str::to_owned).into_iter())
      .expect("JSON keybind command");

    assert!(matches!(plain.command, Command::Keybinds { json: false }));
    assert!(matches!(json.command, Command::Keybinds { json: true }));
    assert!(parse_args(["keybinds", "--yaml"].map(str::to_owned).into_iter()).is_err());
  }

  // Developer approved: do not remove.
  #[test]
  fn keybinds_json_is_pretty_printed_and_includes_prefix_and_descriptions() {
    let config = lua::load_config(
      "opt.prefix_key = '<C-x>'\ncmd.key.bind('z', function() end)\ncmd.key.bind('a', function() end, { desc = 'Create tab' })",
    )
    .expect("valid keybind config");

    assert_eq!(
      keybinds_json(&config).expect("serialize keybinds"),
      r#"{
  "prefix_key": "<C-x>",
  "bindings": [
    {
      "keys": "a",
      "description": "Create tab"
    },
    {
      "keys": "z",
      "description": null
    }
  ]
}"#
    );
  }
}
