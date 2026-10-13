use std::{borrow::Cow, path::PathBuf};

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SessionNumber(u64);

impl SessionNumber {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("Session Number space is exhausted")]
pub struct SessionNumberExhausted;

#[derive(Debug)]
pub struct SessionNumberAllocator {
    next: Option<u64>,
}

impl Default for SessionNumberAllocator {
    fn default() -> Self {
        Self { next: Some(1) }
    }
}

impl SessionNumberAllocator {
    pub fn allocate(&mut self) -> Result<SessionNumber, SessionNumberExhausted> {
        let number = self.next.ok_or(SessionNumberExhausted)?;
        self.next = number.checked_add(1);
        Ok(SessionNumber::new(number))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkspaceId(u64);

impl WorkspaceId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("a Session requires a Workspace containing at least one Tab")]
    EmptyWorkspace,
    #[error("Workspace ID already exists in this Session")]
    DuplicateWorkspace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionTabCloseOutcome {
    NotFound,
    TabClosed,
    WorkspaceClosed,
    SessionClosed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCloseOutcome {
    NotFound,
    WorkspaceEmpty,
    ActiveTabChanged(TabId),
    ActiveTabUnchanged(TabId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    id: WorkspaceId,
    working_directory: PathBuf,
    explicit_name: Option<String>,
    tabs: Vec<TabId>,
    active_tab: Option<TabId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    number: SessionNumber,
    workspaces: Vec<Workspace>,
    active_workspace: Option<WorkspaceId>,
}

impl Session {
    pub fn new(number: SessionNumber, initial_workspace: Workspace) -> Result<Self, SessionError> {
        if initial_workspace.tabs.is_empty() {
            return Err(SessionError::EmptyWorkspace);
        }
        let active_workspace = initial_workspace.id;
        Ok(Self {
            number,
            workspaces: vec![initial_workspace],
            active_workspace: Some(active_workspace),
        })
    }

    pub const fn number(&self) -> SessionNumber {
        self.number
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub const fn active_workspace_id(&self) -> Option<WorkspaceId> {
        self.active_workspace
    }

    pub fn active_workspace(&self) -> Option<&Workspace> {
        let active_workspace = self.active_workspace?;
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == active_workspace)
    }

    pub fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        let active_workspace = self.active_workspace?;
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == active_workspace)
    }

    pub fn workspace_mut(&mut self, workspace_id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
    }

    pub fn active_tab(&self) -> Option<TabId> {
        self.active_workspace()?.active_tab()
    }

    pub fn add_workspace(&mut self, workspace: Workspace) -> Result<(), SessionError> {
        if workspace.tabs.is_empty() {
            return Err(SessionError::EmptyWorkspace);
        }
        if self
            .workspaces
            .iter()
            .any(|existing| existing.id == workspace.id)
        {
            return Err(SessionError::DuplicateWorkspace);
        }
        self.active_workspace = Some(workspace.id);
        self.workspaces.push(workspace);
        Ok(())
    }

    pub fn select_workspace(&mut self, workspace_id: WorkspaceId) -> bool {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
            || self.active_workspace == Some(workspace_id)
        {
            return false;
        }
        self.active_workspace = Some(workspace_id);
        true
    }

    pub fn select_tab(&mut self, tab_id: TabId) -> bool {
        let Some(active_workspace) = self.active_workspace else {
            return false;
        };
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == active_workspace)
            .is_some_and(|workspace| workspace.select_tab(tab_id))
    }

    pub fn add_tab_to_active_workspace(&mut self, tab_id: TabId) -> bool {
        if self
            .workspaces
            .iter()
            .any(|workspace| workspace.tabs.contains(&tab_id))
        {
            return false;
        }
        let Some(active_workspace) = self.active_workspace else {
            return false;
        };
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == active_workspace)
            .is_some_and(|workspace| workspace.add_tab(tab_id))
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> SessionTabCloseOutcome {
        let Some(workspace_index) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.tabs.contains(&tab_id))
        else {
            return SessionTabCloseOutcome::NotFound;
        };
        let workspace_id = self.workspaces[workspace_index].id;
        let outcome = self.workspaces[workspace_index].close_tab(tab_id);
        if outcome == TabCloseOutcome::NotFound {
            return SessionTabCloseOutcome::NotFound;
        }
        if outcome != TabCloseOutcome::WorkspaceEmpty {
            return SessionTabCloseOutcome::TabClosed;
        }

