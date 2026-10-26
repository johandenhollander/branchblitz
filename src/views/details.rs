//! The details panel: a commit's message, meta and files, or the working copy with the commit area.

use super::*;

// ---------------------------------------------------------------- details panel

/// A changed file row (`bb-file`): status letter, directory, file name.
fn file_row(
    ui: &mut Ui,
    pal: &Palette,
    status: char,
    path: &str,
    selected: bool,
    height: f32,
) -> egui::Response {
    let resp = ds::row(ui, pal, height, selected, path);
    let r = resp.rect;
    ds::status_letter(
        ui.painter(),
        pos2(r.left() + 6.0 + 8.0, r.center().y),
        status,
        pal,
    );
    let inner = r.shrink2(vec2(6.0, 0.0));
    ds::paint_path(
        &ui.painter_at(inner),
        inner,
        r.left() + 30.0,
        path,
        pal,
        12.5,
    );
    resp.on_hover_text(path)
}

pub fn details(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some(data) = tab.data.as_ref() else {
        return;
    };
    let open_diff = tab.diff.as_ref().map(|(t, _)| t.clone());
    match &tab.selected {
        Selection::None => {
            ui.add_space(56.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(icons::GIT_COMMIT_HORIZONTAL)
                        .size(24.0)
                        .color(pal.text_subtle),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("Select a commit to see its details").color(pal.text_muted));
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new("Right-click a branch to merge it into the current branch")
                            .size(12.0)
                            .color(pal.text_subtle),
                    )
                    .wrap(),
                );
            });
        }
        Selection::Commit(sha) => {
            let Some(d) = tab.details.as_ref().filter(|d| &d.sha == sha) else {
                ui.add_space(16.0);
                match &tab.details_error {
                    Some(e) => {
                        ui.colored_label(pal.danger, e);
                    }
                    None => {
                        ui.spinner();
                    }
                }
                return;
            };
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 12.0;
                            let (subject, body) =
                                d.message.split_once('\n').unwrap_or((&d.message, ""));
                            ui.add(egui::Label::new(ds::title(subject).color(pal.text)).wrap());
                            if !body.trim().is_empty() {
                                // Re-flow the commit's hard line breaks, keep its paragraphs.
                                let flowed = body
                                    .trim()
                                    .split("\n\n")
                                    .map(|p| p.lines().map(str::trim).collect::<Vec<_>>().join(" "))
                                    .collect::<Vec<_>>()
                                    .join("\n\n");
                                ui.add(
                                    egui::Label::new(RichText::new(flowed).color(pal.text_muted))
                                        .wrap(),
                                );
                            }
                            commit_meta(ui, pal, tab, data, d, acts);
                            let n = d.files.len();
                            ui.label(ds::caption(
                                &if n == 1 {
                                    "1 changed file".into()
                                } else {
                                    format!("{n} changed files")
                                },
                                pal,
                            ));
                            ui.add_space(-8.0);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for f in &d.files {
                                let target = DiffTarget::Commit {
                                    sha: d.sha.clone(),
                                    path: f.path.clone(),
                                };
                                let shown = if let Some(old) = &f.old_path {
                                    format!("{old} -> {}", f.path)
                                } else {
                                    f.path.clone()
                                };
                                if file_row(
                                    ui,
                                    pal,
                                    f.status,
                                    &shown,
                                    open_diff.as_ref() == Some(&target),
                                    26.0,
                                )
                                .clicked()
                                {
                                    acts.push(Action::ShowDiff(target));
                                }
                            }
                        });
                });
        }
        Selection::Wip => wip_panel(ui, tab, pal, acts, open_diff),
    }
}

