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

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    pub open_repos: Vec<PathBuf>,
    pub active_repo: Option<PathBuf>,
    pub recent: Vec<PathBuf>,
    pub commit_limit: usize,
    pub fetch_on_switch: bool,
    /// Minutes between background fetches of the active tab, 0 = off.
    pub auto_fetch_minutes: u32,
    /// Branch colours, in priority order. Global for now; may become per repository.
    pub color_rules: Vec<ColorRule>,
    /// Name of the colour theme (see `theme::THEMES`); light/dark is `theme`.
    pub theme_name: String,
    /// 1.0 = as designed.
    pub contrast: f32,
    pub diff_colors: DiffColors,
    pub animation_ms: u32,
    /// System font families (None = bundled font) and sizes in points.
    pub ui_font: Option<String>,
    pub ui_font_size: f32,
    pub mono_font: Option<String>,
    pub mono_font_size: f32,
    /// Multiplier for mouse wheel and touchpad scrolling (egui alone is slower than a browser).
    pub scroll_speed: f32,
    /// The agent panel at the bottom of the right panel is open, and its height.
    pub agent_open: bool,
    pub agent_height: f32,
    /// The agent's model (a Claude Code alias such as `sonnet`); `None`: Claude Code's own setting.
    pub agent_model: Option<String>,
    pub agent_access: AgentAccess,
    /// Show the agent's tool calls in the conversation.
    pub agent_show_tools: bool,
    /// Extra instructions for the agent, in every conversation.
    pub agent_instructions: String,
    /// The discrete GPU instead of the integrated one (applies at the next start).
    pub gpu_high_performance: bool,
    /// Look once a day for a newer release; when it last looked (Unix seconds) and the newest
    /// version it saw.
    pub check_updates: bool,
    pub updates_checked: i64,
    pub latest_version: Option<String>,
    /// Where the last clone went (its parent folder): the next one starts there.
    pub clone_parent: Option<PathBuf>,
    /// The hint about scaling under WSL was shown.
    pub wsl_hint_shown: bool,
    /// What the Pull button runs, per repository path.
    pub pull_modes: HashMap<String, git::PullMode>,
    /// Repositories where the Pull button fetches instead (its default is "Fetch All").
    pub fetch_default: std::collections::HashSet<String>,
    /// Commit list column widths per repository.
    pub columns: HashMap<String, ColumnWidths>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnWidths {
    pub refs: f32,
    pub graph: Option<f32>,
    pub author: f32,
    pub date: f32,
}

impl Settings {
    pub fn look(&self) -> Look {
        Look {
            contrast: self.contrast.clamp(0.5, 1.5),
            animation_ms: self.animation_ms.min(500),
            ui_size: self.ui_font_size.clamp(11.0, 18.0),
            mono_size: self.mono_font_size.clamp(10.0, 18.0),
            diff: self.diff_colors,
        }
    }

    pub fn palette(&self, theme: egui::Theme) -> Palette {
        Palette::new(theme::theme_def(&self.theme_name), theme, &self.look())
    }

    /// Font sizes saved with the defaults of before the design system (13.5 / 12.5) move to
    /// its sizes (`body` 13, `code` 12); sizes the user chose stay.
    fn upgrade_sizes(&mut self) {
        if self.ui_font_size == 13.5 && self.mono_font_size == 12.5 {
            self.ui_font_size = Look::default().ui_size;
            self.mono_font_size = Look::default().mono_size;
        }
    }

    pub fn pull_mode(&self, repo: &Path) -> git::PullMode {
        self.pull_modes
            .get(&repo.display().to_string())
            .copied()
            .unwrap_or(git::PullMode::Merge)
    }

    /// What the agent is told besides its standard instructions: how much it may do without
    /// asking, and the user's own instructions.
    pub fn agent_prompt(&self) -> String {
        let access = match self.agent_access {
            AgentAccess::Supervised => {
                "Access: supervised. Actions that publish, rewrite or throw away work are put to \
                 the user in a red bar by the app; just call the tool, the app asks."
            }
            AgentAccess::Auto => {
                "Access: auto. Only actions that throw work away (force push, hard reset, \
                 discarding changes, dropping a stash, aborting) are put to the user by the app; \
                 the rest runs directly."
            }
            AgentAccess::Full => {
                "Access: full. The user lets every tool run without asking; still only do what \
                 they asked for."
            }
        };
        let own = self.agent_instructions.trim();
        if own.is_empty() {
            access.to_string()
        } else {
            format!("{access}\n\nThe user's instructions:\n{own}")
        }
    }

    /// Light, dark or the system's; a theme made for one mode decides it.
    pub fn preference(&self) -> egui::ThemePreference {
        match theme::theme_def(&self.theme_name).mode {
            Some(egui::Theme::Dark) => egui::ThemePreference::Dark,
            Some(egui::Theme::Light) => egui::ThemePreference::Light,
            None => match (self.theme, crate::color_scheme::current()) {
                (ThemeChoice::System, Some(egui::Theme::Dark)) => egui::ThemePreference::Dark,
                (ThemeChoice::System, Some(egui::Theme::Light)) => egui::ThemePreference::Light,
                _ => self.theme.preference(),
            },
        }
    }

