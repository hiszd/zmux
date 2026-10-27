use std::{
  fs,
  path::PathBuf,
  time::{
    Duration,
    Instant,
  },
};

use crate::lua::{
  self,
  AppConfig,
  LaunchState,
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Detects stable changes to a configuration file without watching the Server.
pub(crate) struct ConfigFileWatcher {
  path: PathBuf,
  last_observed: Option<Vec<u8>>,
  pending: Option<(Option<Vec<u8>>, Instant)>,
  next_poll: Instant,
}

impl ConfigFileWatcher {
  pub(crate) fn new(path: PathBuf) -> Self {
    let now = Instant::now();
    Self {
      last_observed: fs::read(&path).ok(),
      path,
      pending: None,
      next_poll: now,
    }
  }

  /// Returns true once a changed file has remained stable for the debounce window.
  pub(crate) fn accept_current(&mut self) {
    self.last_observed = fs::read(&self.path).ok();
    self.pending = None;
    self.next_poll = Instant::now() + POLL_INTERVAL;
  }

  pub(crate) fn changed(&mut self, now: Instant) -> bool {
    if now < self.next_poll {
      return false;
    }
    self.next_poll = now + POLL_INTERVAL;
    let current = fs::read(&self.path).ok();
    if current == self.last_observed {
      self.pending = None;
      return false;
    }

    match &self.pending {
      Some((pending, since)) if *pending == current => {
        if now.duration_since(*since) >= DEBOUNCE {
          self.last_observed = current;
          self.pending = None;
          true
        } else {
          false
        }
      }
      _ => {
        self.pending = Some((current, now));
        false
      }
    }
  }
}

pub(crate) struct ConfigReloader {
  path: PathBuf,
  watcher: ConfigFileWatcher,
  arguments: Vec<String>,
  launch: LaunchState,
}

impl ConfigReloader {
  pub(crate) fn new(path: PathBuf, arguments: Vec<String>, launch: LaunchState) -> Self {
    Self {
      watcher: ConfigFileWatcher::new(path.clone()),
      path,
      arguments,
      launch,
    }
  }

  pub(crate) fn reload(&mut self) -> mlua::Result<AppConfig> {
    self.watcher.accept_current();
    lua::load_file_with_context(&self.path, &self.arguments, self.launch.clone()).map_err(|error| {
      mlua::Error::external(format!("could not reload {}: {error}", self.path.display()))
    })
  }

  pub(crate) fn poll(&mut self, enabled: bool, now: Instant) -> Option<mlua::Result<AppConfig>> {
    if !enabled || !self.watcher.changed(now) {
      return None;
    }
    Some(self.reload())
  }
}

#[cfg(test)]
mod tests {
  use std::{
    fs,
    path::PathBuf,
    sync::atomic::{
      AtomicU64,
      Ordering,
    },
    time::{
      Duration,
      Instant,
    },
  };

  use super::{
    ConfigFileWatcher,
    ConfigReloader,
  };

  static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

  struct TempDirectory(PathBuf);

  impl TempDirectory {
    fn new() -> Self {
      let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
      let path =
        std::env::temp_dir().join(format!("zmux-live-config-{}-{sequence}", std::process::id()));
      fs::create_dir_all(&path).expect("create temporary directory");
      Self(path)
    }

    fn file(&self) -> PathBuf { self.0.join("zmux.lua") }
  }

  impl Drop for TempDirectory {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
  }

  #[test]
  fn reports_a_stable_file_change_once() {
    let directory = TempDirectory::new();
    fs::write(directory.file(), b"initial").expect("write initial config");
    let mut watcher = ConfigFileWatcher::new(directory.file());
    let start = Instant::now();

    assert!(!watcher.changed(start));
    fs::write(directory.file(), b"updated").expect("write changed config");
    assert!(!watcher.changed(start + Duration::from_millis(250)));
    assert!(!watcher.changed(start + Duration::from_millis(500)));
    assert!(watcher.changed(start + Duration::from_millis(750)));
    assert!(!watcher.changed(start + Duration::from_millis(1_000)));
  }

  #[test]
  fn loads_a_new_config_only_after_its_file_change_is_stable() {
    let directory = TempDirectory::new();
    let source = |prefix: &str| {
      format!(
        "opt.config_version = {}\nopt.live_reload = true\nopt.prefix_key = '{prefix}'\n",
        crate::lua::CONFIG_API_VERSION
      )
    };
    fs::write(directory.file(), source("<C-Space>")).expect("write initial config");
    let mut reloader =
      ConfigReloader::new(directory.file(), Vec::new(), crate::lua::LaunchState::default());
    let start = Instant::now();
    assert!(reloader.poll(true, start).is_none());

    fs::write(directory.file(), source("<C-x>")).expect("write changed config");
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(250))
        .is_none()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(500))
        .is_none()
    );
    let loaded = reloader
      .poll(true, start + Duration::from_millis(750))
      .expect("stable config change")
      .expect("valid changed config");
    assert_eq!(crate::input::key_notation::key_stroke_to_string(&loaded.prefix_key), "<C-x>");
  }

  #[test]
  fn manual_reload_loads_the_file_when_automatic_reload_is_disabled() {
    let directory = TempDirectory::new();
    let source = format!(
      "opt.config_version = {}\nopt.live_reload = false\nopt.prefix_key = '<C-x>'\n",
      crate::lua::CONFIG_API_VERSION
    );
    fs::write(directory.file(), source).expect("write config");
    let mut reloader =
      ConfigReloader::new(directory.file(), Vec::new(), crate::lua::LaunchState::default());

    let loaded = reloader
      .reload()
      .expect("manually reload file-backed config");

    assert!(!loaded.live_reload);
    assert_eq!(crate::input::key_notation::key_stroke_to_string(&loaded.prefix_key), "<C-x>");
  }

  #[test]
  fn invalid_edits_are_reported_once_and_a_later_valid_edit_can_load() {
    let directory = TempDirectory::new();
    let valid =
      format!("opt.config_version = {}\nopt.live_reload = true\n", crate::lua::CONFIG_API_VERSION);
    fs::write(directory.file(), &valid).expect("write initial config");
    let mut reloader =
      ConfigReloader::new(directory.file(), Vec::new(), crate::lua::LaunchState::default());
    let start = Instant::now();
    assert!(reloader.poll(true, start).is_none());

    fs::write(directory.file(), "invalid lua =").expect("write invalid config");
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(250))
        .is_none()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(500))
        .is_none()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(750))
        .expect("stable invalid edit")
        .is_err()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(1_000))
        .is_none()
    );

    fs::write(directory.file(), valid).expect("write corrected config");
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(1_250))
        .is_none()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(1_500))
        .is_none()
    );
    assert!(
      reloader
        .poll(true, start + Duration::from_millis(1_750))
        .expect("stable corrected edit")
        .is_ok()
    );
  }

  #[test]
  fn restarts_debounce_when_file_changes_again_before_stabilizing() {
    let directory = TempDirectory::new();
    fs::write(directory.file(), b"initial").expect("write initial config");
    let mut watcher = ConfigFileWatcher::new(directory.file());
    let start = Instant::now();

    fs::write(directory.file(), b"first edit").expect("write first edit");
    assert!(!watcher.changed(start));
    fs::write(directory.file(), b"final edit").expect("write final edit");
    assert!(!watcher.changed(start + Duration::from_millis(250)));
    assert!(!watcher.changed(start + Duration::from_millis(500)));
    assert!(watcher.changed(start + Duration::from_millis(750)));
  }
}