/// Commit, Parent, Author, Date, On: a two-column definition list.
fn commit_meta(
    ui: &mut Ui,
    pal: &Palette,
    tab: &RepoTab,
    data: &RepoData,
    d: &git::CommitDetails,
    acts: &mut Vec<Action>,
) {
    let dt = |ui: &mut Ui, t: &str| {
        ui.label(RichText::new(t).size(12.0).color(pal.text_subtle));
    };
    egui::Grid::new("commit-meta")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .min_col_width(64.0)
        .min_row_height(22.0)
        .show(ui, |ui| {
            dt(ui, "Commit");
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(git::short(&d.sha))
                        .monospace()
                        .size(11.5)
                        .color(pal.text),
                );
                if ds::Button::icon_only(icons::COPY, "Copy SHA")
                    .quiet()
                    .small()
                    .show(ui, pal)
                    .clicked()
                {
                    acts.push(Action::Copy(d.sha.clone()));
                }
            });
            ui.end_row();
            dt(
                ui,
                if d.parents.len() > 1 {
                    "Parents"
                } else {
                    "Parent"
                },
            );
            ui.horizontal(|ui| {
                for p in &d.parents {
                    let link = ui
                        .add(
                            egui::Label::new(
                                RichText::new(git::short(p))
                                    .monospace()
                                    .size(11.5)
                                    .color(pal.accent),
                            )
                            .sense(Sense::click()),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    if link.clicked() {
                        acts.push(Action::Jump(p.clone()));
                    }
                }
            });
            ui.end_row();
            dt(ui, "Author");
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                let lane = tab
                    .row_of
                    .get(d.sha.as_str())
                    .and_then(|&i| tab.coloring.commit.get(i))
                    .map(|&k| tab.coloring.color(k, pal))
                    .unwrap_or(pal.accent);
                ds::avatar(ui.painter(), r.center(), &initials(&d.author), lane);
                ui.label(RichText::new(&d.author).color(pal.text))
                    .on_hover_text(&d.email);
            });
            ui.end_row();
            dt(ui, "Date");
            ui.label(RichText::new(fmt_time_long(d.time)).color(pal.text_muted));
            ui.end_row();
            if d.committer != d.author {
                dt(ui, "Committer");
                ui.label(RichText::new(&d.committer).color(pal.text_muted));
                ui.end_row();
            }
            if let Some(idxs) = tab.refs_at.get(&d.sha) {
                dt(ui, "On");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for pill in pills_for(idxs, data) {
                        let lane = pill_lane(&pill, data, &tab.coloring, pal, pal.accent);
                        let (g, w) = ds::chip_layout_prefixed(
                            ui.painter(),
                            pill.prefix.as_deref(),
                            &pill.text,
                            pill.kind,
                            pal,
                            260.0,
                        );
                        let (r, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                        ds::paint_chip(ui.painter(), r.left_center(), g, w, pill.kind, lane, pal);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::Button,
                                true,
                                pill.full_names(data),
                            )
                        });
                        let resp = resp.on_hover_text(pill.full_names(data));
                        resp.context_menu(|ui| pill_menu(ui, pal, &pill, data, lane, acts));
                    }
                });
                ui.end_row();
            }
        });
}