    /// Applies theme, sizes, contrast and animation speed to egui's styles.
    pub fn apply_style(&self, ctx: &egui::Context) {
        theme::setup_style(ctx, theme::theme_def(&self.theme_name), &self.look());
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::System,
            open_repos: Vec::new(),
            active_repo: None,
            recent: Vec::new(),
            commit_limit: 2000,
            fetch_on_switch: true,
            auto_fetch_minutes: 0,
            color_rules: colors::default_rules(),
            theme_name: theme::DEFAULT_THEME.into(),
            contrast: 1.0,
            diff_colors: DiffColors::RedGreen,
            animation_ms: Look::default().animation_ms,
            ui_font: None,
            ui_font_size: Look::default().ui_size,
            mono_font: None,
            mono_font_size: Look::default().mono_size,
            scroll_speed: 2.0,
            agent_open: false,
            agent_height: 320.0,
            agent_model: None,
            agent_access: AgentAccess::default(),
            agent_show_tools: false,
            agent_instructions: String::new(),
            gpu_high_performance: false,
            check_updates: true,
            updates_checked: 0,
            latest_version: None,
            clone_parent: None,
            wsl_hint_shown: false,
            pull_modes: HashMap::new(),
            fetch_default: Default::default(),
            columns: HashMap::new(),
        }
    }
}

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
    Jump(String),
    Input(InputKind),
    Copy(String),
    /// Show this other worktree of the repository in the same tab.
    SwitchWorktree(PathBuf),
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
    diff_gen: u64,
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
    limit: Arc<AtomicUsize>,
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
            limit: Arc::new(AtomicUsize::new(settings.commit_limit)),
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
        let worker = spawn_worker(
            id,
            root.clone(),
            self.tx.clone(),
            self.ctx.clone(),
            self.limit.clone(),
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
            filter: String::new(),
            find: None,
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

    /// Opens the clone dialog, starting in the folder of the last clone (else ~/Projects or ~;
    /// without a window, the temp dir: tests and the MCP server stay out of the user's folders).
    fn open_clone(&mut self) {
        let home = if self.headless {
            std::env::temp_dir()
        } else {
            std::env::home_dir().unwrap_or_default()
        };
        let parent = self
            .settings
            .clone_parent
            .clone()
            .or_else(|| Some(home.join("Projects")).filter(|p| p.is_dir()))
            .unwrap_or(home);
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

    fn pick_folder(&mut self) {
        if self.headless {
            self.status_line = Some((
                false,
                "No folder dialog in automation mode; use open_repo".into(),
            ));
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .set_title("Open repository")
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

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if let MsgKind::PickedFolder(p) = &msg.kind {
                let p = p.clone();
                self.open_repo(&p, true);
                continue;
            }
            if let MsgKind::Cloned(r) = &msg.kind {
                self.cloned(r.clone());
                continue;
            }
            if let MsgKind::PickedCloneParent(p) = &msg.kind {
                if let Some(Modal::Clone { parent, .. }) = &mut self.modal {
                    *parent = p.display().to_string();
                }
                continue;
            }
            if let MsgKind::Latest(r) = &msg.kind {
                match r {
                    Ok(v) => {
                        self.settings.latest_version = v.clone();
                        self.settings.updates_checked = chrono::Utc::now().timestamp();
                    }
                    Err(e) => crate::log::write("INFO", &format!("update check: {e}")),
                }
                continue;
            }
            // Agent events follow their conversation into a replaced tab; the rest belongs to
            // the old worker.
            let i = match &msg.kind {
                MsgKind::Agent(_) => self.tab_index(msg.tab),
                _ => self.tabs.iter().position(|t| t.id == msg.tab),
            };
            let Some(i) = i else {
                continue;
            };
            match msg.kind {
                MsgKind::Loaded(generation, Ok(data)) => {
                    let coloring = colors::assign(&data, &self.settings.color_rules);
                    let tab = &mut self.tabs[i];
                    tab.loads_done = tab.loads_done.max(generation);
                    tab.tags_pending = false;
                    tab.graph = graph::build(&data.commits, &coloring.commit);
                    tab.coloring = coloring;
                    tab.refs_at = git::refs_by_commit(&data.refs);
                    tab.row_of = data
                        .commits
                        .iter()
                        .enumerate()
                        .map(|(n, c)| (c.sha.clone(), n))
                        .collect();
                    tab.load_error = None;
                    let dirty = data.status.is_dirty() || data.state != RepoState::Clean;
                    tab.data = Some(data);
                    if tab.selected == Selection::Wip && !dirty {
                        tab.selected = Selection::None;
                    }
                    if let Selection::Commit(sha) = &tab.selected
                        && !tab.row_of.contains_key(sha)
                    {
                        tab.selected = Selection::None;
                        tab.details = None;
                    }
                    // A resolved conflict leaves the panel; others follow the working tree.
                    if let Some((DiffTarget::Conflict { path }, _)) = &tab.diff
                        && tab
                            .data
                            .as_ref()
                            .is_some_and(|d| !d.status.conflicted.contains(path))
                    {
                        tab.diff = None;
                        tab.conflict_choices.clear();
                    }
                    let refresh = match &tab.diff {
                        Some((
                            t @ (DiffTarget::Unstaged { .. }
                            | DiffTarget::Staged { .. }
                            | DiffTarget::Conflict { .. }),
                            _,
                        )) => Some(t.clone()),
                        _ => None,
                    };
                    if let Some(t) = refresh {
                        self.request_diff(i, t);
                    }
                }
                MsgKind::Loaded(generation, Err(e)) => {
                    crate::log::write(
                        "ERROR",
                        &format!("loading {}: {e}", self.tabs[i].path.display()),
                    );
                    let tab = &mut self.tabs[i];
                    tab.loads_done = tab.loads_done.max(generation);
                    tab.tags_pending = false;
                    tab.load_error = Some(e);
                }
                MsgKind::OpDone { op, quiet, result } => {
                    let tab = &mut self.tabs[i];
                    tab.running.pop_front();
                    tab.tags_pending = result.is_ok() && matches!(op, Op::Fetch | Op::Pull(_));
                    match result {
                        Ok(out) => {
                            if matches!(op, Op::Commit { .. }) {
                                tab.commit_msg.clear();
                                tab.commit_body.clear();
                                tab.amend = false;
                                tab.amend_prefill = None;
                            }
                            let first = out
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let text = if first.is_empty() {
                                format!("{} done", op.label())
                            } else {
                                format!("{}: {}", op.label(), first)
                            };
                            self.status_line = Some((true, text));
                        }
                        Err(e) => {
                            crate::log::write(
                                "ERROR",
                                &format!(
                                    "{} failed in {}: {e}",
                                    op.label(),
                                    self.tabs[i].path.display()
                                ),
                            );
                            self.status_line = Some((
                                false,
                                format!(
                                    "{} failed: {}",
                                    op.label(),
                                    e.lines().next().unwrap_or("")
                                ),
                            ));
                            if let Op::DeleteBranch { name, force: false } = &op
                                && e.contains("not fully merged")
                            {
                                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == msg.tab)
                                    && t.alert.is_none()
                                {
                                    t.alert = Some(Alert {
                                        title: format!("Force delete {name}?"),
                                        body: format!(
                                            "Branch {name} is not fully merged. Deleting it may lose commits."
                                        ),
                                        op: Op::DeleteBranch {
                                            name: name.clone(),
                                            force: true,
                                        },
                                        danger: true,
                                    });
                                }
                                continue;
                            }
                            // Stopping on conflicts is an outcome, not a failure: the banner and
                            // the conflict view take over.
                            // The worker reloads before it reports, so the status is known; with
                            // nothing conflicted it was another error after all.
                            let first = self.tabs[i]
                                .data
                                .as_ref()
                                .and_then(|d| d.status.conflicted.first().cloned());
                            if e.contains("CONFLICT")
                                && let Some(path) = first
                            {
                                self.status_line =
                                    Some((true, format!("{} stopped on conflicts", op.label())));
                                let tab = &mut self.tabs[i];
                                tab.selected = Selection::Wip;
                                tab.details = None;
                                self.apply(
                                    i,
                                    vec![Action::ShowDiff(DiffTarget::Conflict { path })],
                                );
                                continue;
                            }
                            if !quiet {
                                self.offer_modal(Modal::Error {
                                    title: format!("{} failed", op.label()),
                                    body: e,
                                });
                            }
                        }
                    }
                }
                MsgKind::Details(sha, r) => {
                    let tab = &mut self.tabs[i];
                    if tab.selected == Selection::Commit(sha) {
                        match r {
                            Ok(d) => tab.details = Some(d),
                            Err(e) => tab.details_error = Some(e),
                        }
                    }
                }
                MsgKind::AmendMessage(r) => {
                    let tab = &mut self.tabs[i];
                    match r {
                        // Only fill fields the user has not started on.
                        Ok(msg)
                            if tab.amend
                                && tab.commit_msg.trim().is_empty()
                                && tab.commit_body.trim().is_empty() =>
                        {
                            let (summary, body) = split_message(&msg);
                            tab.commit_msg = summary.clone();
                            tab.commit_body = body.clone();
                            tab.amend_prefill = Some((summary, body));
                        }
                        Ok(_) => {}
                        Err(e) => self.status_line = Some((false, e)),
                    }
                }
                MsgKind::Diff(generation, target, r) => {
                    let tab = &mut self.tabs[i];
                    if let Some((t, content)) = &mut tab.diff
                        && *t == target
                        && generation == tab.diff_gen
                    {
                        *content = Some(r);
                    }
                }
                MsgKind::Agent(event) => {
                    if let crate::agent::Event::Error(e) = &event {
                        crate::log::write("AGENT", e);
                    }
                    self.tabs[i].agent.apply(event)
                }
                MsgKind::PickedFolder(_)
                | MsgKind::Latest(_)
                | MsgKind::Cloned(_)
                | MsgKind::PickedCloneParent(_) => unreachable!(),
            }
        }
    }

    /// Shows a modal that was not directly asked for (a failed background operation),
    /// without throwing away a dialog the user is typing in. The status bar still shows it.
    fn offer_modal(&mut self, modal: Modal) {
        if self.modal.is_none() {
            self.modal = Some(modal);
        }
    }

    pub(crate) fn apply(&mut self, i: usize, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Op(op) => self.submit(i, op, false),
                Action::Confirm {
                    title,
                    body,
                    op,
                    danger,
                } => {
                    self.tabs[i].alert = Some(Alert {
                        title,
                        body,
                        op,
                        danger,
                    })
                }
                Action::Select(sel) => {
                    let tab = &mut self.tabs[i];
                    if tab.selected == sel {
                        continue;
                    }
                    tab.selected = sel.clone();
                    tab.details = None;
                    tab.details_error = None;
                    if let Selection::Commit(sha) = sel {
                        self.request_details(i, sha);
                    }
                }
                Action::ShowDiff(target) => {
                    self.tabs[i].conflict_choices.clear();
                    self.tabs[i].diff = Some((target.clone(), None));
                    self.request_diff(i, target);
                }
                Action::CloseDiff => self.tabs[i].diff = None,
                Action::Jump(sha) => {
                    let tab = &mut self.tabs[i];
                    if let Some(&row) = tab.row_of.get(&sha) {
                        tab.scroll_to = Some(row + tab.has_wip_row() as usize);
                    }
                    self.apply(i, vec![Action::Select(Selection::Commit(sha))]);
                }
                // On a row of the graph; a commit outside the loaded history gets the dialog.
                Action::Input(InputKind::Branch { at })
                    if self.tabs[i].row_of.contains_key(&at) =>
                {
                    let tab = &mut self.tabs[i];
                    let row = tab.row_of[&at];
                    tab.scroll_to = Some(row + tab.has_wip_row() as usize);
                    tab.new_branch = Some(NewBranch {
                        at,
                        name: String::new(),
                        focused: false,
                    });
                }
                Action::Input(kind) => {
                    let value = match &kind {
                        InputKind::Rename { old } => old.clone(),
                        _ => String::new(),
                    };
                    self.modal = Some(Modal::Input {
                        tab: self.tabs[i].id,
                        kind,
                        value,
                    });
                }
                Action::Copy(s) => self.ctx.copy_text(s),
                Action::SwitchWorktree(path) => self.switch_worktree(i, &path),
                Action::OpenRepo(path) => self.open_repo(&path, true),
                Action::AgentSend => {
                    let (tx, ctx, id) = (self.tx.clone(), self.ctx.clone(), self.tabs[i].id);
                    let attach = self.agent_attach(i);
                    let model = self.settings.agent_model.as_deref();
                    let extra = self.settings.agent_prompt();
                    let tab = &mut self.tabs[i];
                    tab.agent.send(
                        &self.agent_backend,
                        &tab.path,
                        attach,
                        model,
                        extra,
                        move |event| {
                            let _ = tx.send(Msg {
                                tab: id,
                                kind: MsgKind::Agent(event),
                            });
                            ctx.request_repaint();
                        },
                    );
                }
                Action::AgentStop => self.tabs[i].agent.stop(),
                Action::AgentClear => self.tabs[i].agent.clear(),
                Action::AgentModel(m) => self.settings.agent_model = m,
                Action::AgentAccess(a) => self.settings.agent_access = a,
                Action::AgentOpen(open) => {
                    self.settings.agent_open = open;
                    if !open {
                        self.agent_max = false;
                    }
                }
                Action::AgentMaximize(max) => {
                    self.agent_max = max;
                    if max {
                        self.settings.agent_open = true;
                    }
                }
                Action::OpenFile(rel) => {
                    let path = self.tabs[i].path.join(rel);
                    self.open_file(&path);
                }
                Action::Fetch => self.fetch(i, false),
                Action::Amend(true) => {
                    let tab = &self.tabs[i];
                    let Some(sha) = tab.data.as_ref().and_then(|d| d.head_sha.clone()) else {
                        continue;
                    };
                    let (tx, ctx, path, id) =
                        (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
                    std::thread::spawn(move || {
                        let r = git::commit_details(&path, &sha).map(|d| d.message);
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::AmendMessage(r),
                        });
                        ctx.request_repaint();
                    });
                }
                Action::Amend(false) => {
                    let tab = &mut self.tabs[i];
                    if let Some((summary, body)) = tab.amend_prefill.take()
                        && tab.commit_msg == summary
                        && tab.commit_body == body
                    {
                        tab.commit_msg.clear();
                        tab.commit_body.clear();
                    }
                }
                Action::SetPullMode(mode) => {
                    let key = self.tabs[i].path.display().to_string();
                    match mode {
                        Some(mode) => {
                            self.settings.fetch_default.remove(&key);
                            self.settings.pull_modes.insert(key, mode);
                        }
                        None => {
                            self.settings.fetch_default.insert(key);
                        }
                    }
                }
            }
        }
    }

    fn tab_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let mut switch = None;
        let mut close = None;
        let animate = self.settings.animation_ms > 0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, tab) in self.tabs.iter().enumerate() {
                let active = self.active == Some(i);
                let name: String = if tab.name.chars().count() > 24 {
                    tab.name.chars().take(23).chain(['…']).collect()
                } else {
                    tab.name.clone()
                };
                // A merge, rebase or conflicts in progress: a warning dot before the name.
                let flagged = tab.data.as_ref().is_some_and(|d| {
                    d.state != RepoState::Clean || !d.status.conflicted.is_empty()
                });
                let id = ui.scope_id().with(("repo-tab", tab.id));
                let hovered_before = ui.ctx().read_response(id).is_some_and(|r| r.hovered())
                    || ui
                        .ctx()
                        .read_response(id.with("close"))
                        .is_some_and(|r| r.hovered());
                let fg = if active || hovered_before {
                    pal.text
                } else {
                    pal.text_muted
                };
                let g = ui
                    .painter()
                    .layout_job(ds::text_job(&name, 12.5, 500.0, fg, false));
                let dot_w = if flagged { 6.0 + 8.0 } else { 0.0 };
                let w = 12.0 + dot_w + g.size().x + 8.0 + 18.0 + 8.0;
                let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 30.0), egui::Sense::hover());
                let resp = ui.interact(rect, id, egui::Sense::click());
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, active, &tab.name)
                });
                let r = egui::CornerRadius {
                    nw: theme::RADIUS_MD,
                    ne: theme::RADIUS_MD,
                    sw: 0,
                    se: 0,
                };
                let p = ui.painter();
                let busy = !tab.running.is_empty();
                if active {
                    p.rect_filled(rect, r, pal.app);
                }
                if active && !busy {
                    p.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left() + 8.0, rect.top()),
                            egui::pos2(rect.right() - 8.0, rect.top() + 2.0),
                        ),
                        egui::CornerRadius {
                            sw: 2,
                            se: 2,
                            ..Default::default()
                        },
                        pal.accent,
                    );
                }
                if !active && hovered_before {
                    p.rect_filled(rect, r, pal.hover);
                }
                let yc = rect.center().y;
                let mut x = rect.left() + 12.0;
                if flagged {
                    p.circle_filled(egui::pos2(x + 3.0, yc), 3.0, pal.warning);
                    x += dot_w;
                }
                p.galley(egui::pos2(x, yc - g.size().y / 2.0), g, fg);
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(rect.right() - 8.0 - 9.0, yc),
                    egui::vec2(18.0, 18.0),
                );
                if active || hovered_before {
                    let x = ui.interact(close_rect, id.with("close"), egui::Sense::click());
                    x.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            format!("Close {}", tab.name),
                        )
                    });
                    if x.hovered() {
                        ui.painter().rect_filled(
                            close_rect,
                            egui::CornerRadius::same(theme::RADIUS_XS),
                            pal.hover,
                        );
                    }
                    ds::paint_icon(
                        ui.painter(),
                        close_rect.center(),
                        icons::X,
                        12.0,
                        if x.hovered() {
                            pal.text
                        } else {
                            pal.text_subtle
                        },
                    );
                    if x.clicked() {
                        close = Some(i);
                    }
                }
                // Busy: the top indicator line animates (on inactive tabs too).
                if busy {
                    ds::busy_line(ui, rect, pal, animate);
                }
                if resp.middle_clicked() {
                    close = Some(i);
                } else if resp.clicked() && close.is_none() {
                    switch = Some(i);
                }
                resp.on_hover_text(tab.path.display().to_string());
            }
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
            let rect = egui::Rect::from_center_size(
                rect.center() - egui::vec2(0.0, 1.0),
                egui::vec2(26.0, 26.0),
            );
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, true, "Open repository")
            });
            if resp.hovered() {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(theme::RADIUS_MD),
                    pal.hover,
                );
            }
            ds::paint_icon(
                ui.painter(),
                rect.center(),
                icons::PLUS,
                16.0,
                if resp.hovered() {
                    pal.text
                } else {
                    pal.text_subtle
                },
            );
            if resp.on_hover_text("Open repository").clicked() {
                self.active = None;
            }
        });
        if let Some(i) = close {
            self.close_tab(i);
        } else if let Some(i) = switch {
            self.switch_to(i);
        }
    }

    /// Three zones: the branch switcher, the actions fixed in the centre of the window, and
    /// settings at the right.
    fn toolbar(&mut self, ui: &mut egui::Ui, i: usize, pal: &Palette) -> Vec<Action> {
        let mut acts = Vec::new();
        let tab = &self.tabs[i];
        let data = tab.data.as_ref();
        let current = data.and_then(|d| d.current_ref());
        let (ahead, behind) = current.map(|r| (r.ahead, r.behind)).unwrap_or((0, 0));
        let dirty = data.is_some_and(|d| d.status.is_dirty());
        let has_stash = data.is_some_and(|d| !d.stashes.is_empty());
        let head_sha = data.and_then(|d| d.head_sha.clone());
        let pull_mode = self.settings.pull_mode(&tab.path);
        let animate = self.settings.animation_ms > 0;
        let running = |name: &str| tab.running.iter().any(|l| l == name);

        let full = ui.max_rect();
        let width = ui.ctx().content_rect().width();
        // Narrow windows: labels collapse to icons, the local group first.
        let (sync_labels, local_labels) = (width >= 940.0, width >= 1100.0);
        // The group is centred on its idle width (measured last frame), so busy labels such as
        // "Pulling…" never move the buttons.
        let rest_id = ui.scope_id().with("toolbar-rest");
        let rest_w: f32 = ui.ctx().data(|d| d.get_temp(rest_id)).unwrap_or(520.0);
        let gap = 16.0;
        let anchor = (full.center().x - rest_w / 2.0).round();

        // Start: the branch switcher, truncating instead of pushing the actions.
        let start = egui::Rect::from_min_max(
            full.min,
            egui::pos2((anchor - gap).max(full.left()), full.bottom()),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(start)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                if let Some(a) = branch_switcher(ui, pal, tab) {
                    acts.push(a);
                }
            },
        );

        let label = |s: &str, show: bool| if show { s.to_string() } else { String::new() };

        // Centre: sync group | local group, starting at the fixed anchor.
        let center = egui::Rect::from_min_max(
            egui::pos2(anchor, full.top()),
            egui::pos2(full.right(), full.bottom()),
        );
        let used = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(center)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // GitKraken style: one button for pull or fetch, its default chosen below.
                    let fetch_default = self
                        .settings
                        .fetch_default
                        .contains(&tab.path.display().to_string());
                    let fetching = tab.is_fetching();
                    let pulling = running("Pull") || (fetch_default && fetching);
                    let (text, busy_text, icon) = if fetch_default {
                        ("Fetch", "Fetching…", icons::REFRESH_CW)
                    } else {
                        ("Pull", "Pulling…", icons::ARROW_DOWN_TO_LINE)
                    };
                    let mut pull = ds::Button::new(label(if pulling { busy_text } else { text }, sync_labels))
                        .icon(icon)
                        .label(text)
                        .min_width(if sync_labels { 118.0 } else { 0.0 })
                        .count((behind > 0).then(|| format!("↓{behind}")))
                        .busy(pulling, animate)
                        .enabled(fetch_default || current.is_some())
                        .tooltip(if fetch_default {
                            "Fetch all remotes (git fetch --all --prune)"
                        } else {
                            pull_mode_label(pull_mode)
                        });
                    if fetch_default {
                        pull = pull.spin_icon();
                    }
                    let resp = ds::split(ui, pal, pull, "Pull options", |ui| {
                        ui.set_width(300.0);
                        // Indented like the menu items below it.
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(8, 4))
                            .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        "Select a default pull/fetch operation to execute when \
                                         clicking this button. Click a name to run it once.",
                                    )
                                    .color(pal.text_muted),
                                )
                                .wrap(),
                            );
                            });
                        ds::menu_divider(ui, pal);
                        let choices = [
                            (None, "Fetch All"),
                            (Some(git::PullMode::Merge), pull_mode_label(git::PullMode::Merge)),
                            (
                                Some(git::PullMode::FastForwardOnly),
                                pull_mode_label(git::PullMode::FastForwardOnly),
                            ),
                            (Some(git::PullMode::Rebase), pull_mode_label(git::PullMode::Rebase)),
                        ];
                        let chosen = if fetch_default { None } else { Some(pull_mode) };
                        for (mode, text) in choices {
                            // Same guards as the button: one pull at a time, and only on a branch.
                            let runnable = match mode {
                                None => !fetching,
                                Some(_) => !running("Pull") && !fetching && current.is_some(),
                            };
                            let (make_default, run) =
                                ds::menu_radio_item(ui, pal, mode == chosen, text, runnable);
                            if make_default {
                                // Only the default changes; the menu stays open to show it.
                                acts.push(Action::SetPullMode(mode));
                            }
                            if run {
                                // Run once, keeping the default as it is.
                                acts.push(match mode {
                                    None => Action::Fetch,
                                    Some(m) => Action::Op(Op::Pull(m)),
                                });
                                ui.close();
                            }
                        }
                    });
                    if resp.clicked() && !pulling {
                        acts.push(if fetch_default {
                            Action::Fetch
                        } else {
                            Action::Op(Op::Pull(pull_mode))
                        });
                    }
                    let pushing = running("Push");
                    let branch = data.and_then(|d| d.head_branch.clone()).unwrap_or_default();
                    let push = ds::Button::new(label(if pushing { "Pushing…" } else { "Push" }, sync_labels))
                        .icon(icons::ARROW_UP_FROM_LINE)
                        .label("Push")
                        .min_width(if sync_labels { 110.0 } else { 0.0 })
                        .count((ahead > 0).then(|| format!("↑{ahead}")))
                        .busy(pushing, animate)
                        .enabled(current.is_some())
                        .tooltip("Push");
                    let resp = ds::split(ui, pal, push, "Push options", |ui| {
                        if ds::menu_item(ui, pal, icons::ARROW_UP_FROM_LINE, "Push", None, false).clicked() {
                            acts.push(Action::Op(Op::Push { force: false }));
                            ui.close();
                        }
                        ds::menu_divider(ui, pal);
                        if ds::menu_item(ui, pal, icons::TRIANGLE_ALERT, "Force push (with lease)", None, true)
                            .clicked()
                        {
                            acts.push(Action::Confirm {
                                title: format!("Force push {branch}?"),
                                body: format!(
                                    "This overwrites {branch} on the remote with your local branch (--force-with-lease)."
                                ),
                                op: Op::Push { force: true },
                                danger: true,
                            });
                            ui.close();
                        }
                    });
                    if resp.clicked() && !pushing {
                        acts.push(Action::Op(Op::Push { force: false }));
                    }
                    ui.add_space(gap - 4.0);
                    let (sep, _) = ui.allocate_exact_size(egui::vec2(1.0, 20.0), egui::Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.border);
                    ui.add_space(gap - 4.0);
                    let local = |text: &str, icon: &'static str| {
                        ds::Button::new(label(text, local_labels)).icon(icon).quiet().label(text)
                    };
                    if local("Branch", icons::GIT_BRANCH_PLUS)
                        .enabled(head_sha.is_some())
                        .tooltip("Create a branch here and check it out")
                        .show(ui, pal)
                        .clicked()
                        && let Some(sha) = head_sha.clone()
                    {
                        acts.push(Action::Input(InputKind::Branch { at: sha }));
                    }
                    if local("Stash", icons::ARCHIVE)
                        .enabled(dirty)
                        .tooltip("Stash all changes")
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::Stash));
                    }
                    if local("Pop", icons::ARCHIVE_RESTORE)
                        .enabled(has_stash)
                        .tooltip("Apply and drop the latest stash")
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::StashPop(None)));
                    }
                    ui.min_rect().width().round()
                },
            )
            .inner;
        // While git works, labels are wider; keep centring on the idle width.
        if used != rest_w && !running("Pull") && !running("Push") && !tab.is_fetching() {
            ui.ctx().data_mut(|d| d.insert_temp(rest_id, used));
            ui.ctx().request_repaint();
        }

        // End: settings.
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(full)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                if ds::Button::icon_only(icons::SETTINGS, "Settings")
                    .quiet()
                    .show(ui, pal)
                    .clicked()
                {
                    self.modal = Some(Modal::Settings);
                }
                if let Some(v) = self.newer_release().map(str::to_string)
                    && ds::Button::new(format!("Update: v{v}"))
                        .icon(icons::DOWNLOAD)
                        .quiet()
                        .tooltip("A newer BranchBlitz is available; opens the release page")
                        .show(ui, pal)
                        .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!(
                        "{}/releases/tag/v{v}",
                        crate::REPO_URL
                    )));
                }
                if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i))
                    && ds::Button::icon_only(icons::SEARCH, "Find commits (Ctrl F)")
                        .quiet()
                        .show(ui, pal)
                        .clicked()
                {
                    tab.find.get_or_insert_with(Find::default).focus = true;
                }
            },
        );
        acts
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.spacing_mut().item_spacing.x = 16.0;
        let item = |ui: &mut egui::Ui,
                    icon: &str,
                    text: &str,
                    color: egui::Color32,
                    icon_color: egui::Color32| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(icon).size(13.0).color(icon_color));
                ui.label(RichText::new(text).size(12.0).color(color));
            })
        };
        let tab = self.active.and_then(|i| self.tabs.get(i));
        ui.horizontal_centered(|ui| {
            if let Some(label) = tab.and_then(|t| t.running.front()) {
                item(
                    ui,
                    icons::LOADER,
                    &busy_text(label),
                    pal.text_muted,
                    pal.text_subtle,
                );
            } else if let Some((ok, text)) = &self.status_line {
                let (icon, c) = if *ok {
                    (icons::CIRCLE_CHECK, pal.success)
                } else {
                    (icons::CIRCLE_ALERT, pal.danger)
                };
                item(
                    ui,
                    icon,
                    text,
                    if *ok { pal.text_muted } else { pal.danger },
                    c,
                );
            }
            if let Some(branch) = tab
                .and_then(|t| t.data.as_ref())
                .and_then(|d| d.head_branch.as_deref())
            {
                item(
                    ui,
                    icons::GIT_BRANCH,
                    branch,
                    pal.text_muted,
                    pal.text_subtle,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(concat!("BranchBlitz ", env!("CARGO_PKG_VERSION")))
                        .size(12.0)
                        .color(pal.text_subtle),
                );
                ui.add_space(-12.0);
                let icon = app_icon_texture(ui.ctx());
                ui.add(egui::Image::new((icon.id(), egui::vec2(16.0, 16.0))))
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Image, true, "BranchBlitz")
                    });
                if let Some(tab) = tab {
                    let path = tab.path.display().to_string();
                    let resp = ui
                        .add(
                            egui::Label::new(
                                RichText::new(&path)
                                    .monospace()
                                    .size(11.5)
                                    .color(pal.text_muted),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_text("Click to copy the path");
                    if resp.clicked() {
                        ui.ctx().copy_text(path);
                    }
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let mut open: Option<PathBuf> = None;
        let w = 440.0;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.12);
            let bolt = bolt_texture(ui.ctx());
            ui.add(egui::Image::new((bolt.id(), egui::vec2(47.0, 77.0))));
            ui.add_space(12.0);
            ui.label(
                RichText::new("BranchBlitz")
                    .size(40.0)
                    .variation("wght", 700.0)
                    .extra_letter_spacing(-0.8)
                    .color(pal.text),
            );
            ui.label(RichText::new("See every branch. Merge in two clicks.").color(pal.text_muted));
            ui.add_space(24.0);
            if ds::Button::new("Open repository…")
                .icon(icons::FOLDER_OPEN)
                .primary()
                .size(egui::vec2(240.0, 34.0))
                .show(ui, pal)
                .clicked()
            {
                self.pick_folder();
            }
            ui.add_space(8.0);
            if ds::Button::new("Clone repository…")
                .icon(icons::DOWNLOAD)
                .size(egui::vec2(240.0, 34.0))
                .show(ui, pal)
                .clicked()
            {
                self.open_clone();
            }
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(
                egui::vec2(w, ds::CONTROL_MD),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let open_btn = 72.0;
                    let resp = ui
                        .allocate_ui(egui::vec2(w - open_btn - 8.0, ds::CONTROL_MD), |ui| {
                            ui.add_sized(
                                egui::vec2(w - open_btn - 8.0, ds::CONTROL_MD),
                                egui::TextEdit::singleline(&mut self.open_path)
                                    .hint_text("/path/to/repository")
                                    .align(egui::Align2::LEFT_CENTER),
                            )
                        })
                        .inner;
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ds::Button::new("Open")
                        .size(egui::vec2(open_btn, ds::CONTROL_MD))
                        .show(ui, pal)
                        .clicked()
                        || enter
                    {
                        open = Some(expand_home(self.open_path.trim()));
                    }
                },
            );
            ui.add_space(28.0);
            if !self.settings.recent.is_empty() {
                ui.allocate_ui_with_layout(
                    egui::vec2(w, 0.0),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.label(ds::caption("Recent", pal));
                        ui.add_space(4.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for p in self.settings.recent.clone() {
                            let name = p
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            let resp = ds::row(ui, pal, ds::ROW_H + 4.0, false, &name);
                            let r = resp.rect;
                            let painter = ui.painter();
                            ds::paint_icon(
                                painter,
                                egui::pos2(r.left() + 16.0, r.center().y),
                                icons::FOLDER,
                                14.0,
                                pal.text_subtle,
                            );
                            let ng = ds::galley(painter, &name, 13.0, 550.0, pal.text, 160.0);
                            let nx = r.left() + 32.0;
                            let nw = ng.size().x;
                            painter.galley(
                                egui::pos2(nx, r.center().y - ng.size().y / 2.0),
                                ng,
                                pal.text,
                            );
                            let path = p.display().to_string();
                            let mut job = ds::text_job(&path, 11.5, 400.0, pal.text_muted, true);
                            job.wrap.max_width = (r.right() - 8.0 - (nx + nw + 12.0)).max(10.0);
                            job.wrap.max_rows = 1;
                            job.wrap.break_anywhere = true;
                            job.wrap.overflow_character = Some('…');
                            let pg = painter.layout_job(job);
                            painter.galley(
                                egui::pos2(nx + nw + 12.0, r.center().y - pg.size().y / 2.0),
                                pg,
                                pal.text_muted,
                            );
                            if resp.on_hover_text(path).clicked() {
                                open = Some(p.clone());
                            }
                        }
                    },
                );
            }
        });
        if let Some(p) = open {
            self.open_repo(&p, true);
        }
    }

    fn show_settings(&mut self, ctx: &egui::Context) {
        let agent = match &self.agent_backend {
            crate::agent::Backend::Claude(p) => format!("Claude Code ({})", p.display()),
            crate::agent::Backend::Preview => {
                "None found: install Claude Code (claude on the PATH)".into()
            }
        };
        let diagnostics = self.diagnostics(&agent);
        let newer = self.newer_release().map(str::to_string);
        let log = crate::log::path().map(|p| p.display().to_string());
        let info = settings_view::Info {
            agent: &agent,
            renderer: &self.renderer,
            high_performance_at_start: self.high_performance_at_start,
            diagnostics: &diagnostics,
            log: log.as_deref(),
            newer: newer.as_deref(),
            checked: self.settings.updates_checked,
        };
        let out = settings_view::window(ctx, &mut self.settings, &mut self.settings_page, &info);
        if out.style_changed {
            self.settings.apply_style(ctx);
        }
        if out.fonts_changed {
            fonts::apply(
                ctx,
                self.settings.ui_font.as_deref(),
                self.settings.mono_font.as_deref(),
            );
        }
        if out.colors_changed {
            self.recolor();
        }
        if out.check_updates {
            self.check_for_updates(true);
        }
        if out.open_log
            && let Some(p) = crate::log::path()
        {
            self.open_file(&p);
        }
        if out.close {
            self.modal = None;
            self.limit
                .store(self.settings.commit_limit, Ordering::Relaxed);
            if let Some(i) = self.active {
                self.reload(i);
            }
        }
    }

    fn show_modal(&mut self, ctx: &egui::Context) {
        if matches!(self.modal, Some(Modal::Settings)) {
            self.show_settings(ctx);
            return;
        }
        let Some(modal) = &mut self.modal else { return };
        let mut close = false;
        let mut submit: Option<Op> = None;
        let mut clone: Option<(String, PathBuf)> = None;
        let mut browse_parent = false;
        let mut open_settings: Option<settings_view::Page> = None;
        let target_tab = match modal {
            Modal::Input { tab, .. } => self.tabs.iter().position(|t| t.id == *tab),
            _ => None,
        };
        let data = target_tab
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| t.data.as_ref());
        let pal = self.settings.palette(ctx.theme());
        let frame = egui::Frame::window(&ctx.global_style())
            .inner_margin(egui::Margin::same(24))
            .stroke(egui::Stroke::new(1.0, pal.border_strong));
        // Actions sit at the bottom right: the way forward rightmost, Cancel before it.
        let actions = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                add(ui);
            });
        };
        let resp = egui::Modal::new(egui::Id::unique("bb-modal"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(440.0);
                ui.spacing_mut().item_spacing.y = 12.0;
                match modal {
                    Modal::Input { kind, value, .. } => {
                        let title = match kind {
                            InputKind::Branch { at, .. } => {
                                format!("Create branch at {}", git::short(at))
                            }
                            InputKind::Tag { at } => format!("Create tag at {}", git::short(at)),
                            InputKind::Rename { old } => format!("Rename {old}"),
                        };
                        ui.label(ds::title(title).color(pal.text));
                        let resp = ui.add_sized(
                            egui::vec2(ui.available_width(), ds::CONTROL_MD),
                            egui::TextEdit::singleline(value)
                                .hint_text("Name")
                                .align(egui::Align2::LEFT_CENTER),
                        );
                        resp.request_focus();
                        let problem = new_ref_problem(data, value.trim());
                        let valid = problem.is_none();
                        if let Some(p) = problem.filter(|_| !value.trim().is_empty()) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(icons::CIRCLE_ALERT)
                                        .size(13.0)
                                        .color(pal.danger),
                                );
                                ui.label(RichText::new(p).size(12.0).color(pal.danger));
                            });
                        }
                        actions(ui, &mut |ui| {
                            let ok = ds::Button::new("Create")
                                .primary()
                                .enabled(valid)
                                .show(ui, &pal)
                                .clicked()
                                || (valid && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                            if ok {
                                let name = value.trim().to_string();
                                submit = Some(match kind {
                                    InputKind::Branch { at } => Op::CreateBranch {
                                        name,
                                        at: at.clone(),
                                        checkout: true,
                                    },
                                    InputKind::Tag { at } => Op::CreateTag {
                                        name,
                                        at: at.clone(),
                                    },
                                    InputKind::Rename { old } => Op::RenameBranch {
                                        old: old.clone(),
                                        new: name,
                                    },
                                });
                            }
                            if ds::Button::new("Cancel").quiet().show(ui, &pal).clicked() {
                                close = true;
                            }
                        });
                    }
                    Modal::Notice {
                        title,
                        body,
                        settings,
                    } => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icons::INFO).size(18.0).color(pal.accent));
                            ui.label(ds::title(title.as_str()).color(pal.text));
                        });
                        ui.add(
                            egui::Label::new(RichText::new(body.as_str()).color(pal.text)).wrap(),
                        );
                        actions(ui, &mut |ui| {
                            if let Some(page) = *settings
                                && ds::Button::new("Open settings")
                                    .primary()
                                    .show(ui, &pal)
                                    .clicked()
                            {
                                open_settings = Some(page);
                            }
                            if ds::Button::new("OK").quiet().show(ui, &pal).clicked() {
                                close = true;
                            }
                        });
                    }
                    Modal::Error { title, body } => {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(icons::CIRCLE_ALERT)
                                    .size(18.0)
                                    .color(pal.danger),
                            );
                            ui.label(ds::title(title.as_str()).color(pal.danger));
                        });
                        egui::Frame::new()
                            .fill(pal.canvas)
                            .stroke(egui::Stroke::new(1.0, pal.border))
                            .corner_radius(egui::CornerRadius::same(theme::RADIUS_MD))
                            .inner_margin(egui::Margin::same(12))
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .max_height(320.0)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(body.as_str())
                                                    .monospace()
                                                    .color(pal.text),
                                            )
                                            .wrap(),
                                        );
                                    });
                            });
                        actions(ui, &mut |ui| {
                            if ds::Button::new("Close").show(ui, &pal).clicked() {
                                close = true;
                            }
                            if ds::Button::new("Copy")
                                .icon(icons::COPY)
                                .quiet()
                                .show(ui, &pal)
                                .clicked()
                            {
                                ui.ctx().copy_text(body.clone());
                            }
                        });
                    }
                    Modal::Clone {
                        url,
                        parent,
                        busy,
                        error,
                        focus,
                    } => {
                        ui.label(ds::title("Clone repository").color(pal.text));
                        let field =
                            |ui: &mut egui::Ui, text: &mut String, hint: &str, label: &str| {
                                let r = ui.add_sized(
                                    egui::vec2(ui.available_width(), ds::CONTROL_MD),
                                    egui::TextEdit::singleline(text)
                                        .hint_text(hint)
                                        .align(egui::Align2::LEFT_CENTER),
                                );
                                r.widget_info(|| {
                                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, label)
                                });
                                r
                            };
                        let u = field(
                            ui,
                            url,
                            "https://github.com/owner/repo.git or git@host:owner/repo",
                            "Repository URL",
                        );
                        if std::mem::take(focus) {
                            u.request_focus();
                        }
                        ui.horizontal(|ui| {
                            let browse = 84.0;
                            ui.allocate_ui(
                                egui::vec2(ui.available_width() - browse - 8.0, ds::CONTROL_MD),
                                |ui| field(ui, parent, "Folder to clone into", "Clone into folder"),
                            );
                            if ds::Button::new("Browse…")
                                .size(egui::vec2(browse, ds::CONTROL_MD))
                                .enabled(!*busy)
                                .show(ui, &pal)
                                .clicked()
                            {
                                browse_parent = true;
                            }
                        });
                        let name = git::repo_name_from_url(url);
                        let dest = name.as_ref().map(|n| expand_home(parent.trim()).join(n));
                        let exists = dest
                            .as_ref()
                            .is_some_and(|d| d.read_dir().is_ok_and(|mut e| e.next().is_some()));
                        let (note, problem) = match (&dest, url.trim().is_empty()) {
                            (_, true) => (String::new(), true),
                            (None, false) => ("Not a repository URL".to_string(), true),
                            (Some(d), _) if exists => {
                                (format!("{} already exists", d.display()), true)
                            }
                            (Some(d), _) => (format!("Clones into {}", d.display()), false),
                        };
                        if !note.is_empty() {
                            ui.label(RichText::new(note).size(12.0).color(if problem {
                                pal.danger
                            } else {
                                pal.text_muted
                            }));
                        }
                        if *busy {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(
                                    RichText::new("Cloning… large repositories take a while")
                                        .color(pal.text_muted),
                                );
                            });
                        }
                        if let Some(e) = error.as_ref() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(e.as_str())
                                        .monospace()
                                        .size(12.0)
                                        .color(pal.danger),
                                )
                                .wrap(),
                            );
                        }
                        actions(ui, &mut |ui| {
                            let ok = !problem && !*busy;
                            if (ds::Button::new("Clone")
                                .primary()
                                .enabled(ok)
                                .show(ui, &pal)
                                .clicked()
                                || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter))))
                                && let Some(d) = dest.clone()
                            {
                                *busy = true;
                                *error = None;
                                clone = Some((url.trim().to_string(), d));
                            }
                            if ds::Button::new("Cancel")
                                .quiet()
                                .enabled(!*busy)
                                .show(ui, &pal)
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    }
                    // Drawn by `show_settings`.
                    Modal::Settings => {}
                }
            });
        // A running clone keeps its dialog (clicking outside doesn't hide the result).
        if resp.should_close() && !matches!(self.modal, Some(Modal::Clone { busy: true, .. })) {
            close = true;
        }
        if let Some((url, dest)) = clone {
            self.start_clone(url, dest);
        }
        if let Some(page) = open_settings {
            self.settings_page = page;
            self.modal = Some(Modal::Settings);
            return;
        }
        if browse_parent {
            self.pick_clone_parent();
        }
        if let Some(op) = submit {
            close = true;
            if let Some(i) = target_tab {
                self.submit(i, op, false);
            }
        }
        if close {
            self.modal = None;
        }
    }
}

