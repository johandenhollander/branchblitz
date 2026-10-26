//! The frame loop: input, messages, panels, the actions they collected; saving settings.

use super::*;

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
        let on_node = self.settings.author_place == AuthorPlace::Graph;
        let opts = views::GraphOpts {
            author_column: !on_node,
            author_on_node: on_node,
        };
        let tab = &mut self.tabs[i];
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.canvas))
            .show(ui, |ui| {
                if let Some(e) = &tab.load_error {
                    ui.colored_label(pal.danger, e);
                }
                if tab.diff.is_some() {
                    views::diff_view(ui, tab, &pal, &mut acts);
                } else if tab.file_view.is_some() {
                    crate::file_view::show(ui, tab, &pal, &mut acts);
                } else {
                    views::graph_view(ui, tab, &pal, opts, &mut acts);
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

        views::drag_ghost(&ctx, &pal);
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