/// `CommitPanel`: review the working copy, stage files and write the commit.
fn wip_panel(
    ui: &mut Ui,
    tab: &mut RepoTab,
    pal: &Palette,
    acts: &mut Vec<Action>,
    open_diff: Option<DiffTarget>,
) {
    let Some(data) = tab.data.as_ref() else {
        return;
    };
    let st = &data.status;
    let id = ui.scope_id();

    // Header: "N file changes on <branch>" and discard-all at the far right.
    let n = st.count();
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 12,
            right: 8,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_height(44.0);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let text = if n == 1 {
                    "1 file change on".to_string()
                } else {
                    format!("{n} file changes on")
                };
                ui.label(ds::strong(text).color(pal.text));
                if let Some(b) = data.current_ref() {
                    let lane = tab.coloring.branch(&b.name, pal).unwrap_or(pal.accent);
                    let kind = chip_kind(b, data);
                    let (g, w) = ds::chip_layout(
                        ui.painter(),
                        &b.name,
                        kind,
                        pal,
                        ui.available_width() - 40.0,
                    );
                    let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
                    ds::paint_chip(ui.painter(), r.left_center(), g, w, kind, lane, pal);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = n > 0 && data.state == RepoState::Clean && data.head_sha.is_some();
                    if ds::Button::icon_only(icons::TRASH_2, "Discard all changes…")
                        .quiet()
                        .small()
                        .enabled(can)
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(discard_all(st));
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));

    // The commit area is pinned to the bottom; the files scroll above it.
    let foot_h = 214.0;
    let avail = ui.available_height();
    egui::ScrollArea::vertical()
        .max_height((avail - foot_h).max(80.0))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if !st.conflicted.is_empty() {
                        conflicts(ui, pal, data, id, &open_diff, acts);
                    }
                    let open = section_with_action(
                        ui,
                        pal,
                        id.with("unstaged"),
                        "Unstaged",
                        st.unstaged.len(),
                        "Stage all",
                        acts,
                        Action::Op(Op::StageAll),
                    );
                    if open {
                        if st.unstaged.is_empty() {
                            empty_line(ui, pal, "No unstaged changes");
                        }
                        for f in &st.unstaged {
                            let untracked = f.status == '?';
                            let target = DiffTarget::Unstaged {
                                path: f.path.clone(),
                                untracked,
                            };
                            let selected = open_diff.as_ref() == Some(&target);
                            let status = if untracked { 'A' } else { f.status };
                            let resp = file_row(ui, pal, status, &f.path, selected, ROW_H);
                            let (stage, discard) =
                                row_actions(ui, pal, &resp, selected, "Stage", &f.path, true);
                            if stage || resp.double_clicked() {
                                acts.push(Action::Op(Op::Stage(vec![f.path.clone()])));
                            } else if discard {
                                acts.push(Action::Confirm {
                                    title: format!("Discard changes in {}?", f.path),
                                    body: "This cannot be undone.".into(),
                                    op: Op::Discard {
                                        path: f.path.clone(),
                                        untracked,
                                    },
                                    danger: true,
                                });
                            } else if resp.clicked() {
                                acts.push(Action::ShowDiff(target));
                            }
                        }
                    }
                    ui.add_space(8.0);
                    let open = section_with_action(
                        ui,
                        pal,
                        id.with("staged"),
                        "Staged",
                        st.staged.len(),
                        "Unstage all",
                        acts,
                        Action::Op(Op::UnstageAll),
                    );
                    if open {
                        if st.staged.is_empty() {
                            empty_line(ui, pal, "Stage files to include them in the commit");
                        }
                        for f in &st.staged {
                            let target = DiffTarget::Staged {
                                path: f.path.clone(),
                            };
                            let selected = open_diff.as_ref() == Some(&target);
                            let resp = file_row(ui, pal, f.status, &f.path, selected, ROW_H);
                            let (unstage, _) =
                                row_actions(ui, pal, &resp, selected, "Unstage", &f.path, false);
                            if unstage || resp.double_clicked() {
                                acts.push(Action::Op(Op::Unstage(vec![f.path.clone()])));
                            } else if resp.clicked() {
                                acts.push(Action::ShowDiff(target));
                            }
                        }
                    }
                });
        });

    commit_area(ui, tab, pal, acts);
}

/// The confirmation for "Discard all changes…", naming the first files.
fn discard_all(st: &git::WorkStatus) -> Action {
    let mut files: Vec<&str> = st
        .staged
        .iter()
        .chain(&st.unstaged)
        .map(|f| f.path.as_str())
        .collect();
    files.sort_unstable();
    files.dedup();
    let more = files.len().saturating_sub(3);
    let mut list = files[..files.len() - more].join(", ");
    if more > 0 {
        list += &format!(" and {more} more");
    }
    Action::Confirm {
        title: "Discard all changes?".into(),
        body: format!("Staged and unstaged changes and new files are lost: {list}"),
        op: Op::DiscardAll,
        danger: true,
    }
}

fn conflicts(
    ui: &mut Ui,
    pal: &Palette,
    data: &RepoData,
    id: egui::Id,
    open_diff: &Option<DiffTarget>,
    acts: &mut Vec<Action>,
) {
    let st = &data.status;
    if !ds::section_header(
        ui,
        pal,
        id.with("conflicts"),
        "Conflicts",
        Some(st.conflicted.len()),
        true,
        false,
    ) {
        return;
    }
    for p in &st.conflicted {
        let target = DiffTarget::Conflict { path: p.clone() };
        let resp = file_row(ui, pal, 'U', p, open_diff.as_ref() == Some(&target), ROW_H);
        if resp.clicked() {
            acts.push(Action::ShowDiff(target));
        }
        resp.context_menu(|ui| {
            ui.set_width(260.0);
            // During a rebase git's "ours" is the branch being rebased onto,
            // so "mine" is "--theirs" there.
            let mine_is_ours = data.state != RepoState::Rebasing;
            if ds::menu_item(ui, pal, icons::CHECK, "Take my version", None, false).clicked() {
                acts.push(Action::Op(Op::TakeSide {
                    path: p.clone(),
                    ours: mine_is_ours,
                }));
                ui.close();
            }
            if ds::menu_item(
                ui,
                pal,
                icons::DOWNLOAD,
                "Take incoming version",
                None,
                false,
            )
            .clicked()
            {
                acts.push(Action::Op(Op::TakeSide {
                    path: p.clone(),
                    ours: !mine_is_ours,
                }));
                ui.close();
            }
            if ds::menu_item(ui, pal, icons::PENCIL, "Open in editor", None, false).clicked() {
                acts.push(Action::OpenFile(p.clone()));
                ui.close();
            }
            ds::menu_divider(ui, pal);
            if ds::menu_item(
                ui,
                pal,
                icons::CIRCLE_CHECK,
                "Mark as resolved",
                None,
                false,
            )
            .clicked()
            {
                acts.push(Action::Op(Op::MarkResolved(p.clone())));
                ui.close();
            }
        });
    }
    empty_line(
        ui,
        pal,
        "Click a file to choose per conflict, or right-click for more",
    );
    ui.add_space(8.0);
}