/// A commit message as (summary, description): first line, then the rest without the blank
/// line between them.
pub(crate) fn split_message(msg: &str) -> (String, String) {
    let msg = msg.trim();
    match msg.split_once('\n') {
        Some((summary, body)) => (summary.trim().to_string(), body.trim().to_string()),
        None => (msg.to_string(), String::new()),
    }
}

/// The branch switcher at the start of the toolbar: a searchable list of local branches.
fn branch_switcher(ui: &mut egui::Ui, pal: &Palette, tab: &RepoTab) -> Option<Action> {
    let data = tab.data.as_ref();
    let value = match (
        data.and_then(|d| d.head_branch.clone()),
        data.and_then(|d| d.head_sha.clone()),
    ) {
        (Some(b), _) => b,
        (None, Some(sha)) => format!("detached at {}", git::short(&sha)),
        _ => "—".into(),
    };
    let max_w = ui.available_width();
    let value_g = ds::galley(
        ui.painter(),
        &value,
        13.0,
        550.0,
        pal.text,
        (max_w - 70.0).min(280.0),
    );
    let w = 6.0 + 28.0 + 8.0 + value_g.size().x.max(40.0) + 8.0 + 14.0 + 8.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w.min(max_w), 38.0), egui::Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, format!("Branch {value}"))
    });
    let p = ui.painter();
    if resp.hovered() || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp))
    {
        p.rect_filled(rect, egui::CornerRadius::same(theme::RADIUS_MD), pal.hover);
    }
    let icon_r = egui::Rect::from_min_size(rect.min + egui::vec2(6.0, 5.0), egui::vec2(28.0, 28.0));
    p.rect_filled(
        icon_r,
        egui::CornerRadius::same(theme::RADIUS_SM),
        pal.accent_soft,
    );
    ds::paint_icon(p, icon_r.center(), icons::GIT_BRANCH, 16.0, pal.accent);
    let tx = icon_r.right() + 8.0;
    let label = p.layout_job(ds::text_job("Branch", 11.0, 400.0, pal.text_subtle, false));
    p.galley(egui::pos2(tx, rect.top() + 3.0), label, pal.text_subtle);
    p.galley(egui::pos2(tx, rect.top() + 18.0), value_g, pal.text);
    ds::paint_icon(
        p,
        egui::pos2(rect.right() - 8.0 - 7.0, rect.center().y),
        icons::CHEVRONS_UP_DOWN,
        14.0,
        pal.text_subtle,
    );
    ds::focus_ring(ui, &resp, pal, theme::RADIUS_MD);
    let mut act = None;
    let filter_id = ui.scope_id().with("branch-switcher-filter");
    egui::Popup::menu(&resp)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.0)
        .show(|ui| {
            ui.set_width(320.0);
            let Some(data) = data else { return };
            let mut filter: String = ui.ctx().data(|d| d.get_temp(filter_id)).unwrap_or_default();
            let field = ds::search_field(ui, pal, &mut filter, "Switch to branch", None);
            field.request_focus();
            ui.ctx()
                .data_mut(|d| d.insert_temp(filter_id, filter.clone()));
            ui.add_space(4.0);
            let f = filter.to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for r in data.refs.iter().filter(|r| {
                        r.kind == git::RefKind::Local
                            && (f.is_empty() || r.name.to_lowercase().contains(&f))
                    }) {
                        let current = data.head_branch.as_deref() == Some(r.name.as_str());
                        let icon = if current {
                            icons::CHECK
                        } else {
                            icons::GIT_BRANCH
                        };
                        if ds::menu_item(ui, pal, icon, &r.name, None, false).clicked() {
                            if !current {
                                act = Some(Action::Op(Op::Checkout(r.name.clone())));
                            }
                            ui.close();
                        }
                    }
                });
        });
    act
}

