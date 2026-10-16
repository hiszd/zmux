use std::path::PathBuf;

use zmux::{
  domain::{
    SessionName,
    SessionTabCloseOutcome,
  },
  protocol::{
    HeadlessRequest,
    HeadlessResult,
    HeadlessSession,
    HeadlessTab,
    HeadlessWorkspace,
    ServerMessage,
    Viewport,
  },
};

use super::ServerState;

impl ServerState {
  pub(super) fn handle_headless_request(
    &mut self,
    connection_id: u64,
    request_id: u64,
    request: HeadlessRequest,
  ) -> Result<(), Box<dyn std::error::Error>> {
    let Some(connection) = self.headless.get(&connection_id) else {
      return Ok(());
    };
    let sender = connection.sender.clone();
    let result: Result<HeadlessResult, String> = (|| match request {
      HeadlessRequest::SessionList => Ok(HeadlessResult::Sessions(self.headless_sessions())),
      HeadlessRequest::StopServer => {
        self.stop_server();
        Ok(HeadlessResult::Accepted)
      }
      HeadlessRequest::SessionCreate {
        name,
        workspace_name,
      } => {
        if let Some(name) = &name {
          Self::validate_name(name)?;
        }
        if let Some(name) = &workspace_name {
          Self::validate_name(name)?;
        }
        let (working_directory, shell, environment) = {
          let connection = self
            .headless
            .get(&connection_id)
            .ok_or("headless connection closed")?;
          (
            connection.working_directory.clone(),
            connection.shell.executable.clone(),
            connection.shell.environment.clone(),
          )
        };
        self
          .create_session(
            name.unwrap_or_default(),
            &working_directory,
            &shell,
            &environment,
            Viewport {
              rows: 24,
              columns: 80,
            },
          )
          .map(|session| {
            if let Some(workspace_name) = workspace_name
              && let Some(workspace) = self
                .session_mut(&session)
                .and_then(|value| value.model.active_workspace_mut())
            {
              workspace.set_name(Some(workspace_name));
            }
            HeadlessResult::Accepted
          })
          .map_err(|error| error.to_string())
      }
      HeadlessRequest::SessionRename { id, name } => self.headless_rename_session(id, name),
      HeadlessRequest::SessionRemove { id } => self.headless_remove_session(id),
      HeadlessRequest::SessionFocus { .. } => {
        Err("cmd.session.focus is a TUI Client operation and is unavailable headlessly".to_owned())
      }
      HeadlessRequest::WorkspaceCreate {
        session_id,
        name,
        working_directory,
      } => self.headless_create_workspace(session_id, name, working_directory),
      HeadlessRequest::WorkspaceList { session_id } => self.headless_workspace_list(session_id),
      HeadlessRequest::WorkspaceRename { id, name } => self.headless_workspace_rename(id, name),
      HeadlessRequest::WorkspaceSetCwd {
        id,
        working_directory,
      } => self.headless_workspace_setcwd(id, working_directory),
      HeadlessRequest::WorkspaceFocus { id } => self.headless_workspace_focus(id),
      HeadlessRequest::WorkspaceRemove { id } => self.headless_workspace_remove(id),
      HeadlessRequest::TabCreate { workspace_id, name } => {
        self.headless_create_tab(workspace_id, name)
      }
      HeadlessRequest::TabList { workspace_id } => self.headless_tab_list(workspace_id),
      HeadlessRequest::TabRename { id, name } => self.headless_tab_rename(id, name),
      HeadlessRequest::TabFocus { id } => self.headless_tab_focus(id),
      HeadlessRequest::TabRemove { id } => self.headless_tab_remove(id),
    })();
    sender.send(ServerMessage::HeadlessReply {
      id: request_id,
      result,
    })?;
    Ok(())
  }

  pub(super) fn headless_sessions(&self) -> Vec<HeadlessSession> {
    self
      .sessions
      .iter()
      .map(|session| HeadlessSession {
        id: session.model.name().get().to_owned(),
        name: session.model.name().get().to_owned(),
        workspaces: session.model.workspaces().len(),
        tabs: session.tabs.len(),
      })
      .collect()
  }

