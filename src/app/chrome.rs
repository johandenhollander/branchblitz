//! The window around the repository: tab bar, toolbar, status bar, welcome screen.

use super::*;

impl BranchBlitz {
    pub(super) fn tab_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let mut switch = None;
        let mut close = None;
        let animate = self.settings.animation_ms > 0;
        let secs = self.settings.animation_ms.min(500) as f32 / 1000.0;
        ui.horizontal(|ui| {
            const GAP: f32 = 2.0;
            ui.spacing_mut().item_spacing.x = GAP;
            let tab_id = |ui: &egui::Ui, t: &RepoTab| ui.scope_id().with(("repo-tab", t.id));
            let label = |t: &RepoTab| -> String {
                if t.name.chars().count() > 24 {
                    t.name.chars().take(23).chain(['…']).collect()
                } else {
                    t.name.clone()
                }
            };
            // A merge, rebase or conflicts in progress: a warning dot before the name.
            let flagged = |t: &RepoTab| {
                t.data
                    .as_ref()
                    .is_some_and(|d| d.state != RepoState::Clean || !d.status.conflicted.is_empty())
            };
            let tab_w = |ui: &egui::Ui, t: &RepoTab| {
                let g =
                    ui.painter()
                        .layout_job(ds::text_job(&label(t), 12.5, 500.0, pal.text, false));
                let dot_w = if flagged(t) { 6.0 + 8.0 } else { 0.0 };
                12.0 + dot_w + g.size().x + 8.0 + 18.0 + 8.0
            };
            // Dragging a tab: the others make room where it would land, which is remembered
            // for the drop.
            let drag_state = ui.scope_id().with("tab-drag");
            let lifted = ui.input(|i| i.pointer.primary_down()).then(|| {
                self.tabs
                    .iter()
                    .position(|t| ui.ctx().is_being_dragged(tab_id(ui, t)))
            });
            if lifted.flatten().is_none()
                && let Some((from, to)) = ui
                    .ctx()
                    .data_mut(|d| d.remove_temp::<(usize, usize)>(drag_state))
            {
                // Dropped: the tabs already stand in their new order, now make it real.
                self.move_tab(from, to);
            }
            let x0 = ui.cursor().left();
            let widths: Vec<f32> = self.tabs.iter().map(|t| tab_w(ui, t)).collect();
            let mut lefts: Vec<f32> = widths
                .iter()
                .scan(0.0, |x, w| {
                    let l = *x;
                    *x += w + GAP;
                    Some(l)
                })
                .collect();
            let mut delta = egui::Vec2::ZERO;
            if let Some(Some(from)) = lifted {
                delta = ui.input(|inp| {
                    inp.pointer
                        .latest_pos()
                        .zip(inp.pointer.press_origin())
                        .map(|(now, start)| now - start)
                        .unwrap_or_default()
                });
                let center = lefts[from] + widths[from] / 2.0 + delta.x;
                let (to, slots) = tab_slots(&widths, GAP, from, center);
                lefts = slots;
                ui.ctx().data_mut(|d| d.insert_temp(drag_state, (from, to)));
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            let dragging = lifted.flatten();
            for (i, tab) in self.tabs.iter().enumerate() {
                let active = self.active == Some(i);
                let name = label(tab);
                let flagged = flagged(tab);
                let id = tab_id(ui, tab);
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
                let (natural, _) =
                    ui.allocate_exact_size(egui::vec2(widths[i], 30.0), egui::Sense::hover());
                // Drawn where it stands now, sliding there when the order changes.
                let x = ui
                    .ctx()
                    .animate_value_with_time(id.with("x"), x0 + lefts[i], secs);
                let rect = natural.translate(egui::vec2(x - natural.left(), 0.0));
                let resp = ui.interact(rect, id, egui::Sense::click_and_drag());
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, active, &tab.name)
                });
                let r = egui::CornerRadius {
                    nw: theme::RADIUS_MD,
                    ne: theme::RADIUS_MD,
                    sw: 0,
                    se: 0,
                };
                let yc = rect.center().y;
                // Dragged: an outlined, empty slot stays behind and a copy of the tab floats
                // with the pointer.
                if dragging == Some(i) {
                    ui.painter().rect(
                        rect.shrink(1.0),
                        r,
                        pal.hover,
                        egui::Stroke::new(1.0, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                    let g = ui.painter().layout_job(ds::text_job(
                        &name,
                        12.5,
                        500.0,
                        pal.text_subtle,
                        false,
                    ));
                    let x = rect.left() + 12.0 + dot_w;
                    ui.painter()
                        .galley(egui::pos2(x, yc - g.size().y / 2.0), g, pal.text_subtle);
                    let ghost = natural.translate(delta);
                    let lp = ui.ctx().layer_painter(egui::LayerId::new(
                        egui::Order::Tooltip,
                        egui::Id::unique("bb-tab-ghost"),
                    ));
                    let all = egui::CornerRadius::same(theme::RADIUS_MD);
                    lp.add(ui.visuals().popup_shadow.as_shape(ghost, all));
                    lp.rect(
                        ghost,
                        all,
                        pal.raised,
                        egui::Stroke::new(1.0, pal.border_strong),
                        egui::StrokeKind::Inside,
                    );
                    let gy = ghost.center().y;
                    let mut x = ghost.left() + 12.0;
                    if flagged {
                        lp.circle_filled(egui::pos2(x + 3.0, gy), 3.0, pal.warning);
                        x += dot_w;
                    }
                    let g = lp.layout_job(ds::text_job(&name, 12.5, 500.0, pal.text, false));
                    lp.galley(egui::pos2(x, gy - g.size().y / 2.0), g, pal.text);
                    continue;
                }
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
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, i: usize, pal: &Palette) -> Vec<Action> {
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
                    let flow = local("Git flow", icons::GIT_MERGE)
                        .enabled(head_sha.is_some())
                        .tooltip("Git flow: start and finish feature, release and hotfix branches")
                        .show(ui, pal);
                    if let Some(d) = data {
                        egui::Popup::menu(&flow)
                            .align(egui::RectAlign::BOTTOM_START)
                            .gap(4.0)
                            .show(|ui| flow_menu(ui, pal, d, &mut acts));
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

    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
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

    pub(super) fn welcome(&mut self, ui: &mut egui::Ui, pal: &Palette) {
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

/// The toolbar's Git flow menu: set it up, start a branch, finish the checked out one.
fn flow_menu(ui: &mut egui::Ui, pal: &Palette, data: &RepoData, acts: &mut Vec<Action>) {
    ui.set_width(260.0);
    if !data.flow.configured() {
        if ds::menu_item(
            ui,
            pal,
            icons::SETTINGS,
            "Initialize git flow…",
            None,
            false,
        )
        .clicked()
        {
            acts.push(Action::FlowInit);
            ui.close();
        }
        return;
    }
    for kind in git::FlowKind::ALL {
        let text = format!("Start {}…", kind.name());
        if ds::menu_item(ui, pal, icons::GIT_BRANCH_PLUS, &text, None, false).clicked() {
            acts.push(Action::Input(InputKind::FlowStart { kind }));
            ui.close();
        }
    }
    if let Some(head) = &data.head_branch
        && let Some((kind, _)) = data.flow.kind_of(head)
    {
        ds::menu_divider(ui, pal);
        views::flow_finish_item(ui, pal, data, kind, head, acts);
    }
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
    png_texture(ctx, "bolt", include_bytes!("../../assets/bolt.png"))
}

/// The small app icon (design system `app-icon-small.svg`), for the status bar.
fn app_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    png_texture(
        ctx,
        "app-icon",
        include_bytes!("../../assets/icons/branchblitz-32.png"),
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

/// Where the tabs stand while tab `from` is dragged with its centre at `center` (measured from
/// the first tab's left edge): the place to move it to (as for `move_tab`) and each tab's left
/// edge. The others make room as soon as the dragged tab passes their middle.
fn tab_slots(widths: &[f32], gap: f32, from: usize, center: f32) -> (usize, Vec<f32>) {
    let mut natural = Vec::with_capacity(widths.len());
    let mut x = 0.0;
    for w in widths {
        natural.push(x);
        x += w + gap;
    }
    let mut order: Vec<usize> = (0..widths.len()).filter(|&j| j != from).collect();
    let k = order
        .iter()
        .filter(|&&j| natural[j] + widths[j] / 2.0 <= center)
        .count();
    order.insert(k, from);
    let mut lefts = vec![0.0; widths.len()];
    let mut x = 0.0;
    for j in order {
        lefts[j] = x;
        x += widths[j] + gap;
    }
    (if k <= from { k } else { k + 1 }, lefts)
}

#[cfg(test)]
mod tests {
    use super::tab_slots;

    #[test]
    fn dragged_tab_pushes_its_neighbours_aside() {
        let w = [10.0, 20.0, 10.0];
        // Not past a neighbour's middle: everything stays.
        assert_eq!(tab_slots(&w, 0.0, 2, 36.0), (2, vec![0.0, 10.0, 30.0]));
        // Past the middle of the tab on its left: that one moves right of it.
        assert_eq!(tab_slots(&w, 0.0, 2, 19.0), (1, vec![0.0, 20.0, 10.0]));
        // All the way left.
        assert_eq!(tab_slots(&w, 0.0, 2, 1.0), (0, vec![10.0, 20.0, 0.0]));
        // The first one to the end: `move_tab` counts the place before it was taken out.
        assert_eq!(tab_slots(&w, 2.0, 0, 50.0), (3, vec![34.0, 0.0, 22.0]));
    }
}
