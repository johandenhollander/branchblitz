//! App state: open repositories (tabs), their worker threads, the frame loop and settings.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use serde::{Deserialize, Serialize};

use crate::colors::{self, ColorRule, Coloring};
use crate::fonts;
use crate::git::{self, CommitDetails, DiffTarget, Op, RepoData, RepoState};
use crate::graph::{self, Graph};
use crate::settings_view;
use crate::theme::{self, DiffColors, Look, Palette, ThemeChoice};
use crate::views;
use crate::{ds, icons};

const SETTINGS_KEY: &str = "branchblitz.settings";
/// At most this many matches of a find in the whole history.
pub const FIND_HISTORY_MAX: usize = 500;

mod actions;
mod agent_tools;
mod automation;
mod chrome;
mod frame;
mod messages;
mod modals;
mod notices;
mod settings;
#[cfg(test)]
mod tests;
mod worker;

pub(crate) use chrome::split_message;
use modals::modal_title;
use notices::system_info;
pub use settings::{AuthorPlace, ColumnWidths, Settings};
use worker::spawn_worker;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    None,
    Wip,
    Commit(String),
}

enum Job {
    /// Reload; the number lets the UI tell when its latest request has been served.
    Load(u64),
    Op {
        op: Op,
        quiet: bool,
    },
}

pub struct Msg {
    tab: u64,
    kind: MsgKind,
}

// A few messages per git operation; boxing the repository data would buy nothing.
#[allow(clippy::large_enum_variant)]
enum MsgKind {
    Loaded(u64, Result<RepoData, String>),
    OpDone {
        op: Op,
        quiet: bool,
        result: Result<String, String>,
    },
    Details(String, Result<CommitDetails, String>),
    /// The message of HEAD, to prefill the commit fields for an amend.
    AmendMessage(Result<String, String>),
    Diff(u64, DiffTarget, Result<String, String>),
    PickedFolder(PathBuf),
    Agent(crate::agent::Event),
    /// The newest release on GitHub, or why it couldn't be asked.
    Latest(Result<Option<String>, String>),
    /// A clone finished (its folder), or failed.
    Cloned(Result<PathBuf, String>),
    /// The folder chosen to clone into.
    PickedCloneParent(PathBuf),
    /// The home folder chosen in Settings.
    PickedHomeDir(PathBuf),
    /// Find matches in the whole history: the query and the commits.
    FoundInHistory(String, Result<Vec<String>, String>),
    /// File history or blame (its generation).
    FileView(u64, Result<crate::file_view::Content, String>),
}

/// What the agent may do without asking the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentAccess {
    /// Asks before anything that publishes, rewrites or throws away work.
    #[default]
    Supervised,
    /// Asks only before what throws work away (force push, hard reset, discard, drop, abort).
    Auto,
    /// Never asks.
    Full,
}

/// What the UI asks for; handled after the frame is drawn.
pub enum Action {
    Op(Op),
    Confirm {
        title: String,
        body: String,
        op: Op,
        danger: bool,
    },
    Select(Selection),
    ShowDiff(DiffTarget),
    CloseDiff,
    /// Show the history or blame of a file (seen from commit `rev`, or HEAD / the working tree).
    FileView {
        kind: crate::file_view::Kind,
        path: String,
        rev: Option<String>,
    },
    CloseFileView,
    Jump(String),
    Input(InputKind),
    Copy(String),
    /// Show this other worktree of the repository in the same tab.
    SwitchWorktree(PathBuf),
    /// Load the next portion of older commits (scrolled to the end).
    LoadMore,
    /// Search the whole history for the find query (in the background).
    FindHistory(String),
    /// Load older commits until this one is loaded, then show it.
    LoadUntil(String),
    /// Open (or switch to) the repository at this path in its own tab.
    OpenRepo(PathBuf),
    /// Open a file of the repository (relative path) in the desktop's default app.
    OpenFile(String),
    /// Send the agent input of this tab, or stop the answer that is streaming in.
    AgentSend,
    AgentStop,
    /// Start a new conversation.
    AgentClear,
    /// The model for the next questions (`None`: Claude Code's own setting).
    AgentModel(Option<String>),
    AgentAccess(AgentAccess),
    /// Open or close the agent panel; show it at the size of the Settings window.
    AgentOpen(bool),
    AgentMaximize(bool),
    Fetch,
    /// Make this the mode the Pull button runs in this repository.
    /// The Pull button's default for this repository: `None` = Fetch All.
    SetPullMode(Option<git::PullMode>),
    /// "Amend previous commit" was switched on or off.
    Amend(bool),
    /// Ask for the branches to set up git flow with.
    FlowInit,
}