fn pull_mode_label(mode: git::PullMode) -> &'static str {
    match mode {
        git::PullMode::Merge => "Pull (fast-forward if possible)",
        git::PullMode::FastForwardOnly => "Pull (fast-forward only)",
        git::PullMode::Rebase => "Pull (rebase)",
    }
}

/// The logo bolt (design system `branchblitz-bolt.svg`, rendered to PNG).
fn bolt_texture(ctx: &egui::Context) -> egui::TextureHandle {
    png_texture(ctx, "bolt", include_bytes!("../assets/bolt.png"))
}

/// The small app icon (design system `app-icon-small.svg`), for the status bar.
fn app_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    png_texture(
        ctx,
        "app-icon",
        include_bytes!("../assets/icons/branchblitz-32.png"),
    )
}

/// A bundled PNG as texture, loaded once per context.
fn png_texture(ctx: &egui::Context, name: &str, png: &[u8]) -> egui::TextureHandle {
    let id = egui::Id::unique(("png-texture", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t;
    }
    let img = image::load_from_memory(png)
        .expect("bundled PNG")
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    let t = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// "Fetch" becomes "Fetching…" for the status bar.
fn busy_text(label: &str) -> String {
    match label {
        "Fetch" => "Fetching…".into(),
        "Pull" => "Pulling…".into(),
        "Push" => "Pushing…".into(),
        other => format!("{other}…"),
    }
}

/// Hooks for the MCP server (`mcp.rs`), which drives the app without a window.
impl BranchBlitz {
    #[cfg(test)]
    pub(crate) fn tabs_ref(&self) -> &[RepoTab] {
        &self.tabs
    }

    #[cfg(test)]
    pub(crate) fn tabs_mut(&mut self) -> &mut Vec<RepoTab> {
        &mut self.tabs
    }

    #[cfg(test)]
    pub(crate) fn clear_status(&mut self) {
        self.status_line = None;
    }

    pub(crate) fn set_headless(&mut self) {
        self.headless = true;
    }

    /// What a bug report needs: version, system, rendering, git, agent.
    fn diagnostics(&self, agent: &str) -> String {
        let s = &self.settings;
        format!(
            "BranchBlitz {}\n{}\nRendering: {}{}\nAgent: {agent}; model {}; access {:?}",
            env!("CARGO_PKG_VERSION"),
            self.system,
            self.renderer,
            if s.gpu_high_performance {
                " (high performance chosen)"
            } else {
                ""
            },
            s.agent_model.as_deref().unwrap_or("default"),
            s.agent_access,
        )
    }

    /// Asks GitHub for the newest release, at most once a day (Settings, About). In the
    /// background: the answer comes as a message.
    pub fn check_for_updates(&mut self, force: bool) {
        let day = 24 * 60 * 60;
        let due = chrono::Utc::now().timestamp() - self.settings.updates_checked > day;
        if self.headless || !(force || (self.settings.check_updates && due)) {
            return;
        }
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = git::latest_release(crate::REPO_URL);
            let _ = tx.send(Msg {
                tab: 0,
                kind: MsgKind::Latest(r),
            });
            ctx.request_repaint();
        });
    }

    /// A release newer than this build, if one was seen.
    fn newer_release(&self) -> Option<&str> {
        let latest = self.settings.latest_version.as_deref()?;
        let this = git::version_key(env!("CARGO_PKG_VERSION"))?;
        (git::version_key(latest)? > this).then_some(latest)
    }

    /// Under WSL, once: the Windows display scaling doesn't reach the app, and there is a
    /// Windows build with the Windows title bar.
    pub fn wsl_hint(&mut self) {
        let wsl = std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::fs::read_to_string("/proc/version")
                .is_ok_and(|v| v.to_lowercase().contains("microsoft"));
        if wsl && !self.settings.wsl_hint_shown && self.modal.is_none() {
            self.show_wsl_hint();
        }
    }

    pub(crate) fn show_wsl_hint(&mut self) {
        self.settings.wsl_hint_shown = true;
        self.modal = Some(Modal::Notice {
            title: "Running under WSL".into(),
            body: "WSL doesn't pass the Windows display scaling on to Linux apps, so BranchBlitz \
                   may look small. Set the size in Settings, Appearance, Scale (or use Ctrl + and \
                   Ctrl −).\n\nThere is also a Windows build (the …-windows-x86_64.zip download) \
                   with the Windows title bar and scaling."
                .into(),
            settings: Some(settings_view::Page::Appearance),
        });
    }

    /// After a crash (see `log::init`): says so, and where the details are.
    pub fn report_crash(&mut self) {
        let log = crate::log::path().map_or("the log".into(), |p| p.display().to_string());
        self.modal = Some(Modal::Error {
            title: "BranchBlitz quit unexpectedly".into(),
            body: format!(
                "The last session ended with a crash. The details are in {log}. A bug report \
                 with them helps a lot: Settings, About, Report a bug."
            ),
        });
    }

    /// Answers with the agent CLI that is installed (Claude Code), if any, and lets it use the
    /// app through its own MCP endpoint.
    pub fn use_installed_agent(&mut self) {
        self.agent_backend = crate::agent::Backend::detect();
        if self.agent_backend != crate::agent::Backend::Preview {
            self.live = crate::live::Live::start(self.ctx.clone());
        }
    }

    pub(crate) fn set_confirm_danger(&mut self, allow: bool) {
        self.confirm_danger = allow;
    }

    pub(crate) fn set_theme_choice(&mut self, choice: ThemeChoice) {
        self.settings.theme = choice;
    }

    /// Opens (or focuses) a repository; the error is returned instead of shown in a dialog.
    pub(crate) fn open_for_automation(&mut self, path: &Path) -> Result<(), String> {
        let before = self.modal.is_some();
        self.open_repo(path, true);
        if !before && let Some(Modal::Error { body, .. }) = &self.modal {
            let e = body.clone();
            self.modal = None;
            return Err(e);
        }
        Ok(())
    }

    /// Whether background work is still pending: git jobs, loads, commit details or a diff.
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self.modal, Some(Modal::Clone { busy: true, .. }))
            || self.tabs.iter().any(|t| {
                !t.running.is_empty()
                || t.loads_done < t.loads_requested
                || t.tags_pending
                || (matches!(t.selected, Selection::Commit(_))
                    && t.details.is_none()
                    && t.details_error.is_none())
                || matches!(t.diff, Some((_, None)))
                // An agent answering. (The agent's own tool calls wait with `tab_busy` instead.)
                || t.agent.busy()
            })
    }

    /// One-line summary for tool results.
    pub(crate) fn summary(&self) -> String {
        let tab = self.active.map(|i| &self.tabs[i]);
        let mut parts = vec![match tab {
            Some(t) => format!(
                "repo {} on {}",
                t.name,
                t.data
                    .as_ref()
                    .and_then(|d| d.head_branch.clone())
                    .unwrap_or_else(|| "(detached)".into())
            ),
            None => "no repo open".into(),
        }];
        if let Some(t) = tab
            && !t.running.is_empty()
        {
            parts.push(format!(
                "running: {}",
                Vec::from(t.running.clone()).join(", ")
            ));
        }
        if let Some(a) = tab.and_then(|t| t.alert.as_ref()) {
            parts.push(format!("alert: {}", a.title));
        }
        if let Some(m) = &self.modal {
            parts.push(format!("dialog: {}", modal_title(m)));
        }
        if let Some((ok, s)) = &self.status_line {
            parts.push(format!("status{}: {s}", if *ok { "" } else { " (error)" }));
        }
        parts.join(" | ")
    }

    /// Machine-readable view of what the user would see, for the `state` tool.
    pub(crate) fn snapshot(&self, max_commits: usize) -> serde_json::Value {
        self.snapshot_of(max_commits, self.active)
    }

    /// Like `snapshot`, with the details of tab `detail` (usually the active one).
    fn snapshot_of(&self, max_commits: usize, detail: Option<usize>) -> serde_json::Value {
        use serde_json::json;
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(n, t)| {
                let mut v = json!({
                    "name": t.name,
                    "path": t.path,
                    "active": self.active == Some(n),
                    "running": t.running,
                    "load_error": t.load_error,
                });
                if detail != Some(n) {
                    return v;
                }
                v["selected"] = match &t.selected {
                    Selection::None => json!(null),
                    Selection::Wip => json!("WIP"),
                    Selection::Commit(sha) => json!(sha),
                };
                v["columns"] = json!({
                    "branch": t.ref_col_w,
                    "graph": t.graph_col_w,
                    "author": t.author_col_w,
                    "date": t.date_col_w,
                });
                v["alert"] = json!(
                    t.alert
                        .as_ref()
                        .map(|a| json!({"title": a.title, "body": a.body, "danger": a.danger}))
                );
                v["new_branch"] = json!(
                    t.new_branch
                        .as_ref()
                        .map(|b| json!({"at": b.at, "name": b.name}))
                );
                v["diff_open"] = json!(t.diff.as_ref().map(|(d, _)| format!("{d:?}")));
                let Some(d) = &t.data else { return v };
                let refs = |kind: git::RefKind| -> Vec<serde_json::Value> {
                    d.refs
                        .iter()
                        .filter(|r| r.kind == kind)
                        .map(|r| {
                            let mut e = json!({"name": r.name, "commit": git::short(&r.target)});
                            if r.kind == git::RefKind::Tag {
                                // null = not known yet (no fetch since start).
                                e["on_remote"] = json!(d.tag_on_remote(r));
                            }
                            if let Some(u) = &r.upstream {
                                e["upstream"] = json!(u);
                                e["ahead"] = json!(r.ahead);
                                e["behind"] = json!(r.behind);
                                if r.upstream_gone {
                                    e["upstream_gone"] = json!(true);
                                }
                            }
                            e
                        })
                        .collect()
                };
                let files = |fs: &[git::FileChange]| -> Vec<String> {
                    fs.iter()
                        .map(|f| format!("{} {}", f.status, f.path))
                        .collect()
                };
                v["head_branch"] = json!(d.head_branch);
                v["head"] = json!(d.head_sha.as_deref().map(git::short));
                v["repo_state"] = json!(format!("{:?}", d.state));
                v["local_branches"] = json!(refs(git::RefKind::Local));
                v["remote_branches"] = json!(refs(git::RefKind::Remote));
                v["tags"] = json!(refs(git::RefKind::Tag));
                v["stashes"] = json!(d.stashes.iter().map(|s| &s.message).collect::<Vec<_>>());
                v["staged"] = json!(files(&d.status.staged));
                v["unstaged"] = json!(files(&d.status.unstaged));
                v["conflicted"] = json!(d.status.conflicted);
                v["commits_loaded"] = json!(d.commits.len());
                v["commits"] = json!(
                    d.commits
                        .iter()
                        .take(max_commits)
                        .map(|c| {
                            let labels: Vec<&str> = t
                                .refs_at
                                .get(&c.sha)
                                .map(|ix| ix.iter().map(|&k| d.refs[k].name.as_str()).collect())
                                .unwrap_or_default();
                            json!({
                                "sha": git::short(&c.sha),
                                "subject": c.subject,
                                "author": c.author,
                                "parents": c.parents.len(),
                                "refs": labels,
                            })
                        })
                        .collect::<Vec<_>>()
                );
                v
            })
            .collect();
        json!({
            "tabs": tabs,
            "dialog": self.modal.as_ref().map(|m| {
                let mut v = json!({"title": modal_title(m)});
                match m {
                    Modal::Error { body, .. } => v["body"] = json!(body),
                    Modal::Input { value, .. } => v["value"] = json!(value),
                    Modal::Notice { body, .. } => v["body"] = json!(body),
                    Modal::Clone { error, busy, .. } => {
                        v["busy"] = json!(busy);
                        v["error"] = json!(error);
                    }
                    Modal::Settings => {}
                }
                v
            }),
            "status_line": self.status_line.as_ref().map(|(ok, s)| json!({"ok": ok, "text": s})),
            "theme": format!("{:?}", self.settings.theme),
            "theme_name": self.settings.theme_name,
            "busy": self.is_busy(),
        })
    }
}