/// Section header with a count badge after the caption and a small quiet action at the
/// right ("Stage all", disabled at 0). Returns whether the section is open.
#[allow(clippy::too_many_arguments)]
fn section_with_action(
    ui: &mut Ui,
    pal: &Palette,
    id: egui::Id,
    title: &str,
    count: usize,
    action: &str,
    acts: &mut Vec<Action>,
    a: Action,
) -> bool {
    let open = ds::section_header(ui, pal, id, title, None, true, false);
    let row = ui.min_rect().intersect(Rect::from_min_max(
        pos2(f32::NEG_INFINITY, ui.cursor().top() - ROW_H),
        pos2(f32::INFINITY, ui.cursor().top()),
    ));
    let cap_w = egui::WidgetText::from(ds::caption(title, pal))
        .into_galley(ui, None, f32::INFINITY, egui::TextStyle::Body)
        .size()
        .x;
    let g = ui.painter().layout_job(ds::text_job(
        &count.to_string(),
        11.0,
        600.0,
        pal.text_muted,
        true,
    ));
    let badge = Rect::from_min_size(
        pos2(row.left() + 26.0 + cap_w + 6.0, row.center().y - 9.0),
        vec2(g.size().x + 12.0, 18.0),
    );
    ui.painter()
        .rect_filled(badge, CornerRadius::same(9), pal.control);
    ui.painter()
        .galley(badge.center() - g.size() / 2.0, g, pal.text_muted);
    let btn = Rect::from_min_max(
        pos2(row.right() - 120.0, row.top() + 2.0),
        pos2(row.right(), row.bottom() - 2.0),
    );
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(btn)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| {
            if ds::Button::new(action)
                .quiet()
                .small()
                .enabled(count > 0)
                .show(ui, pal)
                .clicked()
            {
                acts.push(a);
            }
        },
    );
    open
}

fn empty_line(ui: &mut Ui, pal: &Palette, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(26.0);
        ui.add(egui::Label::new(RichText::new(text).size(12.0).color(pal.text_subtle)).wrap());
    });
    ui.add_space(6.0);
}

/// Row actions shown on hover and on the selected row: "Stage"/"Unstage" and, for unstaged
/// files, discard. Returns (main clicked, discard clicked).
fn row_actions(
    ui: &mut Ui,
    pal: &Palette,
    row: &egui::Response,
    selected: bool,
    main: &str,
    path: &str,
    discard: bool,
) -> (bool, bool) {
    let r = row.rect;
    if !(ui.rect_contains_pointer(r) || selected) {
        return (false, false);
    }
    let main_w = if discard { 52.0 } else { 64.0 };
    let width = main_w + if discard { 24.0 } else { 0.0 } + 8.0;
    let area = Rect::from_min_max(
        pos2(r.right() - width, r.top() + 3.0),
        pos2(r.right() - 4.0, r.bottom() - 3.0),
    );
    // Cover the end of the path under the buttons.
    ui.painter().rect_filled(
        Rect::from_min_max(pos2(area.left() - 4.0, r.top()), r.right_bottom()),
        CornerRadius::same(5),
        if selected { pal.selected } else { pal.hover },
    );
    let mut out = (false, false);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            out.0 = ds::Button::new(main)
                .quiet()
                .small()
                .size(vec2(main_w, 22.0))
                .label(format!("{main} {path}"))
                .show(ui, pal)
                .clicked();
            if discard {
                out.1 = ds::Button::icon_only(icons::UNDO_2, "Discard changes")
                    .quiet()
                    .small()
                    .size(vec2(22.0, 22.0))
                    .show(ui, pal)
                    .clicked();
            }
        },
    );
    out
}

