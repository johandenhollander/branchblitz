//! Startup notices (crash, WSL), the update check and diagnostics for bug reports.

use super::*;

impl BranchBlitz {
    /// What a bug report needs: version, system, rendering, git, agent.
    pub(super) fn diagnostics(&self, agent: &str) -> String {
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
    pub(super) fn newer_release(&self) -> Option<&str> {
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
}

/// System, desktop session and git, for the diagnostics.
pub(super) fn system_info() -> String {
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