#[derive(Clone)]
pub enum InputKind {
    /// Opens the name field in the graph, on the commit's row; the branch is checked out.
    Branch {
        at: String,
    },
    Tag {
        at: String,
    },
    Rename {
        old: String,
    },
    /// The name of a new git-flow branch, without its prefix.
    FlowStart {
        kind: git::FlowKind,
    },
}

enum Modal {
    /// `tab` is the id of the repo the operation belongs to, which may no longer be the active one.
    Input {
        tab: u64,
        kind: InputKind,
        value: String,
    },
    Error {
        title: String,
        body: String,
    },
    Settings,
    /// Information, not an error (e.g. the WSL hint); `settings` offers a button to that page.
    Notice {
        title: String,
        body: String,
        settings: Option<settings_view::Page>,
    },
    /// Clone a repository: URL, the folder to clone into, running, git's error.
    Clone {
        url: String,
        parent: String,
        busy: bool,
        error: Option<String>,
        focus: bool,
    },
    /// Set up git flow: the production and the development branch.
    FlowInit {
        tab: u64,
        master: String,
        develop: String,
    },
}

pub struct RepoTab {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    pub data: Option<RepoData>,
    pub graph: Graph,
    pub coloring: Coloring,
    pub refs_at: HashMap<String, Vec<usize>>,
    pub row_of: HashMap<String, usize>,
    pub load_error: Option<String>,
    worker: Sender<Job>,
    pub running: VecDeque<String>,
    pub selected: Selection,
    pub details: Option<CommitDetails>,
    pub details_error: Option<String>,
    pub diff: Option<(DiffTarget, Option<Result<String, String>>)>,
    /// Lines chosen in the diff (indices in its parsed lines) to stage or unstage, and the
    /// line a Shift-click extends from.
    pub diff_selection: std::collections::BTreeSet<usize>,
    pub diff_anchor: Option<usize>,
    /// How many commits this tab loads; grows when scrolling to the end or finding older commits.
    pub limit: Arc<AtomicUsize>,
    pub loading_more: bool,
    /// Load further until this commit is loaded, then show it (a find match in older history).
    load_until: Option<String>,
    diff_gen: u64,
    /// File history or blame, in place of the graph (a diff opened from it shows on top).
    pub file_view: Option<crate::file_view::FileView>,
    file_gen: u64,
    pub filter: String,
    pub find: Option<Find>,
    pub agent: crate::agent::Conversation,
    /// The choice per conflict block of the conflicted file shown in the diff panel.
    pub conflict_choices: Vec<Option<crate::conflict::Choice>>,
    /// Hash of the file text the choices were made on.
    pub conflict_for: u64,
    pub scroll_to: Option<usize>,
    /// Summary line and description of the next commit.
    pub commit_msg: String,
    pub commit_body: String,
    pub amend: bool,
    /// Summary and description filled in from HEAD for an amend; cleared again on un-amend
    /// when left unchanged.
    amend_prefill: Option<(String, String)>,
    pub ref_col_w: f32,
    /// `None`: as wide as the lanes need.
    pub graph_col_w: Option<f32>,
    pub author_col_w: f32,
    pub date_col_w: f32,
    /// The name field for a new branch, shown in the graph on the row it starts at.
    pub new_branch: Option<NewBranch>,
    pub alert: Option<Alert>,
    last_fetch: Option<Instant>,
    loads_requested: u64,
    loads_done: u64,
    /// After a fetch or pull the worker still asks the remotes for their tags; its answer is the
    /// next load message.
    tags_pending: bool,
}