fn modal_title(m: &Modal) -> String {
    match m {
        Modal::Error { title, .. } => title.clone(),
        Modal::Input { kind, .. } => match kind {
            InputKind::Branch { .. } => "Create branch".into(),
            InputKind::Tag { .. } => "Create tag".into(),
            InputKind::Rename { old } => format!("Rename {old}"),
        },
        Modal::Settings => "Settings".into(),
        Modal::Clone { .. } => "Clone repository".into(),
        Modal::Notice { title, .. } => title.clone(),
    }
}

impl eframe::App for BranchBlitz {
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        let speed = self.settings.scroll_speed.clamp(0.5, 5.0);
        for e in &mut raw_input.events {
            // Ctrl + wheel zooms; leave that alone.
            if let egui::Event::MouseWheel {
                delta, modifiers, ..
            } = e
                && !modifiers.command
            {
                *delta *= speed;
            }
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        debug_repaints(ctx);
        // To check the crash report: BB_DEBUG_PANIC=1 crashes after the first frame.
        if std::env::var_os("BB_DEBUG_PANIC").is_some() && ctx.cumulative_pass_nr() > 1 {
            panic!("BB_DEBUG_PANIC is set");
        }
        self.handle_messages();
        self.serve_agent_tools();
        // F5 reloads, e.g. after changing the repository outside the app.
        if ctx.input(|i| i.key_pressed(egui::Key::F5))
            && let Some(i) = self.active
        {
            self.reload(i);
        }
        // The system theme follows the desktop: look at its colours every few seconds while it
        // is in use or on offer in Settings.
        if self.settings.theme_name == crate::system_theme::NAME
            || matches!(self.modal, Some(Modal::Settings))
        {
            if crate::system_theme::poll() && self.settings.theme_name == crate::system_theme::NAME
            {
                self.settings.apply_style(ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
        ctx.set_theme(self.settings.preference());

        // Refresh local state when the window regains focus (e.g. after using a terminal).
        let focused = ctx.input(|i| i.focused);
        if focused
            && !self.was_focused
            && let Some(i) = self.active
        {
            self.reload(i);
        }
        self.was_focused = focused;

        if let Some(i) = self.active {
            let mins = self.settings.auto_fetch_minutes;
            if mins > 0 {
                let interval = Duration::from_secs(mins as u64 * 60);
                if self.tabs[i]
                    .last_fetch
                    .is_none_or(|t| t.elapsed() >= interval)
                {
                    self.fetch(i, true);
                }
                ctx.request_repaint_after(Duration::from_secs(30));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let pal = self.settings.palette(ctx.theme());

        egui::Panel::top("tabs")
            .frame(
                egui::Frame::new()
                    .fill(pal.sunken)
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 6,
                        bottom: 0,
                    }),
            )
            .show_separator_line(false)
            .show(ui, |ui| self.tab_bar(ui, &pal));
        let status = |ui: &mut egui::Ui, app: &mut Self| {
            egui::Panel::bottom("status")
                .frame(
                    egui::Frame::new()
                        .fill(pal.app)
                        .inner_margin(egui::Margin::symmetric(12, 0)),
                )
                .exact_size(26.0)
                .show(ui, |ui| app.status_bar(ui, &pal));
        };

        let Some(i) = self.active else {
            status(ui, self);
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(pal.canvas))
                .show(ui, |ui| self.welcome(ui, &pal));
            self.show_modal(&ctx);
            return;
        };

        let mut acts = Vec::new();
        egui::Panel::top("toolbar")
            .frame(
                egui::Frame::new()
                    .fill(pal.app)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .exact_size(52.0)
            .show(ui, |ui| acts.extend(self.toolbar(ui, i, &pal)));
        if let Some(alert) = &self.tabs[i].alert {
            // Keys answer the bar only while nothing else (a dialog, a text field) has them.
            let keys = self.modal.is_none()
                && !egui::Popup::is_any_open(ui.ctx())
                && ui.ctx().memory(|m| m.focused().is_none());
            let answer = egui::Panel::top("alert")
                .frame(
                    egui::Frame::new()
                        .fill(if alert.danger {
                            pal.danger_soft
                        } else {
                            pal.accent_soft
                        })
                        .inner_margin(egui::Margin::symmetric(12, 6)),
                )
                .show(ui, |ui| views::alert_bar(ui, alert, &pal, keys))
                .inner;
            match answer {
                Some(true) if alert.danger && self.headless && !self.confirm_danger => {
                    // An assistant must not talk itself into a force push or remote delete.
                    self.status_line = Some((
                        false,
                        format!(
                            "{}: confirming needs confirm_dangerous=true",
                            alert.op.label()
                        ),
                    ));
                }
                Some(true) => {
                    let op = self.tabs[i].alert.take().map(|a| a.op);
                    if let Some(op) = op {
                        self.submit(i, op, false);
                    }
                }
                Some(false) => self.tabs[i].alert = None,
                None => {}
            }
        }
        if self.tabs[i]
            .data
            .as_ref()
            .is_some_and(|d| d.state != RepoState::Clean)
        {
            egui::Panel::top("banner")
                .frame(
                    egui::Frame::new()
                        .fill(pal.warning_soft)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 6,
                            top: 6,
                            bottom: 6,
                        }),
                )
                .show(ui, |ui| {
                    views::state_banner(ui, &self.tabs[i], &pal, &mut acts)
                });
        }
        status(ui, self);

        let (agent_open, agent_max) = (self.settings.agent_open, self.agent_max);

        let agent_model = self.settings.agent_model.clone();
        let agent_model = agent_model.as_deref();
        let agent = views::AgentChoice {
            name: self.agent_backend.name(),
            model: agent_model,
            access: self.settings.agent_access,
            show_tools: self.settings.agent_show_tools,
        };
        let mut agent_h = self.settings.agent_height;
        let tab = &mut self.tabs[i];
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(280.0)
            .size_range(220.0..=420.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.app)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| views::sidebar(ui, tab, &pal, &mut acts));
        egui::Panel::right("details")
            .resizable(true)
            .default_size(380.0)
            .size_range(320.0..=560.0)
            .frame(egui::Frame::new().fill(pal.app))
            .show(ui, |ui| {
                // Details on top; the agent below (resizable), or only its bar when closed or
                // shown large.
                let full = ui.available_rect_before_wrap();
                let docked = agent_open && !agent_max;
                let bottom = if docked {
                    agent_h.clamp(160.0, (full.height() - 180.0).max(160.0))
                } else {
                    views::AGENT_BAR_H
                };
                let (top, rest) = full.split_top_bottom_at_y(full.bottom() - bottom);
                ui.scope_builder(egui::UiBuilder::new().max_rect(top), |ui| {
                    ui.set_clip_rect(top);
                    views::details(ui, tab, &pal, &mut acts)
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
                    if docked {
                        if let Some(dy) = views::agent_splitter(ui, &pal) {
                            agent_h = (agent_h - dy).max(160.0);
                        }
                        views::agent_panel(ui, &mut tab.agent, agent, &pal, &mut acts, false);
                    } else {
                        views::agent_bar(ui, &pal, &mut acts, tab.agent.busy());
                    }
                });
            });
        self.settings.agent_height = agent_h;
        let tab = &mut self.tabs[i];
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.canvas))
            .show(ui, |ui| {
                if let Some(e) = &tab.load_error {
                    ui.colored_label(pal.danger, e);
                }
                if tab.diff.is_some() {
                    views::diff_view(ui, tab, &pal, &mut acts);
                } else {
                    views::graph_view(ui, tab, &pal, &mut acts);
                }
            });

        // The agent at the size of the Settings window.
        if agent_max {
            let tab = &mut self.tabs[i];
            let size = settings_view::window_size(&ctx);
            let frame = egui::Frame::window(&ctx.global_style())
                .inner_margin(0)
                .stroke(egui::Stroke::new(1.0, pal.border_strong));
            let resp = egui::Modal::new(egui::Id::unique("bb-agent"))
                .frame(frame)
                .show(&ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        views::agent_panel(ui, &mut tab.agent, agent, &pal, &mut acts, true)
                    });
                });
            if resp.should_close() {
                acts.push(Action::AgentMaximize(false));
            }
        }

        self.apply(i, acts);
        self.show_modal(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // The GPU choice for the next start, where main() can read it before the window.
        if !self.headless
            && let Some(f) = crate::gpu_choice_file()
        {
            if self.settings.gpu_high_performance {
                let _ = f.parent().map(std::fs::create_dir_all);
                let _ = std::fs::write(&f, "");
            } else {
                let _ = std::fs::remove_file(&f);
            }
        }
        for t in &self.tabs {
            self.settings.columns.insert(
                t.path.display().to_string(),
                ColumnWidths {
                    refs: t.ref_col_w,
                    graph: t.graph_col_w,
                    author: t.author_col_w,
                    date: t.date_col_w,
                },
            );
        }
        self.settings.open_repos = self.tabs.iter().map(|t| t.path.clone()).collect();
        self.settings.active_repo = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.path.clone());
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}