        self.workspaces.remove(workspace_index);
        if self.workspaces.is_empty() {
            self.active_workspace = None;
            return SessionTabCloseOutcome::SessionClosed;
        }
        if self.active_workspace == Some(workspace_id) {
            let next_index = workspace_index.min(self.workspaces.len() - 1);
            self.active_workspace = Some(self.workspaces[next_index].id);
        }
        SessionTabCloseOutcome::WorkspaceClosed
    }
}

impl Workspace {
    pub fn new(id: WorkspaceId, working_directory: PathBuf, explicit_name: Option<String>) -> Self {
        Self {
            id,
            working_directory,
            explicit_name: explicit_name.filter(|name| !name.is_empty()),
            tabs: Vec::new(),
            active_tab: None,
        }
    }

    pub const fn id(&self) -> WorkspaceId {
        self.id
    }

    pub fn working_directory(&self) -> &std::path::Path {
        &self.working_directory
    }

    pub fn explicit_name(&self) -> Option<&str> {
        self.explicit_name.as_deref()
    }

    pub fn set_working_directory(&mut self, working_directory: PathBuf) {
        self.working_directory = working_directory;
    }

    pub fn set_name(&mut self, name: Option<String>) {
        self.explicit_name = name.filter(|name| !name.is_empty());
    }

    pub fn add_tab(&mut self, tab_id: TabId) -> bool {
        if self.tabs.contains(&tab_id) {
            return false;
        }
        self.tabs.push(tab_id);
        self.active_tab = Some(tab_id);
        true
    }

    pub const fn active_tab(&self) -> Option<TabId> {
        self.active_tab
    }

    pub fn select_tab(&mut self, tab_id: TabId) -> bool {
        if !self.tabs.contains(&tab_id) || self.active_tab == Some(tab_id) {
            return false;
        }
        self.active_tab = Some(tab_id);
        true
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> TabCloseOutcome {
        let Some(index) = self.tabs.iter().position(|candidate| *candidate == tab_id) else {
            return TabCloseOutcome::NotFound;
        };
        let was_active = self.active_tab == Some(tab_id);
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active_tab = None;
            return TabCloseOutcome::WorkspaceEmpty;
        }
        if !was_active && let Some(active_tab) = self.active_tab {
            return TabCloseOutcome::ActiveTabUnchanged(active_tab);
        }

        let next_active = self.tabs[index.min(self.tabs.len() - 1)];
        self.active_tab = Some(next_active);
        TabCloseOutcome::ActiveTabChanged(next_active)
    }

    pub fn tabs(&self) -> &[TabId] {
        &self.tabs
    }

    pub fn name(&self) -> Cow<'_, str> {
        if let Some(name) = &self.explicit_name {
            return Cow::Borrowed(name);
        }