/// A question in the alert bar under the toolbar, before running `op`.
pub struct Alert {
    pub title: String,
    pub body: String,
    pub op: Op,
    /// Destructive: outlined in red, and Enter does not confirm it.
    pub danger: bool,
}

/// The find bar over the graph (Ctrl F): commits whose subject or hash match.
#[derive(Default)]
pub struct Find {
    pub query: String,
    /// Focus the field on the next frame.
    pub focus: bool,
    /// The query changed: go to the first match on the next frame, once the matches are known.
    pub jump: bool,
    /// Matches in the whole history (query, newest first), from git: some may not be loaded.
    pub older: Option<(String, Vec<String>)>,
    /// When to search the whole history (UI time): once typing pauses.
    pub search_at: Option<f64>,
    /// Searches of the whole history still running.
    pub searching: u32,
}

pub struct NewBranch {
    pub at: String,
    pub name: String,
    /// Focus was asked for; losing it from then on cancels.
    pub focused: bool,
}

impl RepoTab {
    pub fn is_fetching(&self) -> bool {
        self.running.iter().any(|l| l == "Fetch")
    }

    pub fn has_wip_row(&self) -> bool {
        self.data
            .as_ref()
            .is_some_and(|d| d.status.is_dirty() || d.state != RepoState::Clean)
    }
}

pub struct BranchBlitz {
    settings: Settings,
    tabs: Vec<RepoTab>,
    active: Option<usize>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
    next_id: u64,
    modal: Option<Modal>,
    open_path: String,
    status_line: Option<(bool, String)>,
    was_focused: bool,
    /// Driven by the MCP server: no window, so no dialogs of our own.
    headless: bool,
    /// Set by the MCP server for a single call whose caller explicitly allowed a destructive action.
    confirm_danger: bool,
    settings_page: settings_view::Page,
    /// The agent panel is shown at the size of the Settings window.
    agent_max: bool,
    /// Who answers in the agent panel.
    agent_backend: crate::agent::Backend,
    /// The GPU and graphics API in use, e.g. "Intel(R) Graphics (Vulkan)"; and whether the
    /// discrete GPU was asked for at this start.
    renderer: String,
    high_performance_at_start: bool,
    /// System, desktop and git version, for the diagnostics (asked once).
    system: String,
    /// The app's own MCP endpoint for its agent, and tool calls waiting for their result.
    live: Option<crate::live::Live>,
    live_pending: Vec<Pending>,
    /// Tab ids replaced by another (a tab switched to another worktree): old -> new.
    tab_alias: HashMap<u64, u64>,
}

/// An agent tool call that answers once the tab has the result.
struct Pending {
    tab: u64,
    reply: Sender<Result<String, String>>,
    /// Wait for the diff panel rather than for the operations.
    diff: bool,
    /// Said before the outcome.
    note: Option<String>,
    deadline: Instant,
}

/// What a tool call answers: now, or once the tab is idle (or its diff loaded).
enum Answer {
    Now(String),
    WhenIdle,
    WhenDiff,
    /// Once idle: this text, then the outcome.
    WhenIdleSaying(String),
}