/// With `BB_DEBUG_REPAINT` set: every 2 s, how many frames were drawn and why (stderr).
fn debug_repaints(ctx: &egui::Context) {
    use std::sync::Mutex;
    static STATE: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    if std::env::var_os("BB_DEBUG_REPAINT").is_none() {
        return;
    }
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let (since, frames) = st.get_or_insert((Instant::now(), 0));
    *frames += 1;
    if since.elapsed() >= Duration::from_secs(2) {
        eprintln!("{frames} frames in 2 s; causes: {:?}", ctx.repaint_causes());
        *st = Some((Instant::now(), 0));
    }
}

// ---------------------------------------------------------------- agent tools

/// System, desktop session and git, for the diagnostics.
fn system_info() -> String {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME="))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| std::env::consts::OS.to_string());
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let desktop = [var("XDG_CURRENT_DESKTOP"), var("XDG_SESSION_TYPE")]
        .into_iter()
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "System: {os} ({})\nDesktop: {}\nGit: {}",
        std::env::consts::ARCH,
        if desktop.is_empty() { "-" } else { &desktop },
        git::version().unwrap_or_else(|e| format!("not found ({e})")),
    )
}

/// The tab is doing something the agent should wait for (its own answer excluded).
fn tab_busy(t: &RepoTab) -> bool {
    !t.running.is_empty()
        || t.loads_done < t.loads_requested
        || t.tags_pending
        || matches!(t.diff, Some((_, None)))
}