        self.working_directory
            .file_name()
            .unwrap_or_else(|| self.working_directory.as_os_str())
            .to_string_lossy()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        Session, SessionNumber, SessionNumberAllocator, SessionTabCloseOutcome, TabCloseOutcome,
        TabId, Workspace, WorkspaceId,
    };

    fn workspace(id: u64, tab_ids: &[u64]) -> Workspace {
        let mut workspace = Workspace::new(
            WorkspaceId::new(id),
            PathBuf::from(format!("/work/{id}")),
            None,
        );
        for tab_id in tab_ids {
            assert!(workspace.add_tab(TabId::new(*tab_id)));
        }
        workspace
    }

    #[test]
    fn session_numbers_are_sequential_and_start_at_one() {
        let mut numbers = SessionNumberAllocator::default();

        assert_eq!(numbers.allocate().unwrap().get(), 1);
        assert_eq!(numbers.allocate().unwrap().get(), 2);
        assert_eq!(numbers.allocate().unwrap().get(), 3);
    }

    #[test]
    fn workspace_name_defaults_to_directory_basename_and_tracks_directory_edits() {
        let mut workspace = Workspace::new(WorkspaceId::new(1), PathBuf::from("/work/alpha"), None);

        assert_eq!(workspace.name(), "alpha");

        workspace.set_working_directory(PathBuf::from("/work/beta"));

        assert_eq!(workspace.name(), "beta");
    }

    #[test]
    fn explicit_workspace_name_survives_directory_edits_and_can_be_cleared() {
        let mut workspace = Workspace::new(
            WorkspaceId::new(2),
            PathBuf::from("/work/alpha"),
            Some("frontend".to_owned()),
        );
        workspace.set_working_directory(PathBuf::from("/work/beta"));

        assert_eq!(workspace.name(), "frontend");

        workspace.set_name(None);

        assert_eq!(workspace.name(), "beta");
    }

    #[test]
    fn empty_explicit_workspace_name_uses_directory_basename() {
        let workspace = Workspace::new(
            WorkspaceId::new(7),
            PathBuf::from("/work/alpha"),
            Some(String::new()),
        );

        assert_eq!(workspace.name(), "alpha");
    }

    #[test]
    fn newly_added_tab_becomes_the_workspace_active_tab() {
        let mut workspace = Workspace::new(WorkspaceId::new(3), PathBuf::from("/work"), None);
        let first = TabId::new(10);
        let second = TabId::new(11);

        assert!(workspace.add_tab(first));
        assert_eq!(workspace.active_tab(), Some(first));

        assert!(workspace.add_tab(second));
        assert_eq!(workspace.active_tab(), Some(second));
    }

    #[test]
    fn closing_active_tab_selects_the_tab_moved_into_its_position() {
        let mut workspace = Workspace::new(WorkspaceId::new(4), PathBuf::from("/work"), None);
        let tabs = [TabId::new(20), TabId::new(21), TabId::new(22)];
        for tab in tabs {
            assert!(workspace.add_tab(tab));
        }
        assert!(workspace.select_tab(tabs[1]));

        assert_eq!(
            workspace.close_tab(tabs[1]),
            TabCloseOutcome::ActiveTabChanged(tabs[2])
        );
        assert_eq!(workspace.active_tab(), Some(tabs[2]));
    }

    #[test]
    fn closing_final_tab_marks_workspace_for_cascading_removal() {
        let mut workspace = Workspace::new(WorkspaceId::new(5), PathBuf::from("/work"), None);
        let only_tab = TabId::new(30);
        assert!(workspace.add_tab(only_tab));

        assert_eq!(
            workspace.close_tab(only_tab),
            TabCloseOutcome::WorkspaceEmpty
        );
        assert!(workspace.tabs().is_empty());
        assert_eq!(workspace.active_tab(), None);
    }

    #[test]
    fn closing_inactive_tab_preserves_workspace_selection() {
        let mut workspace = Workspace::new(WorkspaceId::new(6), PathBuf::from("/work"), None);
        let first = TabId::new(40);
        let second = TabId::new(41);
        assert!(workspace.add_tab(first));
        assert!(workspace.add_tab(second));

        assert_eq!(
            workspace.close_tab(first),
            TabCloseOutcome::ActiveTabUnchanged(second)
        );
        assert_eq!(workspace.active_tab(), Some(second));
    }

    #[test]
    fn switching_workspaces_restores_each_workspace_last_active_tab() {
        let first_workspace = workspace(1, &[1, 2]);
        let first_workspace_id = first_workspace.id();
        let second_workspace = workspace(2, &[3]);
        let second_workspace_id = second_workspace.id();
        let mut session = Session::new(SessionNumber::new(7), first_workspace)
            .expect("a workspace with tabs can start a session");

        assert!(session.add_workspace(second_workspace).is_ok());
        assert_eq!(session.active_workspace_id(), Some(second_workspace_id));

        assert!(session.select_workspace(first_workspace_id));
        assert_eq!(session.active_tab(), Some(TabId::new(2)));
        assert!(session.add_tab_to_active_workspace(TabId::new(4)));
        assert_eq!(session.active_tab(), Some(TabId::new(4)));
        assert!(session.select_tab(TabId::new(1)));

        assert!(session.select_workspace(second_workspace_id));
        assert_eq!(session.active_tab(), Some(TabId::new(3)));

        assert!(session.select_workspace(first_workspace_id));
        assert_eq!(session.active_tab(), Some(TabId::new(1)));
    }

    #[test]
    fn closing_tabs_cascades_through_workspace_and_session() {
        let first_workspace = workspace(1, &[1, 2]);
        let first_workspace_id = first_workspace.id();
        let second_workspace = workspace(2, &[3]);
        let mut session = Session::new(SessionNumber::new(8), first_workspace)
            .expect("a workspace with tabs can start a session");
        session
            .add_workspace(second_workspace)
            .expect("new workspace contains a tab");

        assert_eq!(
            session.close_tab(TabId::new(3)),
            SessionTabCloseOutcome::WorkspaceClosed
        );
        assert_eq!(session.active_workspace_id(), Some(first_workspace_id));
        assert_eq!(session.active_tab(), Some(TabId::new(2)));

        assert_eq!(
            session.close_tab(TabId::new(1)),
            SessionTabCloseOutcome::TabClosed
        );
        assert_eq!(
            session.close_tab(TabId::new(2)),
            SessionTabCloseOutcome::SessionClosed
        );
        assert_eq!(session.active_workspace_id(), None);
    }
}