/// Commit message (72-character countdown) and description, either one enough, Amend, and one
/// full-width button that says what is missing or what will happen.
fn commit_area(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some(data) = tab.data.as_ref() else {
        return;
    };
    let st = &data.status;
    let staged = st.staged.len();
    let conflicts = !st.conflicted.is_empty();
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    let summary_id = ui.scope_id().with("commit-summary");
    let body_id = ui.scope_id().with("commit-body");
    egui::Frame::new()
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let focused = ui
                .ctx()
                .memory(|m| m.has_focus(summary_id) || m.has_focus(body_id));
            let field = egui::Frame::new()
                .fill(pal.canvas)
                .stroke(Stroke::new(1.0, pal.border_control))
                .corner_radius(CornerRadius::same(crate::theme::RADIUS_MD))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        let left = 72 - tab.commit_msg.chars().count() as i64;
                        let color = if left < 0 {
                            pal.warning
                        } else {
                            pal.text_subtle
                        };
                        let cg = ui.painter().layout_job(ds::text_job(
                            &left.to_string(),
                            11.0,
                            500.0,
                            color,
                            true,
                        ));
                        ui.add_sized(
                            vec2(ui.available_width() - cg.size().x - 16.0, 32.0),
                            egui::TextEdit::singleline(&mut tab.commit_msg)
                                .id(summary_id)
                                .frame(egui::Frame::NONE)
                                .hint_text(RichText::new("Commit message").color(pal.text_subtle))
                                .align(egui::Align2::LEFT_CENTER),
                        );
                        let (r, _) =
                            ui.allocate_exact_size(vec2(cg.size().x + 8.0, 32.0), Sense::hover());
                        ui.painter().galley(
                            pos2(r.left(), r.center().y - cg.size().y / 2.0),
                            cg,
                            color,
                        );
                    });
                    let sep = ui.available_rect_before_wrap();
                    ui.painter()
                        .hline(sep.x_range(), sep.top(), Stroke::new(1.0, pal.border));
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut tab.commit_body)
                                    .id(body_id)
                                    .frame(egui::Frame::NONE)
                                    .hint_text(RichText::new("Description").color(pal.text_subtle))
                                    .desired_rows(3)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                });
            if focused {
                ui.painter().rect_stroke(
                    field.response.rect.expand(1.0),
                    CornerRadius::same(crate::theme::RADIUS_MD + 1),
                    Stroke::new(2.0, pal.focus),
                    egui::StrokeKind::Outside,
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut tab.amend, "Amend previous commit")
                    .changed()
                {
                    acts.push(Action::Amend(tab.amend));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new("Ctrl Enter")
                            .monospace()
                            .size(11.0)
                            .color(pal.text_subtle),
                    );
                });
            });
            // Either field will do: some people only write the description.
            let has_message =
                !tab.commit_msg.trim().is_empty() || !tab.commit_body.trim().is_empty();
            let label = if tab.amend {
                "Amend last commit".to_string()
            } else if staged == 0 {
                "Stage files to commit".to_string()
            } else if !has_message {
                "Write a message to commit".to_string()
            } else if staged == 1 {
                "Commit 1 file".to_string()
            } else {
                format!("Commit {staged} files")
            };
            let can_commit = has_message && (staged > 0 || tab.amend) && !conflicts;
            let ctrl_enter =
                focused && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
            let clicked = ds::Button::new(label)
                .icon(icons::GIT_COMMIT_HORIZONTAL)
                .primary()
                .enabled(can_commit)
                .size(vec2(ui.available_width(), 34.0))
                .show(ui, pal)
                .clicked();
            if (clicked || ctrl_enter) && can_commit {
                acts.push(Action::Op(Op::Commit {
                    message: commit_message(&tab.commit_msg, &tab.commit_body),
                    amend: tab.amend,
                }));
            }
        });
}

/// Summary and description as one message, separated by a blank line. Without a summary the
/// description is the whole message.
fn commit_message(summary: &str, body: &str) -> String {
    match (summary.trim(), body.trim()) {
        (summary, "") => summary.to_string(),
        ("", body) => body.to_string(),
        (summary, body) => format!("{summary}\n\n{body}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_message_joins_summary_and_description() {
        assert_eq!(commit_message(" Fix it ", "  "), "Fix it");
        assert_eq!(
            commit_message("Fix it", "\nWhy.\n\nMore.\n"),
            "Fix it\n\nWhy.\n\nMore."
        );
        assert_eq!(commit_message("  ", "\nFix it\n\nWhy.\n"), "Fix it\n\nWhy.");
    }
}