impl BranchBlitz {
    /// Handles the agent's tool calls (see `live`) and answers those whose result is in.
    fn serve_agent_tools(&mut self) {
        let requests: Vec<crate::live::Request> = match &self.live {
            Some(l) => l.requests.try_iter().collect(),
            None => return,
        };
        for r in requests {
            let Some(i) = self.tab_index(r.tab) else {
                let _ = r
                    .reply
                    .send(Err("The repository was closed in BranchBlitz".into()));
                continue;
            };
            let id = self.tabs[i].id;
            match self.agent_call(i, &r.tool, &r.args) {
                Ok(Answer::Now(text)) => {
                    let _ = r.reply.send(Ok(text));
                }
                Ok(answer) => self.live_pending.push(Pending {
                    tab: id,
                    reply: r.reply,
                    diff: matches!(answer, Answer::WhenDiff),
                    note: match answer {
                        Answer::WhenIdleSaying(t) => Some(t),
                        _ => None,
                    },
                    deadline: Instant::now() + Duration::from_secs(300),
                }),
                Err(e) => {
                    let _ = r.reply.send(Err(e));
                }
            }
        }
        for p in std::mem::take(&mut self.live_pending) {
            let Some(i) = self.tab_index(p.tab) else {
                let _ = p
                    .reply
                    .send(Err("The repository was closed in BranchBlitz".into()));
                continue;
            };
            let t = &self.tabs[i];
            let ready = if p.diff {
                !matches!(t.diff, Some((_, None)))
            } else {
                !tab_busy(t)
            };
            if ready {
                let text = if p.diff {
                    self.diff_text(i)
                } else {
                    match &p.note {
                        Some(n) => format!("{n}\n{}", self.outcome(i)),
                        None => self.outcome(i),
                    }
                };
                let _ = p.reply.send(Ok(text));
            } else if Instant::now() > p.deadline {
                let _ = p
                    .reply
                    .send(Err("Still running; check `state` later".into()));
            } else {
                self.live_pending.push(p);
            }
        }
        if !self.live_pending.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// What the user sees after an action: the status line, a question or error shown, and
    /// where the tab is now.
    fn outcome(&self, i: usize) -> String {
        let t = &self.tabs[i];
        let mut parts = Vec::new();
        if let Some((ok, s)) = &self.status_line {
            parts.push(if *ok {
                s.clone()
            } else {
                format!("Failed: {s}")
            });
        }
        if let Some(Modal::Error { title, body }) = &self.modal {
            parts.push(format!("{title}:\n{body}"));
        }
        if let Some(a) = &t.alert {
            parts.push(format!(
                "Waiting for the user to confirm in the red bar: {}",
                a.title
            ));
        }
        if let Some(d) = &t.data {
            let branch = d.head_branch.as_deref().unwrap_or("(detached HEAD)");
            let mut now = format!(
                "Now on {branch}; {} staged, {} unstaged, {} conflicted",
                d.status.staged.len(),
                d.status.unstaged.len(),
                d.status.conflicted.len()
            );
            if d.state != RepoState::Clean {
                now.push_str(&format!("; {:?} in progress", d.state));
            }
            parts.push(now);
        }
        parts.join("\n")
    }

    /// The diff shown in tab `i`, for the agent (long diffs cut).
    fn diff_text(&self, i: usize) -> String {
        match &self.tabs[i].diff {
            Some((t, Some(Ok(text)))) => {
                let mut s: String = text.chars().take(40_000).collect();
                if s.len() < text.len() {
                    s.push_str("\n[diff cut off]");
                }
                if s.trim().is_empty() {
                    format!("No textual changes in {}", t.path())
                } else {
                    s
                }
            }
            Some((_, Some(Err(e)))) => format!("Failed: {e}"),
            _ => "No diff shown".into(),
        }
    }

    /// One agent tool call on tab `i`, done the way the UI does it.
    fn agent_call(
        &mut self,
        i: usize,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Answer, String> {
        use serde_json::json;
        let arg = |k: &str| args[k].as_str().map(str::to_string);
        let need = |k: &str| arg(k).ok_or_else(|| format!("`{k}` is required"));
        let data = self.tabs[i]
            .data
            .clone()
            .ok_or("The repository is still loading; try again")?;
        let mine_is_ours = data.state != RepoState::Rebasing;
        let op = |me: &mut Self, op: Op| {
            me.status_line = None;
            me.apply(i, vec![Action::Op(op)]);
            Ok(Answer::WhenIdle)
        };
        // Asks the user in the red bar under the toolbar; runs only on their click. Never
        // replaces a question the user still has open.
        // How much the agent may do without asking (the setting in the panel's header).
        let access = self.settings.agent_access;
        let run = op;
        let ask = |me: &mut Self, title: String, body: String, op: Op| {
            if access == AgentAccess::Full {
                return run(me, op);
            }
            if me.tabs[i].alert.is_some() {
                return Err(
                    "The user has a question open in the bar; wait until they answer".into(),
                );
            }
            // Always red: a red bar needs a click, so a stray Enter can't confirm something
            // the agent asked for.
            me.apply(
                i,
                vec![Action::Confirm {
                    title: title.clone(),
                    body,
                    op,
                    danger: true,
                }],
            );
            Ok(Answer::Now(format!(
                "Asked the user in the bar: \"{title}\". It runs only if they confirm; check \
                 `state` later."
            )))
        };
        // Publishes or rewrites, but nothing is lost for good: asks only when supervised.
        let check = |me: &mut Self, title: String, body: String, op: Op| {
            if access == AgentAccess::Supervised {
                ask(me, title, body, op)
            } else {
                run(me, op)
            }
        };
        // A loaded commit, by (a prefix of) its hash: the agent can't slip in rev expressions.
        let commit = |sha: &str| {
            data.commits
                .iter()
                .find(|c| !sha.is_empty() && c.sha.starts_with(sha))
                .ok_or_else(|| format!("No loaded commit {sha}"))
        };
        match tool {
            "state" => {
                let n = args["commits"].as_u64().unwrap_or(30) as usize;
                let v = self.snapshot_of(n, Some(i))["tabs"][i].clone();
                Ok(Answer::Now(
                    serde_json::to_string_pretty(&v).unwrap_or_default(),
                ))
            }
            "select_commit" => {
                let sha = need("sha")?;
                let c = data
                    .commits
                    .iter()
                    .find(|c| c.sha.starts_with(&sha))
                    .ok_or_else(|| format!("No loaded commit {sha}"))?;
                let text = format!("Selected {} {}", git::short(&c.sha), c.subject);
                self.apply(i, vec![Action::Jump(c.sha.clone())]);
                Ok(Answer::Now(text))
            }
            "show_diff" => {
                let path = need("path")?;
                let target = if let Some(sha) = arg("commit") {
                    DiffTarget::Commit { sha, path }
                } else if args["staged"].as_bool() == Some(true) {
                    DiffTarget::Staged { path }
                } else if data.status.conflicted.contains(&path) {
                    DiffTarget::Conflict { path }
                } else {
                    let untracked = data
                        .status
                        .unstaged
                        .iter()
                        .any(|f| f.path == path && f.status == '?');
                    DiffTarget::Unstaged { path, untracked }
                };
                self.apply(i, vec![Action::ShowDiff(target)]);
                Ok(Answer::WhenDiff)
            }
            "checkout" => {
                let name = need("name")?;
                let r = data
                    .refs
                    .iter()
                    .find(|r| r.name == name)
                    .ok_or_else(|| format!("No branch or tag {name}"))?;
                let action = crate::views::checkout_action(r, &data);
                let worktree = match &action {
                    Action::SwitchWorktree(p) => Some(p.display().to_string()),
                    _ => None,
                };
                self.status_line = None;
                self.apply(i, vec![action]);
                // Said the way the user sees it: the same window now shows that worktree.
                // Answered once that worktree is loaded, so the next call finds it.
                if let Some(path) = worktree {
                    return Ok(Answer::WhenIdleSaying(format!(
                        "{name} is checked out in the worktree {path} (git allows a branch in one \
                         worktree only), so BranchBlitz now shows that worktree, in the same window. \
                         You keep working there."
                    )));
                }
                Ok(Answer::WhenIdle)
            }
            "merge" => {
                let mode = match arg("mode").as_deref() {
                    Some("no_ff") => git::MergeMode::NoFastForward,
                    Some("ff_only") => git::MergeMode::FastForwardOnly,
                    Some("squash") => git::MergeMode::Squash,
                    _ => git::MergeMode::Default,
                };
                op(
                    self,
                    Op::Merge {
                        what: need("name")?,
                        mode,
                    },
                )
            }
            "rebase" => op(self, Op::Rebase(need("onto")?)),
            "branch" => {
                let name = need("name")?;
                match need("action")?.as_str() {
                    "create" => op(
                        self,
                        Op::CreateBranch {
                            name,
                            at: arg("at").unwrap_or_else(|| "HEAD".into()),
                            checkout: args["checkout"].as_bool().unwrap_or(true),
                        },
                    ),
                    "delete" => {
                        if data.head_branch.as_deref() == Some(name.as_str()) {
                            return Err(format!("{name} is checked out; check out another first"));
                        }
                        if data.worktree_of(&name).is_some() {
                            return Err(format!("{name} is checked out in another worktree"));
                        }
                        op(self, Op::DeleteBranch { name, force: false })
                    }
                    "rename" => op(
                        self,
                        Op::RenameBranch {
                            old: name,
                            new: need("new_name")?,
                        },
                    ),
                    "fast_forward" => {
                        let up = data
                            .local(&name)
                            .and_then(|r| r.upstream.clone())
                            .ok_or_else(|| format!("{name} has no upstream"))?;
                        op(
                            self,
                            Op::FastForward {
                                local: name,
                                to: up,
                            },
                        )
                    }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "sync" => match need("action")?.as_str() {
                "fetch" => {
                    self.status_line = None;
                    self.fetch(i, false);
                    Ok(Answer::WhenIdle)
                }
                "pull" => {
                    let mode = match arg("pull_mode").as_deref() {
                        Some("ff_only") => git::PullMode::FastForwardOnly,
                        Some("rebase") => git::PullMode::Rebase,
                        Some("merge") | None => self.settings.pull_mode(&self.tabs[i].path),
                        Some(m) => return Err(format!("unknown pull_mode {m}")),
                    };
                    op(self, Op::Pull(mode))
                }
                // Publishing is the user's call.
                "push" => {
                    let b = data.head_branch.clone().unwrap_or_default();
                    check(
                        self,
                        format!("Push {b}?"),
                        format!("The agent wants to push {b} to the remote."),
                        Op::Push { force: false },
                    )
                }
                "force_push" => {
                    let b = data.head_branch.clone().unwrap_or_default();
                    ask(
                        self,
                        format!("Force push {b}?"),
                        format!(
                            "This overwrites {b} on the remote with your local branch (--force-with-lease)."
                        ),
                        Op::Push { force: true },
                    )
                }
                a => Err(format!("unknown action {a}")),
            },
            "changes" => {
                let paths: Vec<String> = args["paths"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| p.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                match need("action")?.as_str() {
                    "stage" if !paths.is_empty() => op(self, Op::Stage(paths)),
                    "unstage" if !paths.is_empty() => op(self, Op::Unstage(paths)),
                    "stage" | "unstage" => Err("`paths` is required".into()),
                    "stage_all" => op(self, Op::StageAll),
                    "unstage_all" => op(self, Op::UnstageAll),
                    "discard" => {
                        let [path] = paths.as_slice() else {
                            return Err("discard one path at a time".into());
                        };
                        if !data.status.unstaged.iter().any(|f| &f.path == path) {
                            return Err(format!("{path} has no unstaged changes"));
                        }
                        let untracked = data
                            .status
                            .unstaged
                            .iter()
                            .any(|f| &f.path == path && f.status == '?');
                        ask(
                            self,
                            format!("Discard changes to {path}?"),
                            "Your changes to this file are lost.".into(),
                            Op::Discard {
                                path: path.clone(),
                                untracked,
                            },
                        )
                    }
                    "commit" => {
                        let message = need("message")?;
                        if message.trim().is_empty() {
                            return Err("the message is empty".into());
                        }
                        if args["amend"].as_bool() == Some(true) {
                            // Rewrites the last commit, which may be pushed already.
                            return check(
                                self,
                                "Amend the last commit?".into(),
                                format!(
                                    "The agent wants to replace the last commit's message with: {}",
                                    message.lines().next().unwrap_or("")
                                ),
                                Op::Commit {
                                    message,
                                    amend: true,
                                },
                            );
                        }
                        op(
                            self,
                            Op::Commit {
                                message,
                                amend: false,
                            },
                        )
                    }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "commit_action" => {
                let c = commit(&need("sha")?)?;
                let (sha, what) = (
                    c.sha.clone(),
                    format!("{} {}", git::short(&c.sha), c.subject),
                );
                let reset = |mode| Op::Reset {
                    to: sha.clone(),
                    mode,
                };
                let branch = data.head_branch.clone().unwrap_or_else(|| "HEAD".into());
                match need("action")?.as_str() {
                    "cherry_pick" => op(self, Op::CherryPick(sha.clone())),
                    "revert" => op(self, Op::Revert(sha.clone())),
                    // Resets rewrite the branch: always the user's call.
                    "reset_soft" | "reset_mixed" => check(
                        self,
                        format!("Reset {branch} to {what}?"),
                        "The commits after it leave the branch; their changes stay in the working copy.".into(),
                        reset(if args["action"] == "reset_soft" {
                            git::ResetMode::Soft
                        } else {
                            git::ResetMode::Mixed
                        }),
                    ),
                    "reset_hard" => ask(
                        self,
                        format!("Hard reset {branch} to {what}?"),
                        "Uncommitted changes and the commits after it on this branch are lost.".into(),
                        reset(git::ResetMode::Hard),
                    ),
                    "tag" => op(
                        self,
                        Op::CreateTag {
                            name: need("tag")?,
                            at: sha.clone(),
                        },
                    ),
                    a => Err(format!("unknown action {a}")),
                }
            }
            "stash" => match need("action")?.as_str() {
                "save" => op(self, Op::Stash),
                "pop" => op(self, Op::StashPop(arg("name"))),
                "apply" => op(
                    self,
                    Op::StashApply(arg("name").unwrap_or_else(|| "stash@{0}".into())),
                ),
                "drop" => {
                    let name = arg("name").unwrap_or_else(|| "stash@{0}".into());
                    ask(
                        self,
                        format!("Drop {name}?"),
                        "The stash is deleted.".into(),
                        Op::StashDrop(name),
                    )
                }
                a => Err(format!("unknown action {a}")),
            },
            "read_conflict" => {
                let path = need("path")?;
                if !data.status.conflicted.contains(&path) {
                    return Err(format!("{path} is not conflicted"));
                }
                let text = git::diff(
                    &self.tabs[i].path,
                    &DiffTarget::Conflict { path: path.clone() },
                )?;
                self.apply(
                    i,
                    vec![Action::ShowDiff(DiffTarget::Conflict {
                        path: path.clone(),
                    })],
                );
                let Some(parts) = crate::conflict::parse(&text) else {
                    return Ok(Answer::Now(format!(
                        "{path} has no conflict markers left; resolve it with side or mark it \
                         resolved by staging (changes stage)."
                    )));
                };
                let blocks: Vec<_> = parts
                    .iter()
                    .filter_map(|p| match p {
                        crate::conflict::Part::Block(b) => Some(if mine_is_ours {
                            json!({"mine": b.ours, "incoming": b.theirs})
                        } else {
                            json!({"mine": b.theirs, "incoming": b.ours})
                        }),
                        _ => None,
                    })
                    .collect();
                Ok(Answer::Now(
                    serde_json::to_string_pretty(&json!({"path": path, "blocks": blocks}))
                        .unwrap_or_default(),
                ))
            }
            "resolve_conflict" => {
                let path = need("path")?;
                if !data.status.conflicted.contains(&path) {
                    return Err(format!("{path} is not conflicted"));
                }
                if let Some(side) = arg("side") {
                    let mine = match side.as_str() {
                        "mine" => true,
                        "incoming" => false,
                        s => return Err(format!("unknown side {s}")),
                    };
                    // Overwrites the file, edits included.
                    return check(
                        self,
                        format!("Take the {side} version of {path}?"),
                        "The whole file becomes that version; edits in it are lost.".into(),
                        Op::TakeSide {
                            path,
                            ours: mine == mine_is_ours,
                        },
                    );
                }
                use crate::conflict::Choice;
                let choices: Vec<Option<Choice>> = args["choices"]
                    .as_array()
                    .ok_or("give `choices` or `side`")?
                    .iter()
                    .map(|c| match (c.as_str(), mine_is_ours) {
                        (Some("mine"), true) | (Some("incoming"), false) => Ok(Some(Choice::Ours)),
                        (Some("mine"), false) | (Some("incoming"), true) => {
                            Ok(Some(Choice::Theirs))
                        }
                        (Some("both"), true) => Ok(Some(Choice::OursThenTheirs)),
                        (Some("both"), false) => Ok(Some(Choice::TheirsThenOurs)),
                        _ => Err(format!("unknown choice {c}")),
                    })
                    .collect::<Result<_, _>>()?;
                let text = git::diff(
                    &self.tabs[i].path,
                    &DiffTarget::Conflict { path: path.clone() },
                )?;
                let parts =
                    crate::conflict::parse(&text).ok_or("no conflict markers in the file")?;
                let n = crate::conflict::blocks(&parts);
                if choices.len() != n {
                    return Err(format!(
                        "{path} has {n} conflict blocks; give one choice for each"
                    ));
                }
                let content =
                    crate::conflict::resolve(&parts, &choices).ok_or("missing a choice")?;
                op(
                    self,
                    Op::ResolveFile {
                        path,
                        content,
                        expected: text,
                    },
                )
            }
            "operation" => {
                let (cont, abort) = match data.state {
                    RepoState::Merging => (Op::ContinueMerge, Op::AbortMerge),
                    RepoState::Rebasing => (Op::ContinueRebase, Op::AbortRebase),
                    RepoState::Clean => {
                        return Err("No merge, rebase or cherry-pick in progress".into());
                    }
                    _ => (Op::ContinueMerge, Op::AbortCherryPick),
                };
                match need("action")?.as_str() {
                    "continue" => op(self, cont),
                    // Throws away the resolutions so far.
                    "abort" => ask(
                        self,
                        format!(
                            "Abort the {}?",
                            match data.state {
                                RepoState::Rebasing => "rebase",
                                RepoState::CherryPicking => "cherry-pick",
                                RepoState::Reverting => "revert",
                                _ => "merge",
                            }
                        ),
                        "Resolved conflicts and other progress are lost.".into(),
                        abort,
                    ),
                    a => Err(format!("unknown action {a}")),
                }
            }
            _ => Err(format!("unknown tool {tool}")),
        }
    }
}

fn spawn_worker(
    id: u64,
    path: PathBuf,
    tx: Sender<Msg>,
    ctx: egui::Context,
    limit: Arc<AtomicUsize>,
) -> Sender<Job> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let mut last: Option<RepoData> = None;
        // Highest load request seen; every load sent afterwards includes it.
        let mut generation = 0;
        // Remote tags are only known after a fetch; every load carries the last answer.
        let mut remote_tags: Option<HashMap<String, String>> = None;
        while let Ok(first) = job_rx.recv() {
            let mut jobs = vec![first];
            while let Ok(j) = job_rx.try_recv() {
                jobs.push(j);
            }
            for j in &jobs {
                if let Job::Load(g) = j {
                    generation = generation.max(*g);
                }
            }
            let send_load = |last: &mut Option<RepoData>, remote_tags: &Option<_>| {
                let loaded = git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                    d.remote_tags = Clone::clone(remote_tags);
                    d
                });
                *last = loaded.as_ref().ok().cloned();
                let _ = tx.send(Msg {
                    tab: id,
                    kind: MsgKind::Loaded(generation, loaded),
                });
                ctx.request_repaint();
            };
            let mut need_load = false;
            for job in jobs {
                match job {
                    Job::Load(_) => need_load = true,
                    Job::Op { op, quiet } => {
                        // Show the local state first; the operation (e.g. fetch) may be slow.
                        if need_load {
                            send_load(&mut last, &remote_tags);
                        }
                        let result = op.run(&path, last.as_ref());
                        if result.is_ok()
                            && let Op::PushTag { name, .. } = &op
                            && let Some(t) = &mut remote_tags
                            && let Some(r) = last.as_ref().and_then(|d| {
                                d.refs
                                    .iter()
                                    .find(|r| r.kind == git::RefKind::Tag && &r.name == name)
                            })
                        {
                            t.insert(name.clone(), r.target.clone());
                        }
                        let check_tags = result.is_ok() && matches!(op, Op::Fetch | Op::Pull(_));
                        // Reload before reporting so the graph is current when the spinner stops.
                        let loaded =
                            git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                                d.remote_tags = remote_tags.clone();
                                d
                            });
                        last = loaded.as_ref().ok().cloned();
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::Loaded(generation, loaded),
                        });
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::OpDone { op, quiet, result },
                        });
                        ctx.request_repaint();
                        need_load = false;
                        // Which tags the remotes have: asked after the fetch is reported, so a slow
                        // remote doesn't keep the fetch busy. Unreachable means unknown, not "unpushed".
                        if check_tags {
                            let remotes =
                                last.as_ref().map(|d| d.remotes.clone()).unwrap_or_default();
                            remote_tags = git::remote_tags(&path, &remotes).ok();
                            let loaded = match last.clone() {
                                Some(mut d) => {
                                    d.remote_tags = remote_tags.clone();
                                    Ok(d)
                                }
                                None => git::load(&path, limit.load(Ordering::Relaxed)),
                            };
                            last = loaded.as_ref().ok().cloned();
                            let _ = tx.send(Msg {
                                tab: id,
                                kind: MsgKind::Loaded(generation, loaded),
                            });
                            ctx.request_repaint();
                        }
                    }
                }
            }
            if need_load {
                let loaded = git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                    d.remote_tags = remote_tags.clone();
                    d
                });
                last = loaded.as_ref().ok().cloned();
                let _ = tx.send(Msg {
                    tab: id,
                    kind: MsgKind::Loaded(generation, loaded),
                });
                ctx.request_repaint();
            }
        }
    });
    job_tx
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

