//! Settings: stored between runs (eframe storage), edited in the Settings window.

use super::*;

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
    /// Where the commit list shows the author.
    pub author_place: AuthorPlace,
    /// Where the Open and Clone dialogs start; `None`: ~/Projects if it exists, else ~.
    pub home_dir: Option<PathBuf>,
    /// Reopen the tabs in the order they were dragged into; off: sorted by name.
    pub remember_tab_order: bool,
}

/// The author in a column with avatar and name, or as initials on the commit dots.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorPlace {
    #[default]
    Column,
    Graph,
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
    pub(super) fn upgrade_sizes(&mut self) {
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
            author_place: AuthorPlace::Column,
            home_dir: None,
            remember_tab_order: true,
        }
    }
}