impl BranchBlitz {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        colors::upgrade_rules(&mut settings.color_rules);
        settings.upgrade_sizes();
        settings.apply_style(&cc.egui_ctx);
        fonts::apply(
            &cc.egui_ctx,
            settings.ui_font.as_deref(),
            settings.mono_font.as_deref(),
        );
        cc.egui_ctx.set_theme(settings.preference());
        let (tx, rx) = mpsc::channel();
        let mut app = BranchBlitz {
            settings: settings.clone(),
            tabs: Vec::new(),
            active: None,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            next_id: 1,
            modal: None,
            open_path: String::new(),
            status_line: None,
            was_focused: true,
            headless: false,
            confirm_danger: false,
            settings_page: settings_view::Page::default(),
            agent_max: false,
            // A real agent only for the window (see `use_installed_agent`), never in tests.
            agent_backend: crate::agent::Backend::Preview,
            renderer: cc
                .wgpu_render_state
                .as_ref()
                .map(|r| {
                    let info = r.adapter.get_info();
                    format!("{} ({:?})", info.name, info.backend)
                })
                .or_else(|| {
                    use eframe::glow::HasContext;
                    // SAFETY: a plain glGetString on the context eframe made current.
                    let gl = cc.gl.as_ref()?;
                    let name = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                    Some(format!("{name} (OpenGL)"))
                })
                .unwrap_or_else(|| "unknown".into()),
            high_performance_at_start: settings.gpu_high_performance,
            system: system_info(),
            live: None,
            live_pending: Vec::new(),
            tab_alias: HashMap::new(),
        };
        for path in &settings.open_repos {
            if path.exists() {
                app.open_repo(path, false);
            }
        }
        if !settings.remember_tab_order {
            app.tabs.sort_by_key(|t| t.name.to_lowercase());
        }
        app.active = settings
            .active_repo
            .as_ref()
            .and_then(|p| app.tabs.iter().position(|t| &t.path == p))
            .or(if app.tabs.is_empty() { None } else { Some(0) });
        // `branchblitz <path>` opens (or focuses) that repository.
        if let Some(path) = open {
            app.open_repo(&path, true);
        } else if let Some(i) = app.active {
            app.fetch(i, true);
        }
        app
    }

    fn open_repo(&mut self, path: &Path, activate: bool) {
        let root = match git::toplevel(path) {
            Ok(r) => r,
            Err(e) => {
                self.modal = Some(Modal::Error {
                    title: "Not a git repository".into(),
                    body: e,
                });
                return;
            }
        };
        if let Some(i) = self.tabs.iter().position(|t| t.path == root) {
            if activate {
                self.switch_to(i);
            }
            return;
        }
        let tab = self.new_tab(root.clone());
        self.tabs.push(tab);
        self.settings.recent.retain(|p| p != &root);
        self.settings.recent.insert(0, root);
        self.settings.recent.truncate(15);
        if activate {
            let i = self.tabs.len() - 1;
            self.active = Some(i);
            self.fetch(i, true);
        }
    }

    /// A tab for the repository at `root` that starts loading.
    fn new_tab(&mut self, root: PathBuf) -> RepoTab {
        let id = self.next_id;
        self.next_id += 1;
        let saved = self
            .settings
            .columns
            .get(&root.display().to_string())
            .copied();
        let saved = saved.as_ref();
        let limit = Arc::new(AtomicUsize::new(self.settings.commit_limit));
        let worker = spawn_worker(
            id,
            root.clone(),
            self.tx.clone(),
            self.ctx.clone(),
            limit.clone(),
        );
        let name = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let _ = worker.send(Job::Load(1));
        RepoTab {
            id,
            path: root.clone(),
            name,
            data: None,
            graph: Graph::default(),
            coloring: Coloring::default(),
            refs_at: HashMap::new(),
            row_of: HashMap::new(),
            load_error: None,
            worker,
            running: VecDeque::new(),
            selected: Selection::None,
            details: None,
            details_error: None,
            diff: None,
            diff_gen: 0,
            file_view: None,
            file_gen: 0,
            filter: String::new(),
            find: None,
            diff_selection: Default::default(),
            diff_anchor: None,
            limit,
            loading_more: false,
            load_until: None,
            agent: Default::default(),
            conflict_choices: Vec::new(),
            conflict_for: 0,
            scroll_to: None,
            new_branch: None,
            alert: None,
            commit_msg: String::new(),
            commit_body: String::new(),
            amend: false,
            amend_prefill: None,
            ref_col_w: saved.map_or(200.0, |c| c.refs),
            graph_col_w: saved.and_then(|c| c.graph),
            author_col_w: saved.map_or(150.0, |c| c.author),
            date_col_w: saved.map_or(92.0, |c| c.date),
            last_fetch: None,
            loads_requested: 1,
            loads_done: 0,
            tags_pending: false,
        }
    }

    /// Shows another worktree of the same repository in tab `i` (or switches to the tab that
    /// already has it), like checking out the branch it has.
    fn switch_worktree(&mut self, i: usize, path: &Path) {
        let Ok(root) = git::toplevel(path) else {
            return self.open_repo(path, true);
        };
        if let Some(j) = self.tabs.iter().position(|t| t.path == root) {
            return self.switch_to(j);
        }
        let mut tab = self.new_tab(root.clone());
        let old = &self.tabs[i];
        tab.ref_col_w = old.ref_col_w;
        tab.graph_col_w = old.graph_col_w;
        tab.author_col_w = old.author_col_w;
        tab.date_col_w = old.date_col_w;
        tab.filter = old.filter.clone();
        // The conversation goes along (an agent may be the one switching).
        tab.agent = std::mem::take(&mut self.tabs[i].agent);
        self.tab_alias.insert(self.tabs[i].id, tab.id);
        self.tabs[i] = tab;
    }

    /// How an agent started for tab `i` reaches the app's tools.
    pub(crate) fn agent_attach(&self, i: usize) -> Option<crate::live::Attach> {
        let l = self.live.as_ref()?;
        Some(crate::live::Attach {
            exe: std::env::current_exe().ok()?,
            socket: l.socket.clone(),
            token: l.token.clone(),
            tab: self.tabs.get(i)?.id,
        })
    }

    /// Shows a real backend's controls (model choice) without starting it.
    #[cfg(test)]
    pub(crate) fn set_agent_backend(&mut self, b: crate::agent::Backend) {
        self.agent_backend = b;
    }

    #[cfg(test)]
    pub(crate) fn set_commit_limit(&mut self, n: usize) {
        self.settings.commit_limit = n;
        for t in &self.tabs {
            t.limit.store(n, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(crate) fn set_clone_parent(&mut self, p: PathBuf) {
        self.settings.clone_parent = Some(p);
    }

    #[cfg(test)]
    pub(crate) fn set_latest_version(&mut self, v: &str) {
        self.settings.latest_version = Some(v.into());
        self.settings.updates_checked = 1;
    }

    #[cfg(test)]
    pub(crate) fn agent_prompt_for_tests(&self) -> String {
        self.settings.agent_prompt()
    }

    #[cfg(test)]
    pub(crate) fn agent_access(&self) -> AgentAccess {
        self.settings.agent_access
    }

    #[cfg(all(test, unix))]
    pub(crate) fn set_agent_access(&mut self, a: AgentAccess) {
        self.settings.agent_access = a;
    }

    #[cfg(test)]
    pub(crate) fn agent_model(&self) -> Option<&str> {
        self.settings.agent_model.as_deref()
    }

    /// Offers the agent tools without a real agent (tests talk to the socket themselves).
    #[cfg(all(test, unix))]
    pub(crate) fn start_live(&mut self) {
        self.live = crate::live::Live::start(self.ctx.clone());
    }

    /// The tab with this id, following tabs that were replaced.
    fn tab_index(&self, mut id: u64) -> Option<usize> {
        for _ in 0..16 {
            if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
                return Some(i);
            }
            id = *self.tab_alias.get(&id)?;
        }
        None
    }

    /// Loads the next portion of older commits (the commit limit from Settings at a time).
    fn load_more(&mut self, i: usize) {
        let step = self.settings.commit_limit.max(100);
        let tab = &mut self.tabs[i];
        tab.limit.fetch_add(step, Ordering::Relaxed);
        tab.loading_more = true;
        self.reload(i);
    }

    fn switch_to(&mut self, i: usize) {
        if self.active == Some(i) {
            return;
        }
        self.active = Some(i);
        self.reload(i);
        if self.settings.fetch_on_switch {
            self.fetch(i, true);
        }
    }

    /// Moves tab `from` to before the tab now at `to` (`to` = the number of tabs: last).
    pub(crate) fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() {
            return;
        }
        let active = self.active.map(|a| self.tabs[a].id);
        let tab = self.tabs.remove(from);
        let to = if to > from { to - 1 } else { to }.min(self.tabs.len());
        self.tabs.insert(to, tab);
        self.active = active.and_then(|id| self.tabs.iter().position(|t| t.id == id));
    }

    fn close_tab(&mut self, i: usize) {
        self.tabs.remove(i);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > i => Some(a - 1),
            Some(a) if a == i => Some(a.min(self.tabs.len() - 1)),
            other => other,
        };
    }

    fn fetch(&mut self, i: usize, quiet: bool) {
        let tab = &mut self.tabs[i];
        if tab.is_fetching() {
            return;
        }
        tab.last_fetch = Some(Instant::now());
        self.submit(i, Op::Fetch, quiet);
    }

    /// Applies changed colour rules to every open repository.
    fn recolor(&mut self) {
        for tab in &mut self.tabs {
            if let Some(data) = &tab.data {
                tab.coloring = colors::assign(data, &self.settings.color_rules);
                tab.graph = graph::build(&data.commits, &tab.coloring.commit);
            }
        }
    }

    fn reload(&mut self, i: usize) {
        let tab = &mut self.tabs[i];
        tab.loads_requested += 1;
        let _ = tab.worker.send(Job::Load(tab.loads_requested));
    }

    fn submit(&mut self, i: usize, op: Op, quiet: bool) {
        let tab = &mut self.tabs[i];
        tab.running.push_back(op.label());
        let _ = tab.worker.send(Job::Op { op, quiet });
    }

    fn request_details(&self, i: usize, sha: String) {
        let tab = &self.tabs[i];
        let (tx, ctx, path, id) = (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
        std::thread::spawn(move || {
            let r = git::commit_details(&path, &sha);
            let _ = tx.send(Msg {
                tab: id,
                kind: MsgKind::Details(sha, r),
            });
            ctx.request_repaint();
        });
    }

    fn request_diff(&mut self, i: usize, target: DiffTarget) {
        let tab = &mut self.tabs[i];
        tab.diff_gen += 1;
        let generation = tab.diff_gen;
        let (tx, ctx, path, id) = (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
        std::thread::spawn(move || {
            let r = git::diff(&path, &target);
            let _ = tx.send(Msg {
                tab: id,
                kind: MsgKind::Diff(generation, target, r),
            });
            ctx.request_repaint();
        });
    }

    /// The desktop's default app for `path` (`xdg-open`; `open` on macOS, `explorer` on Windows).
    fn open_file(&mut self, path: &Path) {
        if self.headless {
            self.status_line = Some((
                false,
                format!("No editor in automation mode: {}", path.display()),
            ));
            return;
        }
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        let mut opener = std::process::Command::new(opener);
        let spawned = crate::child_env(&mut opener)
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            // Reap the opener when it exits (it hands the file to another process).
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => {
                self.status_line = Some((false, format!("Cannot open {}: {e}", path.display())))
            }
        }
    }

    /// Where Open and Clone start: the home folder from Settings, else ~/Projects or ~; without
    /// a window, the temp dir (tests and the MCP server stay out of the user's folders).
    pub(crate) fn home_dir(&self) -> PathBuf {
        if self.headless {
            return std::env::temp_dir();
        }
        if let Some(h) = self.settings.home_dir.as_ref().filter(|p| p.is_dir()) {
            return h.clone();
        }
        let home = std::env::home_dir().unwrap_or_default();
        Some(home.join("Projects"))
            .filter(|p| p.is_dir())
            .unwrap_or(home)
    }

    /// Opens the clone dialog, starting in the folder of the last clone, else the home folder.
    fn open_clone(&mut self) {
        let parent = match &self.settings.clone_parent {
            Some(p) => p.clone(),
            None => self.home_dir(),
        };
        self.modal = Some(Modal::Clone {
            url: String::new(),
            parent: parent.display().to_string(),
            busy: false,
            error: None,
            focus: true,
        });
    }

    /// Clones in the background; the answer comes as `MsgKind::Cloned`.
    fn start_clone(&mut self, url: String, dest: PathBuf) {
        crate::log::write("INFO", &format!("cloning {url} into {}", dest.display()));
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = git::clone_repo(&url, &dest);
            let _ = tx.send(Msg {
                tab: 0,
                kind: MsgKind::Cloned(r),
            });
            ctx.request_repaint();
        });
    }

    /// A clone finished: open it (and remember where it went), or show git's error.
    fn cloned(&mut self, r: Result<PathBuf, String>) {
        match r {
            Ok(path) => {
                self.settings.clone_parent = path.parent().map(Path::to_path_buf);
                if matches!(self.modal, Some(Modal::Clone { .. })) {
                    self.modal = None;
                }
                self.open_repo(&path, true);
            }
            Err(e) => {
                crate::log::write("ERROR", &format!("clone failed: {e}"));
                match &mut self.modal {
                    Some(Modal::Clone { busy, error, .. }) => {
                        *busy = false;
                        *error = Some(e);
                    }
                    _ => self.status_line = Some((false, format!("Clone failed: {e}"))),
                }
            }
        }
    }

    /// The folder picker for the clone dialog.
    fn pick_clone_parent(&mut self) {
        if self.headless {
            self.status_line = Some((false, "No folder dialog in automation mode".into()));
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new().set_title("Clone into").pick_folder() {
                let _ = tx.send(Msg {
                    tab: 0,
                    kind: MsgKind::PickedCloneParent(p),
                });
                ctx.request_repaint();
            }
        });
    }

    /// The folder picker for the home folder in Settings.
    fn pick_home_dir(&mut self) {
        if self.headless {
            self.status_line = Some((false, "No folder dialog in automation mode".into()));
            return;
        }
        let (tx, ctx, start) = (self.tx.clone(), self.ctx.clone(), self.home_dir());
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Home folder")
                .set_directory(start)
                .pick_folder()
            {
                let _ = tx.send(Msg {
                    tab: 0,
                    kind: MsgKind::PickedHomeDir(p),
                });
                ctx.request_repaint();
            }
        });
    }

    /// A new home folder: clones start there too, instead of where the last one went.
    pub(crate) fn set_home_dir(&mut self, p: Option<PathBuf>) {
        self.settings.home_dir = p;
        self.settings.clone_parent = None;
    }

    fn pick_folder(&mut self) {
        if self.headless {
            self.status_line = Some((
                false,
                "No folder dialog in automation mode; use open_repo".into(),
            ));
            return;
        }
        let (tx, ctx, start) = (self.tx.clone(), self.ctx.clone(), self.home_dir());
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Open repository")
                .set_directory(start)
                .pick_folder()
            {
                let _ = tx.send(Msg {
                    tab: 0,
                    kind: MsgKind::PickedFolder(p),
                });
                ctx.request_repaint();
            }
        });
    }
}

/// Why `name` cannot be a new branch or tag: taken, or not a valid ref name.
pub fn new_ref_problem(data: Option<&RepoData>, name: &str) -> Option<&'static str> {
    let exists = data.is_some_and(|d| {
        d.refs
            .iter()
            .any(|r| r.kind != git::RefKind::Remote && r.name == name)
    });
    if exists {
        Some("A ref with this name already exists")
    } else {
        ref_name_problem(name)
    }
}

/// Why `name` is not usable as a new branch or tag name (subset of `git check-ref-format`).
fn ref_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("Enter a name");
    }
    if name.starts_with('-') {
        return Some("A name cannot start with '-'");
    }
    let bad_char = name
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c));
    if bad_char
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with('.')
        || name.ends_with(".lock")
        || name == "@"
        || name.split('/').any(|part| part.starts_with('.'))
    {
        return Some("Not a valid git ref name");
    }
    None
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(p)
}