#[cfg(test)]
mod render_tests {
    //! Headless renders of the UI against the demo repo (`BB_DEMO=/path/to/repo`).
    //! Run with `BB_DEMO=... cargo test render -- --ignored --nocapture`.
    use super::*;
    use egui_kittest::Harness;

    fn harness(theme: egui::Theme) -> Option<Harness<'static, BranchBlitz>> {
        let repo = PathBuf::from(std::env::var("BB_DEMO").ok()?);
        let mut h = Harness::builder()
            .with_size(egui::vec2(1500.0, 820.0))
            .with_theme(theme)
            .build_eframe(move |cc| {
                let mut app = BranchBlitz::new(cc, Some(repo));
                app.settings.theme = match theme {
                    egui::Theme::Dark => ThemeChoice::Dark,
                    egui::Theme::Light => ThemeChoice::Light,
                };
                app.settings.fetch_on_switch = false;
                app
            });
        for _ in 0..400 {
            h.step();
            let tab = &h.state().tabs[0];
            if tab.data.is_some() && tab.running.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.run_steps(4);
        Some(h)
    }

    fn save(h: &mut Harness<'_, BranchBlitz>, name: &str) {
        let img = h.render().expect("render");
        let path = format!("/tmp/bb-{name}.png");
        img.save(&path).unwrap();
        println!("saved {path}");
    }

    #[test]
    #[ignore]
    fn render_screens() {
        for (theme, name) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
            let Some(mut h) = harness(theme) else { return };
            let sha = h.state().tabs[0].data.as_ref().unwrap().commits[1]
                .sha
                .clone();
            h.state_mut()
                .apply(0, vec![Action::Select(Selection::Commit(sha))]);
            for _ in 0..50 {
                h.step();
                if h.state().tabs[0].details.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            h.run_steps(3);
            save(&mut h, name);
            h.state_mut().modal = Some(Modal::Settings);
            h.run_steps(3);
            save(&mut h, &format!("settings-{name}"));
        }
    }

    /// Right-click the remote `develop` branch and merge it into the current branch.
    #[test]
    #[ignore]
    fn render_merge_via_context_menu() {
        use egui_kittest::kittest::Queryable;
        let Some(mut h) = harness(egui::Theme::Dark) else {
            return;
        };
        let current = h.state().tabs[0]
            .data
            .as_ref()
            .unwrap()
            .head_branch
            .clone()
            .unwrap();
        let remote_develop = h
            .get_all_by_label("develop")
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .expect("remote develop leaf");
        remote_develop.click_secondary();
        h.run_steps(3);
        save(&mut h, "menu");
        h.get_by_label(&format!("Merge origin/develop into {current}"))
            .click();
        h.run_steps(2);
        for _ in 0..300 {
            h.step();
            if h.state().tabs[0].running.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.run_steps(3);
        save(&mut h, "merged");
        h.state_mut().apply(
            0,
            vec![
                Action::Select(Selection::Wip),
                Action::ShowDiff(DiffTarget::Unstaged {
                    path: "a.txt".into(),
                    untracked: false,
                }),
            ],
        );
        for _ in 0..50 {
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        save(&mut h, "wip-diff");
        let data = h.state().tabs[0].data.as_ref().unwrap();
        let head = data.head_sha.clone().unwrap();
        let commit = data.commits.iter().find(|c| c.sha == head).unwrap();
        assert_eq!(
            commit.parents.len(),
            2,
            "HEAD should be a merge commit: {}",
            commit.subject
        );
        println!("merge commit: {}", commit.subject);
    }
}

#[cfg(test)]
mod glyph_check {
    /// Every non-ASCII character in the UI code must exist in the bundled fonts,
    /// otherwise it renders as a box.
    #[test]
    fn all_ui_glyphs_exist() {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(crate::fonts::definitions(None, None));
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let sources = [
            include_str!("app.rs"),
            include_str!("views.rs"),
            include_str!("settings_view.rs"),
            include_str!("ds.rs"),
        ];
        let mut missing = String::new();
        ctx.fonts_mut(|f| {
            for c in sources
                .iter()
                .flat_map(|s| s.chars())
                .filter(|c| !c.is_ascii())
                .chain(crate::icons::ALL.iter().flat_map(|s| s.chars()))
            {
                if !f.has_glyph(&eframe::egui::FontId::proportional(13.0), c)
                    && !missing.contains(c)
                {
                    missing.push(c);
                }
            }
        });
        assert!(missing.is_empty(), "missing glyphs: {missing}");
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    /// Settings saved by an older version (fewer fields) still load, with defaults for the rest.
    #[test]
    fn old_settings_still_load() {
        let old = r#"{"theme":"Dark","open_repos":["/tmp/x"],"commit_limit":500}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.commit_limit, 500);
        assert_eq!(s.theme_name, theme::DEFAULT_THEME);
        assert_eq!(s.color_rules, colors::default_rules());
        assert_eq!(s.look(), Look::default());
    }

    #[test]
    fn wheel_scrolling_is_scaled_but_ctrl_zoom_is_not() {
        use eframe::App;
        let mut h = egui_kittest::Harness::builder().build_eframe(|cc| BranchBlitz::new(cc, None));
        let ctx = h.ctx.clone();
        let wheel = |command: bool| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers {
                command,
                ..Default::default()
            },
        };
        let mut raw = egui::RawInput {
            events: vec![wheel(false), wheel(true)],
            ..Default::default()
        };
        h.state_mut().settings.scroll_speed = 2.5;
        h.state_mut().raw_input_hook(&ctx, &mut raw);
        let dy: Vec<f32> = raw
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                _ => None,
            })
            .collect();
        assert_eq!(dy, vec![-2.5, -1.0]);
    }
}

#[cfg(test)]
mod name_tests {
    use super::ref_name_problem;

    #[test]
    fn validates_ref_names() {
        for ok in ["feature/PROJ-123", "fix_1", "release/3.1.5"] {
            assert_eq!(ref_name_problem(ok), None, "{ok}");
        }
        for bad in [
            "-f",
            "a b",
            "a..b",
            "x.lock",
            "x/",
            "/x",
            "a~1",
            "a:b",
            "a@{1}",
            "x/.hidden",
            "ends.",
        ] {
            assert!(ref_name_problem(bad).is_some(), "{bad}");
        }
    }
}
