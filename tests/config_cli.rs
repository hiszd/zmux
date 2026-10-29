use std::{
  fs,
  path::PathBuf,
  process::{
    Command,
    Output,
  },
  sync::atomic::{
    AtomicUsize,
    Ordering,
  },
};

static NEXT_TEMP_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
  fn new() -> Self {
    let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let path =
      std::env::temp_dir().join(format!("zmux-config-cli-{}-{sequence}", std::process::id()));
    fs::create_dir_all(&path).expect("create temporary directory");
    Self(path)
  }

  fn path(&self) -> &std::path::Path { &self.0 }
}

impl Drop for TempDirectory {
  fn drop(&mut self) {
    clear_read_only_files(&self.0);
    let _ = fs::remove_dir_all(&self.0);
  }
}

#[cfg(windows)]
fn clear_read_only_files(directory: &std::path::Path) {
  let Ok(entries) = fs::read_dir(directory) else {
    return;
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      clear_read_only_files(&path);
    } else if let Ok(metadata) = fs::metadata(&path) {
      let mut permissions = metadata.permissions();
      permissions.set_readonly(false);
      let _ = fs::set_permissions(path, permissions);
    }
  }
}

#[cfg(not(windows))]
fn clear_read_only_files(_directory: &std::path::Path) {
}

fn zmux() -> Command { Command::new(env!("CARGO_BIN_EXE_zmux")) }

fn expected_config_directory(config_home: &std::path::Path) -> PathBuf {
  #[cfg(target_os = "macos")]
  {
    config_home.join("Library/Application Support/zmux")
  }
  #[cfg(not(target_os = "macos"))]
  {
    config_home.join("zmux")
  }
}

fn run_init(config_home: &std::path::Path, replace: bool) -> Output {
  let mut command = zmux();
  command
    .args(["config", "init"])
    .env("XDG_CONFIG_HOME", config_home)
    .env("APPDATA", config_home)
    .env("HOME", config_home)
    .env_remove("ZMUX_CONFIG");
  if replace {
    command.arg("--replace");
  }
  command.output().expect("run zmux config init")
}

#[test]
fn config_init_installs_embedded_assets_and_marks_definitions_read_only() {
  let config_home = TempDirectory::new();
  let output = run_init(config_home.path(), false);
  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

  let config_directory = expected_config_directory(config_home.path());
  assert_eq!(
    fs::read(config_directory.join("defs.lua")).expect("read installed definitions"),
    include_bytes!("../lua/zmux.lua"),
  );
  let installed_settings =
    fs::read(config_directory.join(".luarc.json")).expect("read installed LuaLS settings");
  assert_eq!(installed_settings, include_bytes!("../assets/luarc.json"));
  serde_json::from_slice::<serde_json::Value>(&installed_settings)
    .expect("installed LuaLS settings should be valid JSON");
  let example = config_directory.join("zmux.lua.example");
  assert_eq!(
    fs::read(&example).expect("read installed example"),
    include_bytes!("../assets/zmux.lua.example"),
  );
  let check = zmux()
    .args(["config", "check"])
    .env("ZMUX_CONFIG", &example)
    .output()
    .expect("validate installed example");
  assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
  assert!(!config_directory.join("zmux.lua").exists(), "init must not create active user config");
  assert!(
    fs::metadata(config_directory.join("defs.lua"))
      .expect("stat installed definitions")
      .permissions()
      .readonly(),
    "installed definitions should be read-only"
  );
}

#[test]
fn config_init_preserves_custom_luals_settings_and_refreshes_example() {
  let config_home = TempDirectory::new();
  let first = run_init(config_home.path(), false);
  assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

  let config_directory = expected_config_directory(config_home.path());
  let settings = config_directory.join(".luarc.json");
  let example = config_directory.join("zmux.lua.example");
  fs::write(&settings, b"custom LuaLS settings").expect("customize LuaLS settings");
  fs::write(&example, b"outdated example").expect("seed outdated example");

  let second = run_init(config_home.path(), false);
  assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
  assert_eq!(fs::read(settings).expect("preserved settings"), b"custom LuaLS settings");
  assert_eq!(
    fs::read(example).expect("refreshed example"),
    include_bytes!("../assets/zmux.lua.example"),
  );
}

#[test]
fn config_init_requires_confirmation_before_replacing_and_keeps_one_backup() {
  let config_home = TempDirectory::new();
  let config_directory = expected_config_directory(config_home.path());
  fs::create_dir_all(&config_directory).expect("create config directory");
  let definitions = config_directory.join("defs.lua");
  let backup = config_directory.join("defs.old");
  fs::write(&definitions, b"older package definitions").expect("seed old definitions");
  let mut definitions_permissions = fs::metadata(&definitions)
    .expect("stat old definitions")
    .permissions();
  definitions_permissions.set_readonly(true);
  fs::set_permissions(&definitions, definitions_permissions)
    .expect("make old definitions read-only");
  fs::write(&backup, b"previous backup").expect("seed previous backup");
  let mut backup_permissions = fs::metadata(&backup)
    .expect("stat previous backup")
    .permissions();
  backup_permissions.set_readonly(true);
  fs::set_permissions(&backup, backup_permissions).expect("make previous backup read-only");

  let declined = run_init(config_home.path(), false);
  assert!(!declined.status.success(), "non-interactive replacement must be refused");
  assert!(String::from_utf8_lossy(&declined.stderr).contains("--replace"));
  assert_eq!(fs::read(&definitions).expect("preserved definitions"), b"older package definitions");
  assert_eq!(fs::read(&backup).expect("preserved backup"), b"previous backup");

  let replaced = run_init(config_home.path(), true);
  assert!(replaced.status.success(), "{}", String::from_utf8_lossy(&replaced.stderr));
  assert_eq!(fs::read(&backup).expect("latest backup"), b"older package definitions",);
  assert_eq!(
    fs::read(&definitions).expect("updated definitions"),
    include_bytes!("../lua/zmux.lua"),
  );
  assert!(
    fs::metadata(&definitions)
      .expect("stat updated definitions")
      .permissions()
      .readonly(),
    "updated definitions should be read-only"
  );
}

#[test]
fn config_check_validates_explicit_config_without_starting_the_server() {
  let directory = TempDirectory::new();
  let config = directory.path().join("user.lua");
  fs::write(&config, "opt.config_version = 3\nopt.prefix_key = '<C-x>'\n")
    .expect("write valid config");

  let output = zmux()
    .args(["config", "check"])
    .env("ZMUX_CONFIG", &config)
    .output()
    .expect("run zmux config check");

  assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
  assert!(String::from_utf8_lossy(&output.stdout).contains(&config.display().to_string()));
  assert!(String::from_utf8_lossy(&output.stdout).contains("valid"));
}

#[test]
fn config_check_reports_invalid_config_and_fails() {
  let directory = TempDirectory::new();
  let config = directory.path().join("user.lua");
  fs::write(&config, "error('this config must not run')\n").expect("write invalid config");

  let output = zmux()
    .args(["config", "check"])
    .env("ZMUX_CONFIG", &config)
    .output()
    .expect("run zmux config check");

  assert!(!output.status.success());
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(stderr.contains(&config.display().to_string()));
  assert!(stderr.contains("opt.config_version"));
  assert!(!stderr.contains("this config must not run"));
}
