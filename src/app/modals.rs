//! Dialogs: Settings, errors, branch/tag names, clone, notices.

use super::*;

impl BranchBlitz {
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
            for t in &self.tabs {
                t.limit.store(self.settings.commit_limit, Ordering::Relaxed);
            }
            if let Some(i) = self.active {
                self.reload(i);
            }
        }
    }

    pub(super) fn show_modal(&mut self, ctx: &egui::Context) {
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

pub(super) fn modal_title(m: &Modal) -> String {
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