  pub(super) fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(':') {
      return Err("names must be non-empty and must not contain ':'".to_owned());
    }
    Ok(())
  }

  pub(super) fn headless_rename_session(
    &mut self,
    id: Option<String>,
    name: String,
  ) -> Result<HeadlessResult, String> {
    let old = id.ok_or("Session target is required in headless mode")?;
    Self::validate_name(&name)?;
    if old != name
      && self
        .sessions
        .iter()
        .any(|session| session.model.name().get() == name)
    {
      return Err(format!("Session {name} already exists"));
    }
    let session = self
      .session_mut(&old)
      .ok_or_else(|| format!("Session {old} does not exist"))?;
    session.model.set_name(SessionName::new(name.clone()));
    for client in self.clients.values_mut() {
      if client.session_name == old {
        client.session_name.clone_from(&name);
      }
    }
    self.broadcast_state(&name)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_remove_session(
    &mut self,
    id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let id = id.ok_or("Session target is required in headless mode")?;
    if self.session(&id).is_none() {
      return Err(format!("Session {id} does not exist"));
    }
    self.close_session(&id).map_err(|error| error.to_string())?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_create_workspace(
    &mut self,
    session_id: Option<String>,
    name: Option<String>,
    directory: PathBuf,
  ) -> Result<HeadlessResult, String> {
    let session = session_id.ok_or("Session target is required in headless mode")?;
    if let Some(name) = &name {
      Self::validate_name(name)?;
    }
    self.create_workspace(&session, directory, name)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn split_workspace_id(id: Option<String>) -> Result<(String, String), String> {
    let value = id.ok_or("Workspace target is required in headless mode")?;
    let mut parts = value.splitn(2, ':');
    Ok((
      parts.next().unwrap_or_default().to_owned(),
      parts
        .next()
        .ok_or("Workspace ID must be qualified")?
        .to_owned(),
    ))
  }

  pub(super) fn split_tab_id(id: Option<String>) -> Result<(String, String, String), String> {
    let value = id.ok_or("Tab target is required in headless mode")?;
    let mut parts = value.splitn(3, ':');
    Ok((
      parts.next().unwrap_or_default().to_owned(),
      parts.next().ok_or("Tab ID must be qualified")?.to_owned(),
      parts.next().ok_or("Tab ID must be qualified")?.to_owned(),
    ))
  }

  pub(super) fn headless_workspace_list(
    &self,
    session_id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let session_id = session_id.ok_or("Session target is required in headless mode")?;
    let session = self
      .session(&session_id)
      .ok_or_else(|| format!("Session {session_id} does not exist"))?;
    Ok(HeadlessResult::Workspaces(
      session
        .model
        .workspaces()
        .iter()
        .map(|workspace| HeadlessWorkspace {
          id: format!("{}:{}", session_id, workspace.name()),
          name: workspace.name().into_owned(),
          working_directory: workspace.working_directory().to_path_buf(),
          active: session.model.active_workspace_id() == Some(workspace.id()),
        })
        .collect(),
    ))
  }

  pub(super) fn headless_workspace_rename(
    &mut self,
    id: Option<String>,
    name: String,
  ) -> Result<HeadlessResult, String> {
    Self::validate_name(&name)?;
    let (session_id, old_name) = Self::split_workspace_id(id)?;
    if self.session(&session_id).is_some_and(|session| {
      session
        .model
        .workspaces()
        .iter()
        .any(|workspace| workspace.name() == old_name)
    }) {
      if old_name != name
        && self.session(&session_id).is_some_and(|session| {
          session
            .model
            .workspaces()
            .iter()
            .any(|workspace| workspace.name() == name)
        })
      {
        return Err(format!("Workspace {name} already exists"));
      }
    } else {
      return Err(format!("Workspace {session_id}:{old_name} does not exist"));
    }
    let session = self
      .session_mut(&session_id)
      .ok_or("Session no longer exists")?;
    let workspace_id = session
      .model
      .workspaces()
      .iter()
      .find(|workspace| workspace.name() == old_name)
      .map(|workspace| workspace.id())
      .ok_or("Workspace does not exist")?;
    session
      .model
      .workspace_mut(workspace_id)
      .ok_or("Workspace does not exist")?
      .set_name(Some(name));
    self.broadcast_state(&session_id)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_workspace_setcwd(
    &mut self,
    id: Option<String>,
    directory: PathBuf,
  ) -> Result<HeadlessResult, String> {
    if !directory.is_dir() {
      return Err(format!("working directory does not exist: {}", directory.display()));
    }
    let (session_id, name) = Self::split_workspace_id(id)?;
    let session = self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?;
    let workspace_id = session
      .model
      .workspaces()
      .iter()
      .find(|workspace| workspace.name() == name)
      .map(|workspace| workspace.id())
      .ok_or("Workspace does not exist")?;
    session
      .model
      .workspace_mut(workspace_id)
      .ok_or("Workspace does not exist")?
      .set_working_directory(directory);
    self.broadcast_state(&session_id)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_workspace_focus(
    &mut self,
    id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, name) = Self::split_workspace_id(id)?;
    let workspace_id = self
      .session(&session_id)
      .and_then(|session| {
        session
          .model
          .workspaces()
          .iter()
          .find(|workspace| workspace.name() == name)
          .map(|workspace| workspace.id())
      })
      .ok_or("Workspace does not exist")?;
    self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?
      .model
      .select_workspace(workspace_id);
    self.broadcast_state(&session_id)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_workspace_remove(
    &mut self,
    id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, name) = Self::split_workspace_id(id)?;
    let (workspace_id, tabs) = self
      .session(&session_id)
      .and_then(|session| {
        session
          .model
          .workspaces()
          .iter()
          .find(|workspace| workspace.name() == name)
          .map(|workspace| (workspace.id(), workspace.tabs().to_vec()))
      })
      .ok_or("Workspace does not exist")?;
    let outcome = self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?
      .model
      .close_workspace(workspace_id);
    if let Some(session) = self.session_mut(&session_id) {
      for tab in tabs {
        session.tabs.remove(&tab);
      }
    }
    if outcome == SessionTabCloseOutcome::SessionClosed {
      self
        .close_session(&session_id)
        .map_err(|error| error.to_string())?;
    } else {
      self.broadcast_state(&session_id)?;
    }
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_create_tab(
    &mut self,
    workspace_id: Option<String>,
    name: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, workspace_name) = Self::split_workspace_id(workspace_id)?;
    let id = self
      .session(&session_id)
      .and_then(|session| {
        session
          .model
          .workspaces()
          .iter()
          .find(|workspace| workspace.name() == workspace_name)
          .map(|workspace| workspace.id())
      })
      .ok_or("Workspace does not exist")?;
    if let Some(name) = &name {
      Self::validate_name(name)?;
      if self.session(&session_id).is_some_and(|session| {
        session
          .model
          .workspaces()
          .iter()
          .find(|workspace| workspace.id() == id)
          .is_some_and(|workspace| {
            workspace.tabs().iter().any(|tab_id| {
              session
                .tabs
                .get(tab_id)
                .is_some_and(|tab| tab.name == *name)
            })
          })
      }) {
        return Err(format!("Tab {name} already exists in Workspace {workspace_name}"));
      }
    }
    self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?
      .model
      .select_workspace(id);
    self.create_tab(&session_id)?;
    if let Some(name) = name {
      let tab_id = self
        .session(&session_id)
        .and_then(|session| session.model.active_tab());
      if let Some(tab_id) = tab_id
        && let Some(session) = self.session_mut(&session_id)
        && let Some(tab) = session.tabs.get_mut(&tab_id)
      {
        tab.name = name;
      }
    }
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_tab_list(
    &self,
    workspace_id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, workspace_name) = Self::split_workspace_id(workspace_id)?;
    let session = self.session(&session_id).ok_or("Session does not exist")?;
    let workspace = session
      .model
      .workspaces()
      .iter()
      .find(|workspace| workspace.name() == workspace_name)
      .ok_or("Workspace does not exist")?;
    Ok(HeadlessResult::Tabs(
      workspace
        .tabs()
        .iter()
        .filter_map(|id| session.tabs.get(id))
        .map(|tab| HeadlessTab {
          id: format!("{}:{}:{}", session_id, workspace_name, tab.name),
          name: tab.name.clone(),
          title: tab.title.clone(),
          exit_code: tab.exit_code,
          active: workspace.active_tab() == Some(tab.id),
        })
        .collect(),
    ))
  }

  pub(super) fn headless_tab_rename(
    &mut self,
    id: Option<String>,
    name: String,
  ) -> Result<HeadlessResult, String> {
    let (session_id, workspace_name, old_name) = Self::split_tab_id(id)?;
    Self::validate_name(&name)?;
    let session = self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?;
    let workspace = session
      .model
      .workspaces()
      .iter()
      .find(|workspace| workspace.name() == workspace_name)
      .ok_or("Workspace does not exist")?;
    let tab_id = workspace
      .tabs()
      .iter()
      .find(|id| session.tabs.get(id).is_some_and(|tab| tab.name == old_name))
      .copied()
      .ok_or("Tab does not exist")?;
    if old_name != name
      && workspace
        .tabs()
        .iter()
        .any(|id| session.tabs.get(id).is_some_and(|tab| tab.name == name))
    {
      return Err(format!("Tab {name} already exists"));
    }
    session
      .tabs
      .get_mut(&tab_id)
      .ok_or("Tab does not exist")?
      .name = name;
    self.broadcast_state(&session_id)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_tab_focus(
    &mut self,
    id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, workspace_name, tab_name) = Self::split_tab_id(id)?;
    let session = self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?;
    let (workspace_id, workspace_tabs) = session
      .model
      .workspaces()
      .iter()
      .find(|workspace| workspace.name() == workspace_name)
      .map(|workspace| (workspace.id(), workspace.tabs().to_vec()))
      .ok_or("Workspace does not exist")?;
    let tab_id = workspace_tabs
      .iter()
      .find(|id| session.tabs.get(id).is_some_and(|tab| tab.name == tab_name))
      .copied()
      .ok_or("Tab does not exist")?;
    session.model.select_workspace(workspace_id);
    session.model.select_tab(tab_id);
    self.broadcast_state(&session_id)?;
    Ok(HeadlessResult::Accepted)
  }

  pub(super) fn headless_tab_remove(
    &mut self,
    id: Option<String>,
  ) -> Result<HeadlessResult, String> {
    let (session_id, workspace_name, tab_name) = Self::split_tab_id(id)?;
    let tab_id = self
      .session(&session_id)
      .and_then(|session| {
        session
          .model
          .workspaces()
          .iter()
          .find(|workspace| workspace.name() == workspace_name)
      })
      .and_then(|workspace| {
        workspace
          .tabs()
          .iter()
          .find(|id| {
            self
              .session(&session_id)
              .and_then(|session| session.tabs.get(id))
              .is_some_and(|tab| tab.name == tab_name)
          })
          .copied()
      })
      .ok_or("Tab does not exist")?;
    let outcome = self
      .session_mut(&session_id)
      .ok_or("Session does not exist")?
      .model
      .close_tab(tab_id);
    if let Some(session) = self.session_mut(&session_id) {
      session.tabs.remove(&tab_id);
    }
    if outcome == SessionTabCloseOutcome::SessionClosed {
      self
        .close_session(&session_id)
        .map_err(|error| error.to_string())?;
    } else {
      self.broadcast_state(&session_id)?;
    }
    Ok(HeadlessResult::Accepted)
  }
}
