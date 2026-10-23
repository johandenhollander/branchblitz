use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, RichText, Sense, Stroke, Ui,
    epaint::CubicBezierShape, pos2, text::LayoutJob, vec2,
};

use crate::app::{Action, Find, InputKind, RepoTab, Selection};
use crate::ds::{self, ChipKind};
use crate::git::{
    self, DiffTarget, MergeMode, Op, RefInfo, RefKind, RepoData, RepoState, ResetMode,
};
use crate::icons;
use crate::theme::Palette;

const ROW_H: f32 = ds::ROW_H;
const LANE_W: f32 = 16.0;
const GRAPH_PAD: f32 = 10.0;
/// Commit dots are 11 px; HEAD gets a ring around it.
const NODE_R: f32 = 5.5;

// ---------------------------------------------------------------- helpers

/// `19 Oct 13:45` in the commit list; the year only when it is not this year.
pub fn fmt_time(t: i64) -> String {
    let Some(dt) = Local.timestamp_opt(t, 0).single() else {
        return String::new();
    };
    if dt.year() == Local::now().year() {
        dt.format("%-d %b %H:%M").to_string()
    } else {
        dt.format("%-d %b %Y").to_string()
    }
}

/// `Tue 13 Oct 2026, 12:48` in the detail panel.
fn fmt_time_long(t: i64) -> String {
    Local
        .timestamp_opt(t, 0)
        .single()
        .map(|dt| dt.format("%a %-d %b %Y, %H:%M").to_string())
        .unwrap_or_default()
}

fn initials(name: &str) -> String {
    let mut parts = name
        .split_whitespace()
        .filter_map(|p| p.chars().next())
        .filter(|c| c.is_alphanumeric());
    let first = parts.next();
    let last = parts.next_back();
    [first, last]
        .iter()
        .flatten()
        .flat_map(|c| c.to_uppercase())
        .collect()
}

fn current_name(data: &RepoData) -> String {
    data.head_branch.clone().unwrap_or_else(|| "HEAD".into())
}

/// Length of a leading ticket key such as `PROJ-601` (capitals and digits, dash, digits).
fn ticket_key_len(subject: &str) -> usize {
    let b = subject.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i].is_ascii_uppercase() || (i > 0 && b[i].is_ascii_digit())) {
        i += 1;
    }
    if i == 0 || b.get(i) != Some(&b'-') {
        return 0;
    }
    let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 { 0 } else { i + 1 + digits }
}

fn chip_kind(r: &RefInfo, data: &RepoData) -> ChipKind {
    ChipKind {
        head: r.kind == RefKind::Local && data.head_branch.as_deref() == Some(r.name.as_str()),
        local: r.kind == RefKind::Local,
        remote: r.kind == RefKind::Remote,
        tag: r.kind == RefKind::Tag,
        default: data.is_default_branch(r),
        unpushed: r.kind == RefKind::Tag && data.tag_on_remote(r) == Some(false),
        worktree: r.kind == RefKind::Local && data.worktree_of(&r.name).is_some(),
    }
}

// ---------------------------------------------------------------- context menus

/// The menu for a branch, remote branch or tag. Visible labels are written relative to the
/// current branch; the full description is the item's accessible name and tooltip.
pub fn ref_menu(
    ui: &mut Ui,
    pal: &Palette,
    r: &RefInfo,
    data: &RepoData,
    lane: Color32,
    acts: &mut Vec<Action>,
) {
    ui.set_width(360.0);
    let current = current_name(data);
    let is_current =
        r.kind == RefKind::Local && data.head_branch.as_deref() == Some(r.name.as_str());
    let has_head = data.head_sha.is_some();
    let act = |ui: &mut Ui,
               icon: &'static str,
               text: String,
               full: Option<String>,
               a: Action,
               acts: &mut Vec<Action>| {
        if ds::menu_item(ui, pal, icon, &text, full.as_deref(), false).clicked() {
            acts.push(a);
            ui.close();
        }
    };
    let danger =
        |ui: &mut Ui, icon: &'static str, text: String, a: Action, acts: &mut Vec<Action>| {
            if ds::menu_item(ui, pal, icon, &text, None, true).clicked() {
                acts.push(a);
                ui.close();
            }
        };

    ds::menu_title(ui, pal, &r.name, Some((chip_kind(r, data), lane)));

    // Tags are not merged or rebased onto; a branch already in HEAD has nothing to bring in.
    let mergeable = has_head && !is_current && r.kind != RefKind::Tag;
    if mergeable && data.in_head(&r.target) {
        ds::menu_item_disabled(ui, pal, icons::CHECK, &format!("Already in {current}"));
        ds::menu_divider(ui, pal);
    } else if mergeable {
        act(
            ui,
            icons::GIT_MERGE,
            format!("Merge into {current}"),
            Some(format!("Merge {} into {current}", r.name)),
            Action::Op(Op::Merge {
                what: data.spec(r),
                mode: MergeMode::Default,
            }),
            acts,
        );
        ds::submenu(ui, pal, icons::GIT_MERGE, "More merge options", |ui| {
            ui.set_width(360.0);
            act(
                ui,
                icons::GIT_MERGE,
                "Always create a merge commit".into(),
                Some(format!(
                    "Merge {} into {current} (always create merge commit)",
                    r.name
                )),
                Action::Op(Op::Merge {
                    what: data.spec(r),
                    mode: MergeMode::NoFastForward,
                }),
                acts,
            );
            act(
                ui,
                icons::ARROW_UP,
                "Fast-forward only".into(),
                Some(format!("Fast-forward {current} to {}", r.name)),
                Action::Op(Op::Merge {
                    what: data.spec(r),
                    mode: MergeMode::FastForwardOnly,
                }),
                acts,
            );
            act(
                ui,
                icons::GIT_COMMIT_HORIZONTAL,
                "Squash into one change".into(),
                Some(format!("Squash {} into {current}", r.name)),
                Action::Op(Op::Merge {
                    what: data.spec(r),
                    mode: MergeMode::Squash,
                }),
                acts,
            );
        });
        act(
            ui,
            icons::GIT_COMPARE_ARROWS,
            format!("Rebase {current} onto this"),
            Some(format!("Rebase {current} onto {}", r.name)),
            Action::Op(Op::Rebase(data.spec(r))),
            acts,
        );
        ds::menu_divider(ui, pal);
    } else if r.kind == RefKind::Tag && has_head && !data.in_head(&r.target) {
        // A tag is not merged, but rebasing onto a release tag is fine.
        act(
            ui,
            icons::GIT_COMPARE_ARROWS,
            format!("Rebase {current} onto this"),
            Some(format!("Rebase {current} onto {}", r.name)),
            Action::Op(Op::Rebase(data.spec(r))),
            acts,
        );
        ds::menu_divider(ui, pal);
    }

    match r.kind {
        RefKind::Local => {
            if !is_current {
                checkout_item(ui, pal, &r.name, data, acts);
            } else {
                act(
                    ui,
                    icons::ARROW_DOWN_TO_LINE,
                    "Pull".into(),
                    None,
                    Action::Op(Op::Pull(git::PullMode::Merge)),
                    acts,
                );
                act(
                    ui,
                    icons::ARROW_UP_FROM_LINE,
                    "Push".into(),
                    None,
                    Action::Op(Op::Push { force: false }),
                    acts,
                );
            }
            if let Some(up) = &r.upstream {
                // `git fetch .` won't move a branch checked out in another worktree.
                if r.behind > 0
                    && r.ahead == 0
                    && !is_current
                    && data.worktree_of(&r.name).is_none()
                {
                    act(
                        ui,
                        icons::ARROW_DOWN_TO_LINE,
                        format!("Fast-forward to {up}"),
                        Some(format!("Fast-forward {} to {up}", r.name)),
                        Action::Op(Op::FastForward {
                            local: r.name.clone(),
                            to: up.clone(),
                        }),
                        acts,
                    );
                }
            } else if let Some(remote) = data.default_remote() {
                act(
                    ui,
                    icons::ARROW_UP_FROM_LINE,
                    format!("Push to {remote}"),
                    Some(format!("Push {} to {remote}", r.name)),
                    Action::Op(Op::PushBranch {
                        remote: remote.to_string(),
                        branch: r.name.clone(),
                    }),
                    acts,
                );
            }
        }
        RefKind::Remote => {
            let local = r.branch_name().to_string();
            match data.local(&local) {
                Some(l) if data.head_branch.as_deref() == Some(local.as_str()) => {
                    // Pulling only makes sense when this is the branch's own upstream.
                    if l.upstream.as_deref() == Some(r.name.as_str()) {
                        act(
                            ui,
                            icons::ARROW_DOWN_TO_LINE,
                            format!("Pull into {local}"),
                            None,
                            Action::Op(Op::Pull(git::PullMode::Merge)),
                            acts,
                        );
                    }
                }
                Some(l) => {
                    checkout_item(ui, pal, &local, data, acts);
                    if l.upstream.as_deref() == Some(r.name.as_str())
                        && data.worktree_of(&local).is_none()
                        && l.behind > 0
                        && l.ahead == 0
                    {
                        act(
                            ui,
                            icons::ARROW_DOWN_TO_LINE,
                            format!("Fast-forward {local} to this"),
                            Some(format!("Fast-forward {local} to {}", r.name)),
                            Action::Op(Op::FastForward {
                                local: local.clone(),
                                to: r.name.clone(),
                            }),
                            acts,
                        );
                    }
                }
                None => act(
                    ui,
                    icons::GIT_BRANCH,
                    format!("Check out as {local}"),
                    Some(format!("Checkout as new local branch {local}")),
                    Action::Op(Op::CheckoutRemote {
                        remote_ref: r.name.clone(),
                        local: local.clone(),
                    }),
                    acts,
                ),
            }
        }
        RefKind::Tag => {
            act(
                ui,
                icons::CIRCLE_DOT,
                "Check out (detached)".into(),
                Some(format!("Checkout {} (detached)", r.name)),
                Action::Op(Op::CheckoutCommit(r.name.clone())),
                acts,
            );
        }
    }

    ds::menu_divider(ui, pal);
    act(
        ui,
        icons::GIT_BRANCH_PLUS,
        "Create branch here…".into(),
        None,
        Action::Input(InputKind::Branch {
            at: r.target.clone(),
        }),
        acts,
    );
    if r.kind != RefKind::Tag {
        act(
            ui,
            icons::TAG,
            "Create tag here…".into(),
            None,
            Action::Input(InputKind::Tag {
                at: r.target.clone(),
            }),
            acts,
        );
    }
    ds::menu_divider(ui, pal);
    act(
        ui,
        icons::COPY,
        "Copy name".into(),
        None,
        Action::Copy(r.name.clone()),
        acts,
    );
    act(
        ui,
        icons::COPY,
        "Copy commit SHA".into(),
        None,
        Action::Copy(r.target.clone()),
        acts,
    );
    // Destructive last.
    match r.kind {
        RefKind::Local => {
            ds::menu_divider(ui, pal);
            act(
                ui,
                icons::PENCIL,
                "Rename…".into(),
                None,
                Action::Input(InputKind::Rename {
                    old: r.name.clone(),
                }),
                acts,
            );
            if is_current {
                // Git cannot delete the branch you are on; say so instead of hiding it.
                ds::menu_item_disabled(
                    ui,
                    pal,
                    icons::TRASH_2,
                    "Delete (check out another branch first)",
                );
            } else if data.worktree_of(&r.name).is_some() {
                ds::menu_item_disabled(
                    ui,
                    pal,
                    icons::TRASH_2,
                    "Delete (checked out in another worktree)",
                );
            } else {
                danger(
                    ui,
                    icons::TRASH_2,
                    format!("Delete {}", r.name),
                    Action::Op(Op::DeleteBranch {
                        name: r.name.clone(),
                        force: false,
                    }),
                    acts,
                );
            }
        }
        RefKind::Remote => {
            ds::menu_divider(ui, pal);
            let remote = r.remote.clone().unwrap_or_default();
            let branch = r.branch_name().to_string();
            danger(
                ui,
                icons::TRASH_2,
                format!("Delete {branch} on {remote}"),
                Action::Confirm {
                    title: format!("Delete {branch} on {remote}?"),
                    body: format!(
                        "This removes the branch {branch} from the remote {remote} for everyone."
                    ),
                    op: Op::DeleteRemoteBranch { remote, branch },
                    danger: true,
                },
                acts,
            );
        }
        RefKind::Tag => {
            ds::menu_divider(ui, pal);
            let pushed = data.tag_on_remote(r) == Some(true);
            if let Some(remote) = data.default_remote().filter(|_| !pushed) {
                act(
                    ui,
                    icons::UPLOAD,
                    format!("Push tag to {remote}"),
                    None,
                    Action::Op(Op::PushTag {
                        remote: remote.to_string(),
                        name: r.name.clone(),
                    }),
                    acts,
                );
            }
            danger(
                ui,
                icons::TRASH_2,
                format!("Delete tag {} (local)", r.name),
                Action::Op(Op::DeleteTag(r.name.clone())),
                acts,
            );
        }
    }
}

fn commit_menu(
    ui: &mut Ui,
    pal: &Palette,
    sha: &str,
    subject: &str,
    data: &RepoData,
    acts: &mut Vec<Action>,
) {
    ui.set_width(360.0);
    let current = current_name(data);
    let act = |ui: &mut Ui,
               icon: &'static str,
               text: String,
               full: Option<String>,
               a: Action,
               acts: &mut Vec<Action>| {
        if ds::menu_item(ui, pal, icon, &text, full.as_deref(), false).clicked() {
            acts.push(a);
            ui.close();
        }
    };
    ds::menu_title(
        ui,
        pal,
        &format!(
            "{}  {}",
            git::short(sha),
            subject.chars().take(48).collect::<String>()
        ),
        None,
    );
    if data.head_sha.is_some() && !data.in_head(sha) {
        act(
            ui,
            icons::GIT_MERGE,
            format!("Merge into {current}"),
            Some(format!("Merge this commit into {current}")),
            Action::Op(Op::Merge {
                what: sha.to_string(),
                mode: MergeMode::Default,
            }),
            acts,
        );
        act(
            ui,
            icons::CHERRY,
            format!("Cherry-pick into {current}"),
            None,
            Action::Op(Op::CherryPick(sha.to_string())),
            acts,
        );
        ds::menu_divider(ui, pal);
    }
    act(
        ui,
        icons::CIRCLE_DOT,
        "Check out this commit (detached)".into(),
        Some("Checkout this commit (detached)".into()),
        Action::Op(Op::CheckoutCommit(sha.to_string())),
        acts,
    );
    act(
        ui,
        icons::GIT_BRANCH_PLUS,
        "Create branch here…".into(),
        None,
        Action::Input(InputKind::Branch {
            at: sha.to_string(),
        }),
        acts,
    );
    act(
        ui,
        icons::TAG,
        "Create tag here…".into(),
        None,
        Action::Input(InputKind::Tag {
            at: sha.to_string(),
        }),
        acts,
    );
    ds::menu_divider(ui, pal);
    act(
        ui,
        icons::COPY,
        "Copy SHA".into(),
        None,
        Action::Copy(sha.to_string()),
        acts,
    );
    act(
        ui,
        icons::COPY,
        "Copy message".into(),
        None,
        Action::Copy(subject.to_string()),
        acts,
    );
    ds::menu_divider(ui, pal);
    act(
        ui,
        icons::UNDO_2,
        "Revert this commit".into(),
        None,
        Action::Op(Op::Revert(sha.to_string())),
        acts,
    );
    ds::submenu(
        ui,
        pal,
        icons::ROTATE_CCW,
        &format!("Reset {current} to this commit"),
        |ui| {
            ui.set_width(300.0);
            act(
                ui,
                icons::ROTATE_CCW,
                "Soft — keep all changes staged".into(),
                None,
                Action::Op(Op::Reset {
                    to: sha.to_string(),
                    mode: ResetMode::Soft,
                }),
                acts,
            );
            act(
                ui,
                icons::ROTATE_CCW,
                "Mixed — keep changes unstaged".into(),
                None,
                Action::Op(Op::Reset {
                    to: sha.to_string(),
                    mode: ResetMode::Mixed,
                }),
                acts,
            );
            if ds::menu_item(
                ui,
                pal,
                icons::TRIANGLE_ALERT,
                "Hard — discard all changes",
                None,
                true,
            )
            .clicked()
            {
                acts.push(Action::Confirm {
                    title: format!("Hard reset {current}?"),
                    body: format!(
                        "{current} will point to {} and all uncommitted changes are lost.",
                        git::short(sha)
                    ),
                    op: Op::Reset {
                        to: sha.to_string(),
                        mode: ResetMode::Hard,
                    },
                    danger: true,
                });
                ui.close();
            }
        },
    );
}

// ---------------------------------------------------------------- sidebar

#[derive(Default)]
struct Tree {
    children: BTreeMap<String, Tree>,
    leaves: Vec<usize>,
}

impl Tree {
    fn insert(&mut self, path: &[&str], idx: usize) {
        match path {
            [] | [_] => self.leaves.push(idx),
            [head, rest @ ..] => self
                .children
                .entry(head.to_string())
                .or_default()
                .insert(rest, idx),
        }
    }
}

/// What the sidebar rows need besides the row itself.
struct Side<'a> {
    data: &'a RepoData,
    coloring: &'a crate::colors::Coloring,
    selected: Option<&'a str>,
    pal: &'a Palette,
    tree: bool,
}

pub fn sidebar(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let field = ds::search_field(
        ui,
        pal,
        &mut tab.filter,
        "Filter branches & tags",
        Some("Ctrl K"),
    );
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::K)) {
        field.request_focus();
    }
    if field.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        tab.filter.clear();
        field.surrender_focus();
    }
    ui.add_space(8.0);
    let Some(data) = tab.data.as_ref() else {
        ui.spinner();
        return;
    };
    let filter = tab.filter.to_lowercase();
    let filtering = !filter.is_empty();
    let side = Side {
        data,
        coloring: &tab.coloring,
        selected: match &tab.selected {
            Selection::Commit(s) => Some(s.as_str()),
            _ => None,
        },
        pal,
        tree: !filtering,
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let id = ui.scope_id();
            let idx_of = |kind: RefKind| -> Vec<usize> {
                data.refs
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        r.kind == kind
                            && (filter.is_empty() || r.name.to_lowercase().contains(&filter))
                    })
                    .map(|(i, _)| i)
                    .collect()
            };

            // The checked-out branch is pinned first in Local.
            let mut locals = idx_of(RefKind::Local);
            let head = locals
                .iter()
                .position(|&i| data.head_branch.as_deref() == Some(data.refs[i].name.as_str()))
                .map(|p| locals.remove(p));
            let count = locals.len() + head.is_some() as usize;
            if ds::section_header(
                ui,
                pal,
                id.with("sec-local"),
                "Local",
                Some(count),
                true,
                filtering,
            ) {
                if let Some(h) = head {
                    ref_leaf(ui, &side, &data.refs[h], &data.refs[h].name, 1, acts);
                }
                ref_tree(ui, &side, "local", &locals, |r| r.name.as_str(), 1, acts);
            }

            let remotes = idx_of(RefKind::Remote);
            if ds::section_header(
                ui,
                pal,
                id.with("sec-remote"),
                "Remote",
                Some(remotes.len()),
                true,
                filtering,
            ) {
                for remote in &data.remotes {
                    let mine: Vec<usize> = remotes
                        .iter()
                        .copied()
                        .filter(|&i| data.refs[i].remote.as_deref() == Some(remote))
                        .collect();
                    if folder_row(
                        ui,
                        pal,
                        id.with(("remote", remote)),
                        icons::CLOUD,
                        remote,
                        Some(mine.len()),
                        1,
                        true,
                        filtering,
                    ) {
                        ref_tree(ui, &side, remote, &mine, |r| r.branch_name(), 2, acts);
                    }
                }
            }

            let trees: Vec<&git::Worktree> = data
                .worktrees
                .iter()
                .filter(|w| {
                    filter.is_empty()
                        || w.name().to_lowercase().contains(&filter)
                        || w.branch
                            .as_ref()
                            .is_some_and(|b| b.to_lowercase().contains(&filter))
                })
                .collect();
            if !data.worktrees.is_empty()
                && ds::section_header(
                    ui,
                    pal,
                    id.with("sec-worktrees"),
                    "Worktrees",
                    Some(trees.len()),
                    true,
                    filtering,
                )
            {
                for w in trees {
                    worktree_row(ui, pal, w, acts);
                }
            }

            let mut tags = idx_of(RefKind::Tag);
            tags.reverse();
            if ds::section_header(
                ui,
                pal,
                id.with("sec-tags"),
                "Tags",
                Some(tags.len()),
                false,
                filtering,
            ) {
                for &i in &tags {
                    ref_leaf(ui, &side, &data.refs[i], &data.refs[i].name, 1, acts);
                }
            }

            if ds::section_header(
                ui,
                pal,
                id.with("sec-stashes"),
                "Stashes",
                Some(data.stashes.len()),
                false,
                false,
            ) {
                for s in &data.stashes {
                    stash_row(ui, pal, s, acts);
                }
            }
        });
}

/// A worktree: house for the main one, tree for the others; its branch, then its directory.
/// The current one is selected. Click shows its commit, double-click switches to it.
fn worktree_row(ui: &mut Ui, pal: &Palette, w: &git::Worktree, acts: &mut Vec<Action>) {
    let label = match (&w.branch, &w.head) {
        (Some(b), _) => b.clone(),
        (None, Some(h)) => format!("detached at {}", &h[..h.len().min(7)]),
        (None, None) => "detached".into(),
    };
    let dir = w.name();
    let resp = ds::row(
        ui,
        pal,
        ROW_H,
        w.current,
        &format!("Worktree {dir} ({label})"),
    );
    let r = resp.rect;
    let yc = r.center().y;
    let p = ui.painter();
    let x = r.left() + 6.0 + 14.0;
    let icon = if w.main {
        icons::HOUSE
    } else {
        icons::TREE_PINE
    };
    let ic = if w.current {
        pal.accent
    } else {
        pal.text_subtle
    };
    ds::paint_icon(p, pos2(x + 7.0, yc), icon, 14.0, ic);
    let mut right = r.right() - 6.0;
    if w.locked {
        ds::paint_icon(p, pos2(right - 7.0, yc), icons::LOCK, 13.0, pal.text_muted);
        right -= 20.0;
    }
    let tail = p.layout_job(ds::text_job(&dir, 11.5, 400.0, pal.text_subtle, false));
    let right = right - tail.size().x;
    let weight = if w.current { 550.0 } else { 400.0 };
    let g = ds::galley(p, &label, 13.0, weight, pal.text, right - 8.0 - (x + 20.0));
    let lw = g.size().x;
    p.galley(pos2(x + 20.0, yc - g.size().y / 2.0), g, pal.text);
    // The directory only when it says more than the branch name.
    if dir != label && x + 20.0 + lw + 8.0 < right {
        p.galley(pos2(right, yc - tail.size().y / 2.0), tail, pal.text_subtle);
    }
    let what = if w.current {
        "the worktree you are in".to_string()
    } else {
        "Double-click to switch to it".to_string()
    };
    let resp = resp.on_hover_text(format!("{}\n{what}", w.path.display()));
    if resp.double_clicked() && !w.current {
        acts.push(Action::SwitchWorktree(w.path.clone()));
    } else if resp.clicked()
        && let Some(h) = &w.head
    {
        acts.push(Action::Jump(h.clone()));
    }
    resp.context_menu(|ui| worktree_menu(ui, pal, w, &dir, acts));
}

/// Open here or in a new tab, remove (with its branch), lock, copy the path. The main and the
/// current worktree can't be removed; a locked one must be unlocked first.
fn worktree_menu(ui: &mut Ui, pal: &Palette, w: &git::Worktree, dir: &str, acts: &mut Vec<Action>) {
    ui.set_width(300.0);
    ds::menu_title(ui, pal, dir, None);
    let mut item = |ui: &mut Ui, icon, text: &str, a: Action| {
        if ds::menu_item(ui, pal, icon, text, None, false).clicked() {
            acts.push(a);
            ui.close();
        }
    };
    if !w.current {
        item(
            ui,
            icons::TREE_PINE,
            "Open this worktree",
            Action::SwitchWorktree(w.path.clone()),
        );
        item(
            ui,
            icons::PLUS,
            "Open worktree in new tab",
            Action::OpenRepo(w.path.clone()),
        );
        ds::menu_divider(ui, pal);
    }
    let path = w.path.display().to_string();
    let why_not = if w.main {
        Some("Remove (main worktree)")
    } else if w.current {
        Some("Remove (open another worktree first)")
    } else if w.locked {
        Some("Remove (unlock it first)")
    } else {
        None
    };
    match why_not {
        Some(text) => ds::menu_item_disabled(ui, pal, icons::TRASH_2, text),
        None => {
            let remove = |branch: Option<String>| Action::Confirm {
                title: match &branch {
                    Some(b) => format!("Remove worktree {dir} and delete {b}?"),
                    None => format!("Remove worktree {dir}?"),
                },
                body: format!(
                    "This deletes the directory {path}, including ignored files such as \
                     dependencies. Git refuses when it has uncommitted changes.{}",
                    if branch.is_some() {
                        " The branch is only deleted when it is merged."
                    } else {
                        ""
                    }
                ),
                op: Op::RemoveWorktree {
                    path: path.clone(),
                    branch,
                },
                danger: true,
            };
            if ds::menu_item(ui, pal, icons::TRASH_2, "Remove this worktree", None, true).clicked()
            {
                acts.push(remove(None));
                ui.close();
            }
            if let Some(b) = &w.branch
                && ds::menu_item(
                    ui,
                    pal,
                    icons::TRASH_2,
                    "Remove worktree and delete branch",
                    Some(&format!("Remove worktree {dir} and delete branch {b}")),
                    true,
                )
                .clicked()
            {
                acts.push(remove(Some(b.clone())));
                ui.close();
            }
        }
    }
    ds::menu_divider(ui, pal);
    if !w.main {
        let (icon, text) = if w.locked {
            (icons::LOCK_OPEN, "Unlock this worktree")
        } else {
            (icons::LOCK, "Lock this worktree")
        };
        if ds::menu_item(ui, pal, icon, text, None, false).clicked() {
            acts.push(Action::Op(Op::LockWorktree {
                path: path.clone(),
                lock: !w.locked,
            }));
            ui.close();
        }
    }
    if ds::menu_item(ui, pal, icons::COPY, "Copy path", None, false).clicked() {
        acts.push(Action::Copy(path));
        ui.close();
    }
}

fn stash_row(ui: &mut Ui, pal: &Palette, s: &git::Stash, acts: &mut Vec<Action>) {
    let resp = ds::row(ui, pal, ROW_H, false, &format!("{} {}", s.name, s.message));
    let r = resp.rect;
    let p = ui.painter();
    let x = r.left() + 6.0 + 14.0;
    ds::paint_icon(
        p,
        pos2(x + 7.0, r.center().y),
        icons::ARCHIVE,
        14.0,
        pal.text_subtle,
    );
    let tag = p.layout_job(ds::text_job(&s.name, 11.5, 400.0, pal.text_subtle, true));
    let tw = tag.size().x;
    p.galley(
        pos2(x + 20.0, r.center().y - tag.size().y / 2.0),
        tag,
        pal.text_subtle,
    );
    let g = ds::galley(
        p,
        &s.message,
        13.0,
        400.0,
        pal.text,
        r.right() - (x + 26.0 + tw) - 6.0,
    );
    p.galley(
        pos2(x + 26.0 + tw, r.center().y - g.size().y / 2.0),
        g,
        pal.text,
    );
    resp.context_menu(|ui| {
        ui.set_width(260.0);
        ds::menu_title(ui, pal, &s.name, None);
        if ds::menu_item(ui, pal, icons::ARCHIVE_RESTORE, "Apply", None, false).clicked() {
            acts.push(Action::Op(Op::StashApply(s.name.clone())));
            ui.close();
        }
        if ds::menu_item(ui, pal, icons::ARCHIVE_RESTORE, "Pop", None, false).clicked() {
            acts.push(Action::Op(Op::StashPop(Some(s.name.clone()))));
            ui.close();
        }
        ds::menu_divider(ui, pal);
        if ds::menu_item(ui, pal, icons::TRASH_2, "Drop", None, true).clicked() {
            acts.push(Action::Confirm {
                title: format!("Drop {}?", s.name),
                body: format!("The stash \"{}\" will be deleted.", s.message),
                op: Op::StashDrop(s.name.clone()),
                danger: true,
            });
            ui.close();
        }
    });
}

/// A folder (branch prefix or remote) that opens and closes; returns whether it is open.
#[allow(clippy::too_many_arguments)]
fn folder_row(
    ui: &mut Ui,
    pal: &Palette,
    id: egui::Id,
    icon: &'static str,
    name: &str,
    count: Option<usize>,
    depth: usize,
    default_open: bool,
    force_open: bool,
) -> bool {
    let mut open = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(id))
        .unwrap_or(default_open);
    let resp = ds::row(ui, pal, ROW_H, false, name);
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    let open = open || force_open;
    let r = resp.rect;
    let p = ui.painter();
    let yc = r.center().y;
    let mut x = r.left() + 6.0 + (depth - 1) as f32 * 14.0;
    ds::paint_icon(
        p,
        pos2(x + 7.0, yc),
        if open {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        },
        14.0,
        pal.text_subtle,
    );
    x += 20.0;
    ds::paint_icon(p, pos2(x + 7.0, yc), icon, 14.0, pal.text_subtle);
    x += 20.0;
    let g = ds::galley(p, name, 13.0, 400.0, pal.text, r.right() - x - 40.0);
    p.galley(pos2(x, yc - g.size().y / 2.0), g, pal.text);
    if let Some(n) = count {
        let g = p.layout_job(ds::text_job(
            &n.to_string(),
            11.0,
            600.0,
            pal.text_muted,
            true,
        ));
        let pill = Rect::from_min_size(
            pos2(r.right() - 6.0 - g.size().x - 12.0, yc - 9.0),
            vec2(g.size().x + 12.0, 18.0),
        );
        p.rect_filled(pill, CornerRadius::same(9), pal.control);
        p.galley(pill.center() - g.size() / 2.0, g, pal.text_muted);
    }
    open
}

fn ref_tree(
    ui: &mut Ui,
    side: &Side,
    salt: &str,
    idxs: &[usize],
    name_of: impl Fn(&RefInfo) -> &str,
    depth: usize,
    acts: &mut Vec<Action>,
) {
    let data = side.data;
    if !side.tree {
        for &i in idxs {
            ref_leaf(ui, side, &data.refs[i], name_of(&data.refs[i]), depth, acts);
        }
        return;
    }
    let mut tree = Tree::default();
    for &i in idxs {
        let parts: Vec<&str> = name_of(&data.refs[i]).split('/').collect();
        tree.insert(&parts, i);
    }
    fn walk(
        ui: &mut Ui,
        t: &Tree,
        path: String,
        side: &Side,
        depth: usize,
        acts: &mut Vec<Action>,
    ) {
        for (name, child) in &t.children {
            let p = format!("{path}/{name}");
            let id = ui.scope_id().with(("folder", &p));
            if folder_row(
                ui,
                side.pal,
                id,
                icons::FOLDER,
                name,
                None,
                depth,
                false,
                false,
            ) {
                walk(ui, child, p.clone(), side, depth + 1, acts);
            }
        }
        for &i in &t.leaves {
            let r = &side.data.refs[i];
            let leaf = r.name.rsplit('/').next().unwrap_or(&r.name);
            ref_leaf(ui, side, r, leaf, depth, acts);
        }
    }
    walk(ui, &tree, salt.to_string(), side, depth, acts);
}

fn ref_leaf(
    ui: &mut Ui,
    side: &Side,
    r: &RefInfo,
    label: &str,
    depth: usize,
    acts: &mut Vec<Action>,
) {
    let (data, pal) = (side.data, side.pal);
    let is_current =
        r.kind == RefKind::Local && data.head_branch.as_deref() == Some(r.name.as_str());
    let selected = is_current || side.selected == Some(r.target.as_str());
    let resp = ds::row(ui, pal, ROW_H, selected, label);
    let rect = resp.rect;
    let p = ui.painter();
    let yc = rect.center().y;
    // Leaves line up with the icons of folders at the same depth.
    let mut x = rect.left() + 6.0 + (depth - 1) as f32 * 14.0 + 20.0;
    if is_current {
        p.circle_filled(pos2(x + 7.0, yc), 3.5, pal.accent);
    } else {
        let icon = match r.kind {
            RefKind::Tag => icons::TAG,
            _ => icons::GIT_BRANCH,
        };
        ds::paint_icon(p, pos2(x + 7.0, yc), icon, 14.0, pal.text_subtle);
    }
    x += 20.0;

    // Trailing: ahead/behind or "gone", then the default-branch star.
    let mut right = rect.right() - 6.0;
    if data.is_default_branch(r) {
        ds::paint_icon(p, pos2(right - 7.0, yc), icons::STAR, 14.0, pal.accent);
        right -= 20.0;
    }
    let worktree = (r.kind == RefKind::Local)
        .then(|| data.worktree_of(&r.name))
        .flatten();
    if worktree.is_some() {
        ds::paint_icon(
            p,
            pos2(right - 7.0, yc),
            icons::TREE_PINE,
            13.0,
            pal.text_muted,
        );
        right -= 20.0;
    }
    if r.kind == RefKind::Local {
        let trail = if r.upstream_gone {
            Some(("gone".to_string(), pal.danger))
        } else {
            let mut s = Vec::new();
            if r.ahead > 0 {
                s.push(format!("↑{}", r.ahead));
            }
            if r.behind > 0 {
                s.push(format!("↓{}", r.behind));
            }
            (!s.is_empty()).then(|| (s.join(" "), pal.text_muted))
        };
        if let Some((t, c)) = trail {
            let g = p.layout_job(ds::text_job(&t, 11.5, 500.0, c, true));
            right -= g.size().x;
            p.galley(pos2(right, yc - g.size().y / 2.0), g, c);
            right -= 8.0;
        }
    }
    let weight = if is_current { 550.0 } else { 400.0 };
    let g = ds::galley(p, label, 13.0, weight, pal.text, right - x);
    p.galley(pos2(x, yc - g.size().y / 2.0), g, pal.text);

    let tracks = r
        .upstream
        .as_ref()
        .map(|up| format!("\ntracks {up}"))
        .unwrap_or_default();
    let double = match worktree {
        Some(path) => format!(
            "\nchecked out in worktree {}\nDouble-click to switch to it",
            path.display()
        ),
        None => "\nDouble-click to check out".into(),
    };
    let resp = resp.on_hover_text(format!(
        "{}{tracks}{double}, right-click for actions",
        r.name
    ));
    if resp.double_clicked() {
        acts.push(checkout_action(r, data));
    } else if resp.clicked() {
        acts.push(Action::Jump(r.target.clone()));
    }
    let lane = side
        .coloring
        .branch(r.branch_name(), pal)
        .unwrap_or(pal.accent);
    resp.context_menu(|ui| ref_menu(ui, pal, r, data, lane, acts));
}

pub(crate) fn checkout_action(r: &RefInfo, data: &RepoData) -> Action {
    match r.kind {
        RefKind::Local => checkout_local(&r.name, data),
        RefKind::Remote => {
            let local = r.branch_name().to_string();
            if data.local(&local).is_some() {
                checkout_local(&local, data)
            } else {
                Action::Op(Op::CheckoutRemote {
                    remote_ref: r.name.clone(),
                    local,
                })
            }
        }
        RefKind::Tag => Action::Op(Op::CheckoutCommit(r.name.clone())),
    }
}

/// Checks out a local branch, or switches to the worktree that already has it checked out (git
/// refuses to check out a branch in two worktrees).
fn checkout_local(name: &str, data: &RepoData) -> Action {
    match data.worktree_of(name) {
        Some(path) => Action::SwitchWorktree(path.to_path_buf()),
        None => Action::Op(Op::Checkout(name.to_string())),
    }
}

/// The menu item that checks out local branch `name`, or switches to its worktree.
fn checkout_item(ui: &mut Ui, pal: &Palette, name: &str, data: &RepoData, acts: &mut Vec<Action>) {
    let (icon, text, a11y) = match data.worktree_of(name) {
        Some(path) => (
            icons::TREE_PINE,
            format!("Switch to worktree {}", dir_name(path)),
            Some(format!("Switch to worktree {}", path.display())),
        ),
        None => (icons::GIT_BRANCH, format!("Check out {name}"), None),
    };
    if ds::menu_item(ui, pal, icon, &text, a11y.as_deref(), false).clicked() {
        acts.push(checkout_local(name, data));
        ui.close();
    }
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// ---------------------------------------------------------------- state banner

/// Merge, rebase, cherry-pick or revert in progress: icon, bold lead word, one sentence, then
/// the way out (quiet) before the way forward (primary).
pub fn state_banner(ui: &mut Ui, tab: &RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some(data) = &tab.data else { return };
    let conflicts = data.status.conflicted.len();
    let (icon, lead) = match data.state {
        RepoState::Merging => (icons::GIT_MERGE, "Merge in progress"),
        RepoState::Rebasing => (icons::GIT_COMPARE_ARROWS, "Rebase in progress"),
        RepoState::CherryPicking => (icons::CHERRY, "Cherry-pick in progress"),
        RepoState::Reverting => (icons::UNDO_2, "Revert in progress"),
        RepoState::Clean => return,
    };
    let sentence = match conflicts {
        0 => "No conflicts left.".to_string(),
        1 => "1 conflicted file — resolve it in the panel on the right.".to_string(),
        n => format!("{n} conflicted files — resolve them in the panel on the right."),
    };
    ui.set_min_height(28.0);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(icon).size(16.0).color(pal.warning));
        ui.label(
            RichText::new(lead)
                .variation("wght", 600.0)
                .color(pal.warning),
        );
        ui.label(RichText::new(sentence).color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (out, forward, cont, abort) = match data.state {
                RepoState::Merging => (
                    "Abort merge",
                    "Commit merge",
                    Op::ContinueMerge,
                    Op::AbortMerge,
                ),
                RepoState::Rebasing => (
                    "Abort rebase",
                    "Continue rebase",
                    Op::ContinueRebase,
                    Op::AbortRebase,
                ),
                _ => ("Abort", "Commit", Op::ContinueMerge, Op::AbortCherryPick),
            };
            // Stays primary while conflicts remain, so the next step is visible.
            if ds::Button::new(forward)
                .icon(icons::GIT_COMMIT_HORIZONTAL)
                .primary()
                .enabled(conflicts == 0)
                .show(ui, pal)
                .clicked()
            {
                acts.push(Action::Op(cont));
            }
            if ds::Button::new(out).quiet().show(ui, pal).clicked() {
                acts.push(Action::Op(abort));
            }
        });
    });
}

/// The alert bar under the toolbar: the question, then Cancel and the action at the right.
/// `Some(true)` confirms, `Some(false)` cancels. With `keys`, Escape cancels and Enter
/// confirms an action that is not destructive.
pub fn alert_bar(
    ui: &mut Ui,
    alert: &crate::app::Alert,
    pal: &Palette,
    keys: bool,
) -> Option<bool> {
    let mut answer = None;
    let tone = if alert.danger { pal.danger } else { pal.accent };
    ui.set_min_height(30.0);
    // Question and buttons as one centred group. Its width is known after drawing, so the
    // last frame's width places it; the message is cut short when the bar is too narrow.
    let full = ui.available_width();
    // Per question, so another alert (or tab) never starts from this one's widths.
    let id = ui
        .scope_id()
        .with(("alert-group", &alert.title, &alert.body));
    let stored = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(id));
    let (group_w, body_w) = stored.unwrap_or((full, 0.0));
    let fixed = group_w - body_w;
    ui.horizontal_centered(|ui| {
        if stored.is_none() {
            // Measured this frame, shown from the next one.
            ui.set_invisible();
        }
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(((full - group_w) / 2.0).max(0.0));
        let start = ui.cursor().min.x;
        ui.label(
            RichText::new(if alert.danger {
                icons::TRIANGLE_ALERT
            } else {
                icons::CIRCLE_ALERT
            })
            .size(16.0)
            .color(tone),
        );
        ui.label(
            RichText::new(alert.title.as_str())
                .variation("wght", 600.0)
                .color(pal.text),
        );
        let body = ui
            .scope(|ui| {
                ui.set_max_width(if stored.is_some() {
                    (full - fixed).max(80.0)
                } else {
                    full
                });
                ui.add(
                    egui::Label::new(RichText::new(alert.body.as_str()).color(pal.text_muted))
                        .truncate(),
                )
            })
            .inner;
        ui.add_space(16.0);
        if ds::Button::new("Cancel").quiet().show(ui, pal).clicked() {
            answer = Some(false);
        }
        // Destructive actions are outlined in red, never primary.
        let kind = if alert.danger {
            ds::Kind::Danger
        } else {
            ds::Kind::Primary
        };
        if ds::Button::new(alert.op.label())
            .kind(kind)
            .show(ui, pal)
            .clicked()
        {
            answer = Some(true);
        }
        let measured = (ui.min_rect().right() - start, body.rect.width());
        if (measured.0 - group_w).abs() > 0.5 || (measured.1 - body_w).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(id, measured));
            ui.ctx().request_repaint();
        }
    });
    if keys && answer.is_none() {
        // Consumed, so the same Escape does not also close the diff view.
        let (esc, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if esc {
            answer = Some(false);
        } else if enter && !alert.danger {
            answer = Some(true);
        }
    }
    answer
}

// ---------------------------------------------------------------- graph

struct Cols {
    refs: (f32, f32),
    graph: (f32, f32),
    msg: (f32, f32),
    author: (f32, f32),
    date: (f32, f32),
}

/// User-set widths of the resizable columns; the message takes the rest.
#[derive(Clone, Copy)]
struct ColWidths {
    refs: f32,
    /// `None`: as wide as the lanes need.
    graph: Option<f32>,
    author: f32,
    date: f32,
}

/// Width the lanes of `lanes` need.
fn graph_fit(lanes: usize) -> f32 {
    (lanes.max(1) as f32 * LANE_W + GRAPH_PAD * 2.0).clamp(60.0, 360.0)
}

/// `bb-commits` grid: refs · graph · message · author · date, 12 px padding and gaps.
fn columns(left: f32, width: f32, w: ColWidths, lanes: usize) -> Cols {
    let (pad, gap) = (12.0, 12.0);
    let graph_w = w
        .graph
        .unwrap_or_else(|| graph_fit(lanes))
        .clamp(40.0, 600.0);
    let r0 = left + pad;
    let g0 = r0 + w.refs + gap;
    let m0 = g0 + graph_w + gap;
    let d1 = left + width - pad;
    let d0 = (d1 - w.date).max(m0 + 100.0);
    // The message keeps at least 100 px; the author column gives way first.
    let author_w = if width > 820.0 {
        w.author.min(d0 - gap - (m0 + 100.0 + gap)).max(0.0)
    } else {
        0.0
    };
    let a0 = d0 - gap - author_w;
    Cols {
        refs: (r0, r0 + w.refs),
        graph: (g0, g0 + graph_w),
        msg: (m0, if author_w > 0.0 { a0 - gap } else { d0 - gap }),
        author: (a0, a0 + author_w),
        date: (d0, d1),
    }
}

/// Labels for one commit, merging a local branch with its same-named remote branches.
struct Pill {
    /// Branch name without remote prefix, or the tag name.
    text: String,
    /// The remotes this chip stands for, when the branch is elsewhere on another remote.
    prefix: Option<String>,
    refs: Vec<usize>,
    kind: ChipKind,
    remotes: Vec<String>,
}

impl Pill {
    /// Full ref names, for tooltips and the accessible name.
    fn full_names(&self, data: &RepoData) -> String {
        self.refs
            .iter()
            .map(|&i| data.refs[i].name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn pills_for(idxs: &[usize], data: &RepoData) -> Vec<Pill> {
    let mut pills: Vec<Pill> = Vec::new();
    let mut order: Vec<usize> = idxs.to_vec();
    let rank = |i: &usize| {
        let r = &data.refs[*i];
        let cur = r.kind == RefKind::Local && data.head_branch.as_deref() == Some(r.name.as_str());
        (
            if cur { 0 } else { 1 },
            match r.kind {
                RefKind::Local => 0,
                RefKind::Remote => 1,
                RefKind::Tag => 2,
            },
        )
    };
    order.sort_by_key(rank);
    for i in order {
        let r = &data.refs[i];
        // A branch and its copies on the remotes at this commit: one chip, named without the
        // remote prefix; the cloud icon says it is on a remote.
        if r.kind != RefKind::Tag
            && let Some(p) = pills
                .iter_mut()
                .find(|p| !p.kind.tag && p.text == r.branch_name())
        {
            match r.kind {
                RefKind::Local => {
                    p.kind.local = true;
                    p.kind.worktree |= data.worktree_of(&r.name).is_some();
                }
                _ => {
                    p.kind.remote = true;
                    p.remotes.extend(r.remote.clone());
                }
            }
            p.kind.default |= data.is_default_branch(r);
            p.refs.push(i);
            continue;
        }
        pills.push(Pill {
            text: match r.kind {
                RefKind::Tag => r.name.clone(),
                _ => r.branch_name().to_string(),
            },
            prefix: None,
            refs: vec![i],
            kind: chip_kind(r, data),
            remotes: r.remote.clone().into_iter().collect(),
        });
    }
    // Name the remote only when the same branch sits elsewhere on another remote
    // (origin/develop here, azure/develop further down); otherwise the cloud icon says enough.
    for p in &mut pills {
        let elsewhere = !p.remotes.is_empty()
            && data.refs.iter().enumerate().any(|(i, r)| {
                r.kind == RefKind::Remote && r.branch_name() == p.text && !p.refs.contains(&i)
            });
        if elsewhere {
            p.prefix = Some(p.remotes.join(" · "));
        }
    }
    pills
}

/// The colour a commit row shows: canvas, with the selection or hover tint on top.
/// A find query (lower case) matches the subject anywhere or the hash from its start.
fn commit_matches(c: &crate::git::Commit, query: &str) -> bool {
    c.sha.starts_with(query) || c.subject.to_lowercase().contains(query)
}

/// The find bar, floating at the top right of the graph: field, "n of m", previous/next and
/// close. Enter goes to the next match, Shift+Enter to the previous; typing jumps to the first
/// match from the selection on. True when it should close.
#[allow(clippy::too_many_arguments)]
fn find_bar(
    ui: &mut Ui,
    full: Rect,
    find: &mut Option<Find>,
    hits: &[usize],
    sel_row: Option<usize>,
    data: &RepoData,
    row_of: &std::collections::HashMap<String, usize>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) -> bool {
    let Some(f) = find.as_mut() else { return false };
    let mut close = false;
    // From the selected row: the next match below it, or the previous one above it (wrapping).
    let step = |forward: bool| -> Option<usize> {
        let (first, last) = (*hits.first()?, *hits.last()?);
        Some(if forward {
            hits.iter()
                .copied()
                .find(|&h| sel_row.is_none_or(|r| h > r))
                .unwrap_or(first)
        } else {
            hits.iter()
                .rev()
                .copied()
                .find(|&h| sel_row.is_some_and(|r| h < r))
                .unwrap_or(last)
        })
    };
    let go = |row: Option<usize>, acts: &mut Vec<Action>| {
        if let Some(r) = row {
            acts.push(Action::Jump(data.commits[r].sha.clone()));
        }
    };
    if std::mem::take(&mut f.jump) && !hits.is_empty() {
        let from = hits
            .iter()
            .copied()
            .find(|&h| sel_row.is_none_or(|r| h >= r))
            .unwrap_or(hits[0]);
        go(Some(from), acts);
    }
    let width = 380.0;
    egui::Area::new(ui.scope_id().with("find-bar"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(full.right() - width - 12.0, full.top() + 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(pal.raised)
                .stroke(Stroke::new(1.0, pal.border_strong))
                .corner_radius(CornerRadius::same(crate::theme::RADIUS_MD))
                .inner_margin(4.0)
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let field = ui
                            .scope(|ui| {
                                ui.set_width(200.0);
                                ds::search_field(
                                    ui,
                                    pal,
                                    &mut f.query,
                                    "Find message or hash",
                                    None,
                                )
                            })
                            .inner;
                        if std::mem::take(&mut f.focus) {
                            field.request_focus();
                        }
                        let shift = ui.input(|i| i.modifiers.shift);
                        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            go(step(!shift), acts);
                            field.request_focus();
                        } else if field.changed() {
                            f.jump = true;
                            f.older = None;
                            let q = f.query.trim();
                            if q.chars().count() >= 2 {
                                acts.push(Action::FindHistory(q.to_string()));
                            }
                            ui.ctx().request_repaint();
                        }
                        if (field.has_focus() || field.lost_focus())
                            && ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            close = true;
                        }
                        let at = sel_row.and_then(|r| hits.iter().position(|&h| h == r));
                        let count = match (at, hits.len()) {
                            _ if f.query.trim().is_empty() => String::new(),
                            (_, 0) => "No results".into(),
                            (Some(i), n) => format!("{} of {n}", i + 1),
                            (None, n) => format!("{n} found"),
                        };
                        let g = ui.painter().layout_job(ds::text_job(
                            &count,
                            12.0,
                            400.0,
                            pal.text_muted,
                            false,
                        ));
                        let (r, resp) = ui.allocate_exact_size(vec2(76.0, 24.0), Sense::hover());
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Label, true, count.clone())
                        });
                        ui.painter().galley(
                            pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0),
                            g,
                            pal.text_muted,
                        );
                        let any = !hits.is_empty();
                        let btn = |icon, label: &str| {
                            ds::Button::icon_only(icon, label)
                                .quiet()
                                .small()
                                .enabled(any)
                        };
                        if btn(icons::CHEVRON_UP, "Previous match (Shift+Enter)")
                            .show(ui, pal)
                            .clicked()
                        {
                            go(step(false), acts);
                        }
                        if btn(icons::CHEVRON_DOWN, "Next match (Enter)")
                            .show(ui, pal)
                            .clicked()
                        {
                            go(step(true), acts);
                        }
                        if ds::Button::icon_only(icons::X, "Close find (Esc)")
                            .quiet()
                            .small()
                            .show(ui, pal)
                            .clicked()
                        {
                            close = true;
                        }
                    });
                    // Matches in commits that are not loaded yet: load until the first one.
                    let older: Vec<&String> = f
                        .older
                        .as_ref()
                        .filter(|(q, _)| q.to_lowercase() == f.query.trim().to_lowercase())
                        .map(|(_, shas)| shas.iter().filter(|s| !row_of.contains_key(*s)).collect())
                        .unwrap_or_default();
                    if let Some(first) = older.first() {
                        ui.horizontal(|ui| {
                            let text = match older.len() {
                                1 => "1 more in older history".to_string(),
                                n if n >= 500 => format!("{n}+ more in older history"),
                                n => format!("{n} more in older history"),
                            };
                            let g = ui.painter().layout_job(ds::text_job(
                                &text,
                                12.0,
                                400.0,
                                pal.text_muted,
                                false,
                            ));
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(width - 120.0, 24.0), Sense::hover());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, text.clone())
                            });
                            ui.painter().galley(
                                pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0),
                                g,
                                pal.text_muted,
                            );
                            if ds::Button::new("Show older match")
                                .small()
                                .show(ui, pal)
                                .clicked()
                            {
                                acts.push(Action::LoadUntil((*first).clone()));
                            }
                        });
                    }
                });
        });
    close
}

fn row_background(pal: &Palette, selected: bool, hovered: bool) -> Color32 {
    let tint = |c: Color32| crate::theme::mix(pal.canvas, c, c.a() as f32 / 255.0);
    if selected {
        tint(pal.selected)
    } else if hovered {
        tint(pal.hover)
    } else {
        pal.canvas
    }
}

/// A 20 px gradient from transparent to `bg` ending at `x`: lanes that continue out of view.
fn fade_right(painter: &Painter, x: f32, y: egui::Rangef, bg: Color32) {
    let r = Rect::from_x_y_ranges((x - 20.0)..=x, y);
    let clear = Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 0);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), clear);
    mesh.colored_vertex(r.right_top(), bg);
    mesh.colored_vertex(r.right_bottom(), bg);
    mesh.colored_vertex(r.left_bottom(), clear);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Width the branch column needs for the chips of every commit, drawn as in the graph.
fn fit_refs(
    painter: &Painter,
    data: &RepoData,
    refs_at: &std::collections::HashMap<String, Vec<usize>>,
    pal: &Palette,
) -> f32 {
    refs_at
        .values()
        .map(|idxs| {
            let pills = pills_for(idxs, data);
            let shown = if pills.len() > 2 { 1 } else { pills.len() };
            let mut w: f32 = pills[..shown]
                .iter()
                .map(|p| {
                    ds::chip_layout_prefixed(
                        painter,
                        p.prefix.as_deref(),
                        &p.text,
                        p.kind,
                        pal,
                        1e6,
                    )
                    .1 + 4.0
                })
                .sum();
            if pills.len() > shown {
                let g = painter.layout_job(ds::text_job(
                    &format!("+{}", pills.len() - shown),
                    11.0,
                    600.0,
                    pal.text_muted,
                    true,
                ));
                w += g.size().x + 14.0;
            }
            w + 4.0
        })
        .fold(80.0, f32::max)
}

/// Centre of a lane in the graph column.
fn lane_x(cols: &Cols, lane: usize) -> f32 {
    cols.graph.0 + GRAPH_PAD + LANE_W * lane as f32 + LANE_W / 2.0
}

fn draw_seg(painter: &Painter, from: Pos2, to: Pos2, color: Color32, top_half: bool) {
    let stroke = Stroke::new(2.0, color);
    if (from.x - to.x).abs() < 0.5 {
        painter.line_segment([from, to], stroke);
        return;
    }
    // Top half: leave the lane vertically, enter the node horizontally.
    // Bottom half: leave the node horizontally, enter the lane vertically.
    let (c1, c2) = if top_half {
        (pos2(from.x, to.y), pos2(from.x, to.y))
    } else {
        (pos2(to.x, from.y), pos2(to.x, from.y))
    };
    painter.add(CubicBezierShape::from_points_stroke(
        [from, c1, c2, to],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn dashed_v(painter: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    let mut y = y0;
    while y < y1 {
        painter.line_segment(
            [pos2(x, y), pos2(x, (y + 3.0).min(y1))],
            Stroke::new(2.0, color),
        );
        y += 6.0;
    }
}

pub fn graph_view(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let has_wip = tab.has_wip_row();
    let RepoTab {
        data,
        graph,
        coloring,
        refs_at,
        selected,
        scroll_to,
        ref_col_w,
        graph_col_w,
        author_col_w,
        date_col_w,
        row_of,
        new_branch,
        find,
        limit,
        loading_more,
        ..
    } = tab;
    let Some(data) = data.as_ref() else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    let offset = has_wip as usize;
    let total = data.commits.len() + offset;
    // Follow the interface font size from Settings (13 by default).
    let ui_size = ui.style().text_styles[&egui::TextStyle::Body].size;

    // Keyboard navigation.
    if ui.ctx().memory(|m| m.focused().is_none()) && total > 0 {
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if up || down {
            let cur = match selected {
                Selection::Wip => Some(0),
                Selection::Commit(s) => row_of.get(s.as_str()).map(|r| r + offset),
                Selection::None => None,
            };
            let next = match cur {
                Some(c) if up => c.saturating_sub(1),
                Some(c) => (c + 1).min(total - 1),
                None => 0,
            };
            *scroll_to = Some(next);
            acts.push(Action::Select(if has_wip && next == 0 {
                Selection::Wip
            } else {
                Selection::Commit(data.commits[next - offset].sha.clone())
            }));
        }
    }

    // Find (Ctrl F): rows whose subject or hash match, top to bottom.
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
        find.get_or_insert_with(Find::default).focus = true;
    }
    let query = find
        .as_ref()
        .map(|f| f.query.trim().to_lowercase())
        .unwrap_or_default();
    let hits: Vec<usize> = if query.is_empty() {
        Vec::new()
    } else {
        (0..data.commits.len())
            .filter(|&i| commit_matches(&data.commits[i], &query))
            .collect()
    };

    // Header: captions over a hairline.
    let full = ui.available_rect_before_wrap();
    if find.is_some() {
        let sel_row = match &*selected {
            Selection::Commit(s) => row_of.get(s.as_str()).copied(),
            _ => None,
        };
        if find_bar(ui, full, find, &hits, sel_row, data, row_of, pal, acts) {
            *find = None;
        }
    }
    let widths = ColWidths {
        refs: *ref_col_w,
        graph: *graph_col_w,
        author: *author_col_w,
        date: *date_col_w,
    };
    let cols = columns(full.left(), full.width(), widths, graph.lanes);
    let (header, _) = ui.allocate_exact_size(vec2(full.width(), 30.0), Sense::hover());
    {
        let p = ui.painter();
        p.hline(
            header.x_range(),
            header.bottom() - 0.5,
            Stroke::new(1.0, pal.border),
        );
        let y = header.center().y;
        let cap = |p: &Painter, x: f32, t: &str, right: bool| {
            let g = egui::WidgetText::from(ds::caption(t, pal)).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );
            let x = if right { x - g.size().x } else { x };
            p.galley(pos2(x, y - g.size().y / 2.0), g, pal.text_subtle);
        };
        cap(p, cols.refs.0, "Branch / tag", false);
        cap(p, cols.graph.0, "Graph", false);
        cap(p, cols.msg.0, "Commit message", false);
        if cols.author.1 > cols.author.0 {
            cap(p, cols.author.0, "Author", false);
        }
        cap(p, cols.date.1, "Date", true);
    }
    // Handles between columns: drag to resize; double-click fits the column on the left to its
    // contents, like Excel (the message is the exception: it takes what the author leaves).
    let handle = |x: f32, id: &str| -> (f32, bool) {
        let rect = Rect::from_center_size(pos2(x, header.center().y), vec2(8.0, header.height()));
        let r = ui
            .interact(rect, ui.scope_id().with(id), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to resize, double-click to fit");
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, id));
        if r.hovered() || r.dragged() {
            ui.painter().vline(
                rect.center().x,
                header.y_range().shrink(6.0),
                Stroke::new(1.0, pal.border_strong),
            );
        }
        (
            if r.dragged() { r.drag_delta().x } else { 0.0 },
            r.double_clicked(),
        )
    };
    let gap = 12.0;
    let (dx, fit) = handle(cols.refs.1 + 6.0, "Resize branch column");
    *ref_col_w = if fit {
        fit_refs(ui.painter(), data, refs_at, pal)
    } else {
        *ref_col_w + dx
    }
    .clamp(80.0, 500.0);
    let (dx, fit) = handle(cols.graph.1 + 6.0, "Resize graph column");
    if fit {
        *graph_col_w = None;
    } else if dx != 0.0 {
        *graph_col_w = Some((cols.graph.1 - cols.graph.0 + dx).clamp(40.0, 600.0));
    }
    if cols.author.1 > cols.author.0 {
        // Dragging the message's right edge to the right makes the author column narrower.
        let (dx, fit) = handle(cols.author.0 - 6.0, "Resize message column");
        if fit {
            let longest = data
                .commits
                .iter()
                .map(|c| {
                    // As drawn: the ticket key a little bolder than the rest.
                    let key = ticket_key_len(&c.subject);
                    let mut job = ds::text_job(&c.subject[..key], ui_size, 550.0, pal.text, false);
                    let rest = ds::text_job(&c.subject[key..], ui_size, 400.0, pal.text, false);
                    job.append(&c.subject[key..], 0.0, rest.sections[0].format.clone());
                    ui.painter().layout_job(job).size().x
                })
                .fold(0.0, f32::max);
            *author_col_w = (cols.author.1 - (cols.msg.0 + longest + 8.0 + gap)).clamp(60.0, 400.0);
        } else if dx != 0.0 {
            *author_col_w = (cols.author.1 - cols.author.0 - dx).clamp(60.0, 400.0);
        }
        // Between author and date only those two change, so the message stays put.
        let (dx, fit) = handle(cols.date.0 - 6.0, "Resize author column");
        if fit {
            let longest = data
                .commits
                .iter()
                .map(|c| {
                    let g = ds::galley(ui.painter(), &c.author, ui_size, 400.0, pal.text, 1e6);
                    g.size().x
                })
                .fold(0.0, f32::max);
            *author_col_w = (longest + 24.0 + 6.0).clamp(60.0, 400.0);
        } else if dx != 0.0 {
            // Start from the drawn widths, which may differ from the stored ones on narrow tabs.
            let (author_now, date_now) = (cols.author.1 - cols.author.0, cols.date.1 - cols.date.0);
            let date = (date_now - dx).clamp(60.0, 240.0);
            *author_col_w = (author_now + (date_now - date)).clamp(60.0, 400.0);
            *date_col_w = date;
        }
    } else {
        // Narrow lists have no author column: this border sits between message and date.
        let (dx, fit) = handle(cols.date.0 - 6.0, "Resize date column");
        *date_col_w = if fit {
            data.commits
                .iter()
                .take(500)
                .map(|c| {
                    ds::galley(
                        ui.painter(),
                        &fmt_time(c.time),
                        ui_size,
                        400.0,
                        pal.text,
                        1e6,
                    )
                    .size()
                    .x
                })
                .fold(60.0, f32::max)
                + 4.0
        } else {
            *date_col_w - dx
        }
        .clamp(60.0, 240.0);
    }

    if total == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("No commits yet").color(pal.text_muted))
        });
        return;
    }

    // The commit went away (reset, rebase): so does the name field.
    if new_branch
        .as_ref()
        .is_some_and(|b| !row_of.contains_key(&b.at))
    {
        *new_branch = None;
    }
    let head_row = data
        .head_sha
        .as_ref()
        .and_then(|s| row_of.get(s.as_str()).copied());
    let mut area = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("graph");
    if let Some(target) = scroll_to.take() {
        let view_h = ui.available_height();
        let y = target as f32 * ROW_H;
        let current = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(ui.scope_id().with("graph-offset")))
            .unwrap_or(0.0);
        if y < current || y + ROW_H > current + view_h {
            area = area.vertical_scroll_offset((y - view_h / 3.0).max(0.0));
        }
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    // Near the end of what is loaded, while there may be more: load the next portion.
    let more = data.commits.len() >= limit.load(std::sync::atomic::Ordering::Relaxed);
    let out = area.show_rows(ui, ROW_H, total, |ui, range| {
        if more && !*loading_more && range.end + 20 >= total {
            *loading_more = true;
            acts.push(Action::LoadMore);
        }
        for row in range {
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            let painter = ui.painter_at(rect);
            let yc = rect.center().y;
            let is_wip = has_wip && row == 0;
            let is_sel = if is_wip {
                *selected == Selection::Wip
            } else {
                matches!(selected, Selection::Commit(s) if *s == data.commits[row - offset].sha)
            };
            if is_sel {
                painter.rect_filled(rect, 0.0, pal.selected);
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, pal.hover);
            }
            // Find matches: a faint tint and an accent bar at the left edge.
            if !is_wip && hits.binary_search(&(row - offset)).is_ok() {
                if !is_sel && !resp.hovered() {
                    painter.rect_filled(rect, 0.0, pal.accent_soft.gamma_multiply(0.5));
                }
                painter.rect_filled(
                    Rect::from_x_y_ranges(rect.left()..=rect.left() + 3.0, rect.y_range()),
                    0.0,
                    pal.accent,
                );
            }

            if is_wip {
                // Clipped to the graph column like the commit rows.
                let gpainter = painter.with_clip_rect(Rect::from_x_y_ranges(
                    cols.graph.0..=cols.graph.1,
                    rect.y_range(),
                ));
                let col = head_row.map(|r| graph.rows[r].col).unwrap_or(0);
                let x = lane_x(&cols, col);
                let color = head_row
                    .map(|r| coloring.color(graph.rows[r].color, pal))
                    .unwrap_or(pal.text_subtle);
                if head_row.is_some() {
                    dashed_v(&gpainter, x, yc + NODE_R, rect.bottom(), color);
                }
                gpainter.circle_filled(pos2(x, yc), NODE_R, pal.canvas);
                gpainter.add(egui::Shape::dashed_line(
                    &(0..=24)
                        .map(|k| {
                            let a = k as f32 / 24.0 * std::f32::consts::TAU;
                            pos2(x + NODE_R * a.cos(), yc + NODE_R * a.sin())
                        })
                        .collect::<Vec<_>>(),
                    Stroke::new(2.0, color),
                    3.0,
                    2.0,
                ));
                if graph_fit(graph.lanes) > cols.graph.1 - cols.graph.0 + 0.5 {
                    fade_right(
                        &painter,
                        cols.graph.1,
                        rect.y_range(),
                        row_background(pal, is_sel, resp.hovered()),
                    );
                }
                let st = &data.status;
                let files = |n: usize| {
                    if n == 1 {
                        "1 changed file".to_string()
                    } else {
                        format!("{n} changed files")
                    }
                };
                let label = match st.conflicted.len() {
                    0 => format!("// WIP · {}", files(st.count())),
                    c => format!(
                        "// WIP · {c} conflict{}, {}",
                        if c == 1 { "" } else { "s" },
                        files(st.count() - c)
                    ),
                };
                let mut job = ds::text_job(&label, ui_size, 400.0, pal.text_muted, false);
                job.sections[0].format.italics = true;
                let g = painter.layout_job(job);
                painter.galley(pos2(cols.msg.0, yc - g.size().y / 2.0), g, pal.text_muted);
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, "WIP")
                });
                if resp.clicked() {
                    acts.push(Action::Select(Selection::Wip));
                }
                continue;
            }

            let idx = row - offset;
            let c = &data.commits[idx];
            let g = &graph.rows[idx];
            let color = coloring.color(g.color, pal);

            // Graph lines.
            let gpainter = painter.with_clip_rect(Rect::from_x_y_ranges(
                cols.graph.0..=cols.graph.1,
                rect.y_range(),
            ));
            for s in &g.top {
                draw_seg(
                    &gpainter,
                    pos2(lane_x(&cols, s.from), rect.top()),
                    pos2(lane_x(&cols, s.to), yc),
                    coloring.color(s.color, pal),
                    true,
                );
            }
            for s in &g.bottom {
                draw_seg(
                    &gpainter,
                    pos2(lane_x(&cols, s.from), yc),
                    pos2(lane_x(&cols, s.to), rect.bottom()),
                    coloring.color(s.color, pal),
                    false,
                );
            }
            // Dashed line from the WIP row down to the checked out commit.
            if let (true, Some(hr)) = (has_wip, head_row)
                && idx <= hr
            {
                let hx = lane_x(&cols, graph.rows[hr].col);
                let hc = coloring.color(graph.rows[hr].color, pal);
                dashed_v(
                    &gpainter,
                    hx,
                    rect.top(),
                    if idx == hr {
                        yc - NODE_R - 4.0
                    } else {
                        rect.bottom()
                    },
                    hc,
                );
            }

            // Ref chips, left-aligned in their column: up to two, else one and "+N". A new
            // branch's name field takes their place.
            let node = pos2(lane_x(&cols, g.col), yc);
            let naming = new_branch.as_ref().is_some_and(|b| b.at == c.sha);
            if naming {
                let field =
                    Rect::from_x_y_ranges(cols.refs.0 + 4.0..=cols.refs.1, yc - 13.0..=yc + 13.0);
                if branch_name_field(ui, field, new_branch, data, pal, acts) {
                    *new_branch = None;
                }
                painter.line_segment(
                    [
                        pos2(cols.refs.1 + 2.0, yc),
                        pos2((node.x - NODE_R - 3.0).min(cols.graph.1), yc),
                    ],
                    Stroke::new(1.0, color.gamma_multiply(0.6)),
                );
            } else if let Some(idxs) = refs_at.get(&c.sha) {
                let pills = pills_for(idxs, data);
                let shown = if pills.len() > 2 { 1 } else { pills.len() };
                let more = pills.len() - shown;
                // Width of "+N" and its gap, kept free at the right.
                let more_w = if more > 0 {
                    painter
                        .layout_job(ds::text_job(
                            &format!("+{more}"),
                            11.0,
                            600.0,
                            pal.text_muted,
                            true,
                        ))
                        .size()
                        .x
                        + 14.0
                } else {
                    0.0
                };
                // Sized from the right so the last chip gets the room, then painted from the
                // column's left edge.
                let mut budget = cols.refs.1 - more_w;
                let mut chips: Vec<_> = pills[..shown]
                    .iter()
                    .rev()
                    .map(|pill| {
                        let avail = (budget - cols.refs.0).max(40.0);
                        let (gal, w) = ds::chip_layout_prefixed(
                            &painter,
                            pill.prefix.as_deref(),
                            &pill.text,
                            pill.kind,
                            pal,
                            avail,
                        );
                        budget -= w + 4.0;
                        (pill, gal, w)
                    })
                    .collect();
                chips.reverse();
                let mut x = cols.refs.0;
                for (pill, gal, w) in chips {
                    let lane = pill_lane(pill, data, coloring, pal, color);
                    let pr = ds::paint_chip(&painter, pos2(x, yc), gal, w, pill.kind, lane, pal);
                    x += w + 4.0;
                    let presp = ui.interact(
                        pr,
                        ui.scope_id()
                            .with(("pill", &c.sha, &pill.text, &pill.prefix)),
                        Sense::click(),
                    );
                    presp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, pill.full_names(data))
                    });
                    let tip: Vec<String> = pill
                        .refs
                        .iter()
                        .map(|&i| data.refs[i].name.clone())
                        .collect();
                    let note = if pill.kind.default {
                        "\ndefault branch of the remote"
                    } else {
                        ""
                    };
                    let presp = presp.on_hover_text(format!(
                        "{}{note}\nRight-click for actions",
                        tip.join("\n")
                    ));
                    if presp.double_clicked() {
                        acts.push(checkout_action(&data.refs[pill.refs[0]], data));
                    } else if presp.clicked() {
                        acts.push(Action::Select(Selection::Commit(c.sha.clone())));
                    }
                    presp.context_menu(|ui| pill_menu(ui, pal, pill, data, lane, acts));
                }
                if more > 0 {
                    let r = ds::paint_more(&painter, pos2(x, yc), more, pal);
                    x = r.right() + 4.0;
                    let names: Vec<String> =
                        pills[shown..].iter().map(|p| p.full_names(data)).collect();
                    let mresp = ui
                        .interact(r, ui.scope_id().with(("more", &c.sha)), Sense::click())
                        .on_hover_text(names.join("\n"));
                    mresp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("+{more}"))
                    });
                    mresp.context_menu(|ui| {
                        ui.set_width(300.0);
                        for p in &pills[shown..] {
                            let lane = pill_lane(p, data, coloring, pal, color);
                            ds::submenu(ui, pal, icons::GIT_BRANCH, &p.text, |ui| {
                                pill_menu(ui, pal, p, data, lane, acts)
                            });
                        }
                    });
                }
                // A thin connector from the chips to the dot, in the lane colour.
                painter.line_segment(
                    [
                        pos2(x - 2.0, yc),
                        pos2((node.x - NODE_R - 3.0).min(cols.graph.1), yc),
                    ],
                    Stroke::new(1.0, color.gamma_multiply(0.6)),
                );
            }

            // Node: filled dot, hollow for merges, an extra ring for HEAD. Clipped to the graph
            // column, which the user can make narrower than the lanes.
            let is_head = data.head_sha.as_deref() == Some(c.sha.as_str());
            if c.parents.len() > 1 {
                gpainter.circle_filled(node, NODE_R - 0.5, pal.canvas);
                gpainter.circle_stroke(node, NODE_R - 0.5, Stroke::new(2.0, color));
            } else {
                gpainter.circle_filled(node, NODE_R, color);
            }
            if is_head {
                gpainter.circle_stroke(node, 9.0, Stroke::new(1.5, pal.text));
            }
            // More lanes than fit: fade them out at the column's right edge.
            if graph_fit(graph.lanes) > cols.graph.1 - cols.graph.0 + 0.5 {
                fade_right(
                    &painter,
                    cols.graph.1,
                    rect.y_range(),
                    row_background(pal, is_sel, resp.hovered()),
                );
            }

            // Message: ticket key in 550; merges in `text-muted`.
            let merge = c.parents.len() > 1;
            let subject_color = if merge { pal.text_muted } else { pal.text };
            let key = if merge { 0 } else { ticket_key_len(&c.subject) };
            let mut job = LayoutJob::default();
            for (part, weight) in [(&c.subject[..key], 550.0), (&c.subject[key..], 400.0)] {
                if !part.is_empty() {
                    let s = ds::text_job(part, ui_size, weight, subject_color, false);
                    job.append(part, 0.0, s.sections[0].format.clone());
                }
            }
            job.wrap.max_width = (cols.msg.1 - cols.msg.0).max(1.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let msg = painter.layout_job(job);
            painter.galley(
                pos2(cols.msg.0, yc - msg.size().y / 2.0),
                msg,
                subject_color,
            );

            if cols.author.1 > cols.author.0 {
                ds::avatar(
                    &painter,
                    pos2(cols.author.0 + 9.0, yc),
                    &initials(&c.author),
                    color,
                );
                let a = ds::galley(
                    &painter,
                    &c.author,
                    ui_size,
                    400.0,
                    pal.text_muted,
                    cols.author.1 - cols.author.0 - 24.0,
                );
                painter.galley(
                    pos2(cols.author.0 + 24.0, yc - a.size().y / 2.0),
                    a,
                    pal.text_muted,
                );
            }
            // Elided to the column, which the user can make narrow.
            let date = ds::galley(
                &painter,
                &fmt_time(c.time),
                ui_size,
                400.0,
                pal.text_muted,
                cols.date.1 - cols.date.0,
            );
            painter.galley(
                pos2(cols.date.1 - date.size().x, yc - date.size().y / 2.0),
                date,
                pal.text_muted,
            );

            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, &c.subject)
            });
            if resp.clicked() {
                acts.push(Action::Select(Selection::Commit(c.sha.clone())));
            }
            resp.context_menu(|ui| commit_menu(ui, pal, &c.sha, &c.subject, data, acts));
        }
    });
    let off = out.state.clamped_offset().y;
    ui.ctx()
        .data_mut(|d| d.insert_temp(ui.scope_id().with("graph-offset"), off));
}

/// The name field for a new branch. Enter with a usable name creates the branch and checks it
/// out; Escape, Enter on an empty field or clicking elsewhere drops it. Returns true when the
/// field is done.
fn branch_name_field(
    ui: &mut Ui,
    rect: Rect,
    state: &mut Option<crate::app::NewBranch>,
    data: &RepoData,
    pal: &Palette,
    acts: &mut Vec<Action>,
) -> bool {
    let Some(b) = state.as_mut() else { return true };
    let name = b.name.trim().to_string();
    let problem = crate::app::new_ref_problem(Some(data), &name);
    let shown = problem.filter(|_| !name.is_empty());
    let resp = ui
        .scope(|ui| {
            if shown.is_some() {
                let w = &mut ui.visuals_mut().widgets;
                for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    s.bg_stroke = Stroke::new(1.0, pal.danger);
                }
                ui.visuals_mut().selection.stroke = Stroke::new(1.0, pal.danger);
            }
            ui.put(
                rect,
                egui::TextEdit::singleline(&mut b.name)
                    // A fixed id, so scrolling (which renumbers the rows) keeps the focus.
                    .id(egui::Id::unique(("new-branch", &b.at)))
                    .hint_text("enter branch name")
                    .margin(egui::Margin::symmetric(8, 4))
                    .align(egui::Align2::LEFT_CENTER),
            )
        })
        .inner;
    let resp = match shown {
        Some(p) => resp.on_hover_text(p),
        None => resp.on_hover_text("Enter creates the branch and checks it out, Escape cancels"),
    };
    if !b.focused {
        resp.request_focus();
        b.focused = true;
        return false;
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if resp.lost_focus() && enter {
        if problem.is_some() {
            // Keep the field open until the name works (an empty name stays allowed to cancel).
            if name.is_empty() {
                return true;
            }
            resp.request_focus();
            return false;
        }
        acts.push(Action::Op(Op::CreateBranch {
            name,
            at: b.at.clone(),
            checkout: true,
        }));
        return true;
    }
    !resp.has_focus()
}

/// A branch has its own colour; a tag follows the branch its commit is on.
fn pill_lane(
    pill: &Pill,
    data: &RepoData,
    coloring: &crate::colors::Coloring,
    pal: &Palette,
    commit_color: Color32,
) -> Color32 {
    let first = &data.refs[pill.refs[0]];
    match first.kind {
        RefKind::Tag => commit_color,
        _ => coloring
            .branch(first.branch_name(), pal)
            .unwrap_or(commit_color),
    }
}

fn pill_menu(
    ui: &mut Ui,
    pal: &Palette,
    pill: &Pill,
    data: &RepoData,
    lane: Color32,
    acts: &mut Vec<Action>,
) {
    let first = &data.refs[pill.refs[0]];
    ref_menu(ui, pal, first, data, lane, acts);
    for &i in &pill.refs[1..] {
        let r = &data.refs[i];
        ds::menu_divider(ui, pal);
        ds::submenu(ui, pal, icons::CLOUD, &r.name, |ui| {
            ref_menu(ui, pal, r, data, lane, acts)
        });
    }
}

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

/// Summary (72-character countdown) and description, Amend, and one full-width button that
/// says what is missing or what will happen.
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
                                .hint_text(RichText::new("Summary").color(pal.text_subtle))
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
            let has_summary = !tab.commit_msg.trim().is_empty();
            let label = if tab.amend {
                "Amend last commit".to_string()
            } else if staged == 0 {
                "Stage files to commit".to_string()
            } else if !has_summary {
                "Write a summary to commit".to_string()
            } else if staged == 1 {
                "Commit 1 file".to_string()
            } else {
                format!("Commit {staged} files")
            };
            let can_commit = has_summary && (staged > 0 || tab.amend) && !conflicts;
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

/// Summary and description as one message, separated by a blank line.
fn commit_message(summary: &str, body: &str) -> String {
    match body.trim() {
        "" => summary.trim().to_string(),
        body => format!("{}\n\n{body}", summary.trim()),
    }
}

// ---------------------------------------------------------------- diff view

/// One line of a unified diff with its old and new line numbers.
struct DiffLine<'a> {
    old: Option<usize>,
    new: Option<usize>,
    kind: char,
    text: &'a str,
}

/// The hunks of a unified diff; the file header lines before the first hunk are dropped.
fn parse_diff(text: &str) -> Vec<DiffLine<'_>> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0usize, 0usize);
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            in_hunk = true;
            // "@@ -12,7 +12,8 @@ fn x"
            let mut parts = rest.split_whitespace();
            let num = |s: Option<&str>| {
                s.and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            old = num(parts.next());
            new = num(parts.next());
            out.push(DiffLine {
                old: None,
                new: None,
                kind: '@',
                text: line,
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let kind = line
            .chars()
            .next()
            .filter(|c| matches!(c, '+' | '-' | '\\'))
            .unwrap_or(' ');
        let text = match kind {
            '\\' => line,
            _ => line.get(1..).unwrap_or(""),
        };
        let (o, n) = match kind {
            '+' => (None, Some(new)),
            '-' => (Some(old), None),
            '\\' => (None, None),
            _ => (Some(old), Some(new)),
        };
        match kind {
            '+' => new += 1,
            '-' => old += 1,
            '\\' => {}
            _ => {
                old += 1;
                new += 1;
            }
        }
        out.push(DiffLine {
            old: o,
            new: n,
            kind,
            text,
        });
    }
    out
}

pub fn diff_view(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some((target, content)) = &tab.diff else {
        return;
    };
    if let DiffTarget::Conflict { path } = target {
        let (path, content) = (path.clone(), content.clone());
        return conflict_view(ui, tab, &path, content, pal, acts);
    }
    let lines = match content {
        Some(Ok(t)) => parse_diff(t),
        _ => Vec::new(),
    };
    // Staging part of a file: in the working-copy and the staged diff of a tracked file.
    let raw = match content {
        Some(Ok(t)) => t.as_str(),
        _ => "",
    };
    let partial = matches!(
        target,
        DiffTarget::Unstaged {
            untracked: false,
            ..
        } | DiffTarget::Staged { .. }
    );
    let unstage = matches!(target, DiffTarget::Staged { .. });
    let verb = if unstage { "Unstage" } else { "Stage" };
    // Per parsed line: its hunk and its index in that hunk's body (as `git::partial_patch` counts).
    let mut hunk_pos = Vec::with_capacity(lines.len());
    let (mut h, mut i) = (usize::MAX, 0);
    for l in &lines {
        if l.kind == '@' {
            h = h.wrapping_add(1);
            i = 0;
            hunk_pos.push((h, usize::MAX));
        } else {
            hunk_pos.push((h, i));
            i += 1;
        }
    }
    let selection = &mut tab.diff_selection;
    let anchor = &mut tab.diff_anchor;
    let (adds, dels) = lines.iter().fold((0, 0), |(a, d), l| match l.kind {
        '+' => (a + 1, d),
        '-' => (a, d + 1),
        _ => (a, d),
    });
    // Status letter of the file, from the panel it was opened from.
    let status = match (target, &tab.details, &tab.data) {
        (DiffTarget::Commit { path, .. }, Some(d), _) => {
            d.files.iter().find(|f| &f.path == path).map(|f| f.status)
        }
        (
            DiffTarget::Unstaged {
                untracked: true, ..
            },
            _,
            _,
        ) => Some('A'),
        (DiffTarget::Unstaged { path, .. }, _, Some(data)) => data
            .status
            .unstaged
            .iter()
            .find(|f| &f.path == path)
            .map(|f| f.status),
        (DiffTarget::Staged { path }, _, Some(data)) => data
            .status
            .staged
            .iter()
            .find(|f| &f.path == path)
            .map(|f| f.status),
        _ => None,
    }
    .unwrap_or('M');

    // Header: status, path, where it comes from, +/− stat, close at the far right.
    egui::Frame::new()
        .fill(pal.app)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_height(36.0);
            ui.set_width(ui.available_width());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ds::status_letter(ui.painter(), r.center(), status, pal);
                ui.add(
                    egui::Label::new(
                        RichText::new(target.path())
                            .monospace()
                            .size(12.0)
                            .variation("wght", 500.0)
                            .color(pal.text),
                    )
                    .truncate(),
                );
                match target {
                    DiffTarget::Commit { sha, .. } => {
                        ui.label(
                            RichText::new(format!("in {}", git::short(sha)))
                                .size(12.5)
                                .color(pal.text_muted),
                        );
                    }
                    DiffTarget::Unstaged { .. } => pill(ui, pal, "Unstaged"),
                    DiffTarget::Staged { .. } => pill(ui, pal, "Staged"),
                    DiffTarget::Conflict { .. } => pill(ui, pal, "Conflict"),
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ds::Button::icon_only(icons::X, "Close (Esc)")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::CloseDiff);
                    }
                    crate::file_view::diff_buttons(ui, target, status, pal, acts);
                    if partial && !selection.is_empty() {
                        let n = selection.len();
                        let label = format!("{verb} {n} line{}", if n == 1 { "" } else { "s" });
                        if ds::Button::new(&label)
                            .primary()
                            .small()
                            .tooltip("Shift-click extends the selection, Esc clears it")
                            .show(ui, pal)
                            .clicked()
                        {
                            let chosen: std::collections::HashSet<(usize, usize)> =
                                selection.iter().map(|&k| hunk_pos[k]).collect();
                            if let Some(patch) =
                                git::partial_patch(raw, |h, i| chosen.contains(&(h, i)), unstage)
                            {
                                acts.push(Action::Op(Op::ApplyPatch { patch, unstage }));
                            }
                        }
                    }
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.border);
                    ui.label(
                        RichText::new(format!("−{dels}"))
                            .monospace()
                            .size(11.5)
                            .color(pal.removed),
                    );
                    ui.label(
                        RichText::new(format!("+{adds}"))
                            .monospace()
                            .size(11.5)
                            .color(pal.added),
                    );
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if selection.is_empty() {
            acts.push(Action::CloseDiff);
        } else {
            selection.clear();
        }
    }
    match content {
        None => {
            ui.add_space(16.0);
            ui.spinner();
            return;
        }
        Some(Err(e)) => {
            ui.add_space(16.0);
            ui.colored_label(pal.danger, e);
            return;
        }
        Some(Ok(_)) => {}
    }
    if lines.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("No textual changes").color(pal.text_muted));
        return;
    }
    let font = ui.style().text_styles[&egui::TextStyle::Monospace].clone();
    let line_h = (font.size * 1.66).round();
    let num_font = FontId::monospace(font.size - 1.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("diff")
        .show_rows(ui, line_h, lines.len(), |ui, range| {
            for (k, l) in lines.iter().enumerate().skip(range.start).take(range.len()) {
                let change = matches!(l.kind, '+' | '-');
                let picked = selection.contains(&k);
                let (bg, sign_c, fg) = match l.kind {
                    '+' => (pal.added_bg, pal.added, pal.text),
                    '-' => (pal.removed_bg, pal.removed, pal.text),
                    '@' => (
                        crate::theme::mix(pal.canvas, pal.hunk, 0.07),
                        pal.hunk,
                        pal.hunk,
                    ),
                    '\\' => (Color32::TRANSPARENT, pal.text_subtle, pal.text_subtle),
                    _ => (Color32::TRANSPARENT, pal.text_subtle, pal.text),
                };
                let g = ui
                    .painter()
                    .layout_no_wrap(l.text.replace('\t', "    "), font.clone(), fg);
                // Old and new line numbers (40 px each), the sign column, then the code.
                let code_x = if l.kind == '@' { 12.0 } else { 96.0 };
                let w = (code_x + g.size().x + 12.0).max(ui.available_width());
                let sense = if partial && change {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let (rect, resp) = ui.allocate_exact_size(vec2(w, line_h), sense);
                // A changed line: click to choose it, Shift-click for the lines up to it.
                if partial && change {
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            true,
                            picked,
                            format!("Line {}: {}", l.new.or(l.old).unwrap_or(0), l.text.trim()),
                        )
                    });
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        if ui.input(|i| i.modifiers.shift)
                            && let Some(a) = *anchor
                        {
                            let (lo, hi) = (a.min(k), a.max(k));
                            selection
                                .extend((lo..=hi).filter(|&j| matches!(lines[j].kind, '+' | '-')));
                        } else if !selection.remove(&k) {
                            selection.insert(k);
                        }
                        *anchor = Some(k);
                    }
                }
                let p = ui.painter();
                p.rect_filled(rect, 0.0, bg);
                if picked {
                    p.rect_filled(rect, 0.0, pal.accent.gamma_multiply(0.18));
                    p.rect_filled(
                        Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                        0.0,
                        pal.accent,
                    );
                }
                let yc = rect.center().y;
                if l.kind != '@' {
                    for (n, x) in [(l.old, 40.0), (l.new, 80.0)] {
                        if let Some(n) = n {
                            p.text(
                                pos2(rect.left() + x - 8.0, yc),
                                Align2::RIGHT_CENTER,
                                n.to_string(),
                                num_font.clone(),
                                pal.text_subtle,
                            );
                        }
                    }
                    if matches!(l.kind, '+' | '-') {
                        p.text(
                            pos2(rect.left() + 84.0, yc),
                            Align2::LEFT_CENTER,
                            l.kind.to_string(),
                            font.clone(),
                            sign_c,
                        );
                    }
                }
                p.galley(pos2(rect.left() + code_x, yc - g.size().y / 2.0), g, fg);
                // The hunk header: stage (or unstage) the whole hunk, at the visible right edge.
                if partial && l.kind == '@' {
                    let hunk = hunk_pos[k].0;
                    let right = ui.clip_rect().right() - 8.0;
                    let r = Rect::from_min_max(
                        pos2(right - 104.0, rect.top() + 2.0),
                        pos2(right, rect.bottom() - 2.0),
                    );
                    let b = ui.put(r, |ui: &mut Ui| {
                        ds::Button::new(format!("{verb} hunk"))
                            .small()
                            .quiet()
                            .label(format!("{verb} hunk {}", hunk + 1))
                            .show(ui, pal)
                    });
                    if b.clicked()
                        && let Some(patch) = git::partial_patch(raw, |h, _| h == hunk, unstage)
                    {
                        acts.push(Action::Op(Op::ApplyPatch { patch, unstage }));
                    }
                }
            }
        });
}

/// `bb-pill`: "Unstaged" / "Staged" next to a working-copy file.
fn pill(ui: &mut Ui, pal: &Palette, text: &str) {
    let g = ui
        .painter()
        .layout_job(ds::text_job(text, 11.0, 500.0, pal.text_muted, false));
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 18.0), Sense::hover());
    ui.painter()
        .rect_filled(r, CornerRadius::same(9), pal.control);
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, pal.text_muted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_keys() {
        assert_eq!(ticket_key_len("PROJ-601 - New search page"), 8);
        assert_eq!(ticket_key_len("PROJ-5350 – Left-align"), 9);
        assert_eq!(ticket_key_len("Merge branch 'x'"), 0);
        assert_eq!(ticket_key_len("ABC- no number"), 0);
        assert_eq!(ticket_key_len("A1-2 x"), 4);
    }

    #[test]
    fn commit_message_joins_summary_and_description() {
        assert_eq!(commit_message(" Fix it ", "  "), "Fix it");
        assert_eq!(
            commit_message("Fix it", "\nWhy.\n\nMore.\n"),
            "Fix it\n\nWhy.\n\nMore."
        );
    }

    #[test]
    fn diff_line_numbers() {
        let d = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -3,3 +3,4 @@ fn x\n a\n-b\n+c\n+d\n e\n";
        let l = parse_diff(d);
        assert_eq!(l[0].kind, '@');
        assert_eq!((l[1].old, l[1].new), (Some(3), Some(3)));
        assert_eq!((l[2].old, l[2].new, l[2].kind), (Some(4), None, '-'));
        assert_eq!((l[3].old, l[3].new, l[3].kind), (None, Some(4), '+'));
        assert_eq!((l[4].new, l[4].text), (Some(5), "d"));
        assert_eq!((l[5].old, l[5].new), (Some(5), Some(6)));
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    #[test]
    fn author_and_date_widths_are_used_but_the_message_keeps_room() {
        let w = ColWidths {
            refs: 200.0,
            graph: None,
            author: 220.0,
            date: 120.0,
        };
        let c = columns(0.0, 1400.0, w, 3);
        assert_eq!(c.author.1 - c.author.0, 220.0);
        assert_eq!(c.date.1 - c.date.0, 120.0);
        assert!(c.msg.1 - c.msg.0 >= 100.0);
        // A huge author column gives way to the message.
        let c = columns(0.0, 900.0, ColWidths { author: 400.0, ..w }, 3);
        assert!(c.msg.1 - c.msg.0 >= 100.0, "{:?}", c.msg);
        // A narrower graph than the lanes need is allowed; None follows the lanes.
        let c = columns(
            0.0,
            1400.0,
            ColWidths {
                graph: Some(50.0),
                ..w
            },
            9,
        );
        assert_eq!(c.graph.1 - c.graph.0, 50.0);
        assert_eq!(
            columns(0.0, 1400.0, w, 2).graph.1 - columns(0.0, 1400.0, w, 2).graph.0,
            graph_fit(2)
        );
        // Narrow windows hide the author column.
        let narrow = columns(0.0, 800.0, w, 3);
        assert_eq!(narrow.author.1 - narrow.author.0, 0.0);
    }
}

// ---------------------------------------------------------------- conflict view

/// One conflicted file: per conflict block mine and incoming side by side with a choice (mine,
/// incoming, both), the plain text around them, and save once every block has a choice. During
/// a rebase git's "ours" is the upstream, so "mine" is git's "theirs" there.
fn conflict_view(
    ui: &mut Ui,
    tab: &mut RepoTab,
    path: &str,
    content: Option<Result<String, String>>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::conflict::{self, Choice, Part};
    let Some(data) = tab.data.as_ref() else {
        return;
    };
    let mine_is_ours = data.state != RepoState::Rebasing;
    let head = data.head_branch.clone().unwrap_or_else(|| "HEAD".into());

    // Header: status, path, "Conflict", close.
    egui::Frame::new()
        .fill(pal.app)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_height(36.0);
            ui.set_width(ui.available_width());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ds::status_letter(ui.painter(), r.center(), 'U', pal);
                ui.add(
                    egui::Label::new(
                        RichText::new(path)
                            .monospace()
                            .size(12.0)
                            .variation("wght", 500.0)
                            .color(pal.text),
                    )
                    .truncate(),
                );
                pill(ui, pal, "Conflict");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ds::Button::icon_only(icons::X, "Close (Esc)")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::CloseDiff);
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(Action::CloseDiff);
    }
    // Unreadable as text (e.g. not UTF-8): only the whole-file actions.
    let (text, error) = match content {
        None => {
            ui.add_space(16.0);
            ui.spinner();
            return;
        }
        Some(Err(e)) => (String::new(), Some(e)),
        Some(Ok(t)) => (t, None),
    };
    let parts = if error.is_some() {
        None
    } else {
        conflict::parse(&text)
    };
    let n = parts.as_deref().map_or(0, conflict::blocks);
    // Choices belong to this text: a file changed on disk starts over.
    let stamp = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    };
    if tab.conflict_for != stamp {
        tab.conflict_for = stamp;
        tab.conflict_choices.clear();
    }
    let choices = &mut tab.conflict_choices;
    choices.resize(n, None);
    let chosen = choices.iter().filter(|c| c.is_some()).count();

    // What the UI calls mine/incoming in git's terms, and back.
    let to_git = |mine: bool, incoming: bool| match (mine, incoming, mine_is_ours) {
        (true, false, true) | (false, true, false) => Choice::Ours,
        (true, false, false) | (false, true, true) => Choice::Theirs,
        (_, _, true) => Choice::OursThenTheirs,
        (_, _, false) => Choice::TheirsThenOurs,
    };
    let sides = |c: Choice| -> (bool, bool) {
        let (ours, theirs) = match c {
            Choice::Ours => (true, false),
            Choice::Theirs => (false, true),
            _ => (true, true),
        };
        if mine_is_ours {
            (ours, theirs)
        } else {
            (theirs, ours)
        }
    };

    // Toolbar: progress, then the actions for the whole file.
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let summary = match (&parts, &error) {
                    (_, Some(e)) => e.clone(),
                    (None, None) => "No conflict markers left in this file".to_string(),
                    (Some(_), None) => format!(
                        "{chosen} of {n} conflict{} chosen",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                ui.label(RichText::new(summary).size(12.5).color(pal.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &parts {
                        Some(parts) => {
                            let resolved = conflict::resolve(parts, choices);
                            if ds::Button::new("Save & mark resolved")
                                .icon(icons::CIRCLE_CHECK)
                                .primary()
                                .small()
                                .enabled(resolved.is_some())
                                .show(ui, pal)
                                .clicked()
                                && let Some(content) = resolved
                            {
                                acts.push(Action::Op(Op::ResolveFile {
                                    path: path.to_string(),
                                    content,
                                    expected: text.clone(),
                                }));
                            }
                        }
                        None => {
                            if ds::Button::new("Mark as resolved")
                                .icon(icons::CIRCLE_CHECK)
                                .primary()
                                .small()
                                .show(ui, pal)
                                .clicked()
                            {
                                acts.push(Action::Op(Op::MarkResolved(path.to_string())));
                            }
                        }
                    }
                    if ds::Button::new("Open in editor")
                        .icon(icons::PENCIL)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::OpenFile(path.to_string()));
                    }
                    if ds::Button::new("Take all incoming")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::TakeSide {
                            path: path.to_string(),
                            ours: !mine_is_ours,
                        }));
                    }
                    if ds::Button::new("Take all mine")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::TakeSide {
                            path: path.to_string(),
                            ours: mine_is_ours,
                        }));
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));

    let Some(parts) = parts else {
        if error.is_some() {
            return;
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new("Edited outside the app? Check the file, then mark it as resolved.")
                .color(pal.text_muted),
        );
        return;
    };
    let mono = ui.style().text_styles[&egui::TextStyle::Monospace].size;
    let code = |ui: &mut Ui, text: &str, color: Color32| {
        if text.is_empty() {
            ui.label(
                RichText::new("(nothing)")
                    .italics()
                    .size(mono)
                    .color(pal.text_subtle),
            );
        }
        for l in text.lines() {
            ui.label(RichText::new(l).monospace().size(mono).color(color));
        }
    };
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("conflict")
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut k = 0;
                    for part in &parts {
                        match part {
                            // Context: long stretches keep their first and last lines.
                            Part::Text(t) => {
                                let lines: Vec<&str> = t.lines().collect();
                                let show = |ui: &mut Ui, ls: &[&str]| {
                                    for l in ls {
                                        ui.label(
                                            RichText::new(*l)
                                                .monospace()
                                                .size(mono)
                                                .color(pal.text_muted),
                                        );
                                    }
                                };
                                if lines.len() > 8 {
                                    show(ui, &lines[..3]);
                                    ui.label(
                                        RichText::new(format!("… {} lines", lines.len() - 6))
                                            .size(11.5)
                                            .color(pal.text_subtle),
                                    );
                                    show(ui, &lines[lines.len() - 3..]);
                                } else {
                                    show(ui, &lines);
                                }
                            }
                            Part::Block(b) => {
                                k += 1;
                                let choice = choices[k - 1];
                                let (mine_on, inc_on) = choice.map_or((true, true), sides);
                                let (mine_text, inc_text, mine_label, inc_label) = if mine_is_ours {
                                    (&b.ours, &b.theirs, &b.ours_label, &b.theirs_label)
                                } else {
                                    (&b.theirs, &b.ours, &b.theirs_label, &b.ours_label)
                                };
                                // Git labels the HEAD side "HEAD": the current branch in a merge, the upstream in a rebase.
                                let name = |l: &String| -> String {
                                    match l.as_str() {
                                        "HEAD" if mine_is_ours => head.clone(),
                                        "HEAD" => "upstream".into(),
                                        _ => l.clone(),
                                    }
                                };
                                let (mine_label, inc_label) = (name(mine_label), name(inc_label));
                                ui.add_space(6.0);
                                egui::Frame::new()
                                    .stroke(Stroke::new(
                                        1.0,
                                        if choice.is_some() {
                                            pal.border
                                        } else {
                                            pal.danger
                                        },
                                    ))
                                    .corner_radius(CornerRadius::same(crate::theme::RADIUS_MD))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!("Conflict {k} of {n}"))
                                                    .size(12.5)
                                                    .variation("wght", 600.0)
                                                    .color(pal.text),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    for (text, mine, inc) in [
                                                        ("Both", true, true),
                                                        ("Incoming", false, true),
                                                        ("Mine", true, false),
                                                    ] {
                                                        let c = to_git(mine, inc);
                                                        let on = choice == Some(c);
                                                        let mut btn = ds::Button::new(text)
                                                            .small()
                                                            .label(format!(
                                                                "Conflict {k}: take {}",
                                                                text.to_lowercase()
                                                            ));
                                                        btn = if on { btn.primary() } else { btn };
                                                        if btn.show(ui, pal).clicked() {
                                                            choices[k - 1] =
                                                                if on { None } else { Some(c) };
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        ui.columns(2, |cols| {
                                            let sides = [
                                                (
                                                    "Mine",
                                                    mine_label,
                                                    mine_text,
                                                    mine_on,
                                                    pal.accent_soft,
                                                ),
                                                (
                                                    "Incoming",
                                                    inc_label,
                                                    inc_text,
                                                    inc_on,
                                                    pal.warning_soft,
                                                ),
                                            ];
                                            for (ui, (title, label, text, on, tint)) in
                                                cols.iter_mut().zip(sides)
                                            {
                                                ui.label(
                                                    RichText::new(format!("{title} · {label}"))
                                                        .size(11.5)
                                                        .color(pal.text_muted),
                                                );
                                                egui::Frame::new()
                                                    .fill(if on {
                                                        tint
                                                    } else {
                                                        Color32::TRANSPARENT
                                                    })
                                                    .corner_radius(CornerRadius::same(
                                                        crate::theme::RADIUS_SM,
                                                    ))
                                                    .inner_margin(6.0)
                                                    .show(ui, |ui| {
                                                        ui.set_width(ui.available_width());
                                                        ui.spacing_mut().item_spacing.y = 2.0;
                                                        let color = if on {
                                                            pal.text
                                                        } else {
                                                            pal.text_subtle
                                                        };
                                                        code(ui, text, color);
                                                    });
                                            }
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                        }
                    }
                });
        });
}

// ---------------------------------------------------------------- agent panel

/// Who answers in the agent panel and how: backend name, chosen model, access.
#[derive(Clone, Copy)]
pub struct AgentChoice<'a> {
    pub name: &'a str,
    pub model: Option<&'a str>,
    pub access: crate::app::AgentAccess,
    /// Show the tool calls in the conversation.
    pub show_tools: bool,
}

/// Height of the closed agent panel: a bar at the bottom of the right panel.
pub const AGENT_BAR_H: f32 = 34.0;

/// The closed agent panel; click to open it. A dot while an answer is still coming in.
pub fn agent_bar(ui: &mut Ui, pal: &Palette, acts: &mut Vec<Action>, busy: bool) {
    let rect = ui.available_rect_before_wrap();
    let (r, resp) = ui.allocate_exact_size(vec2(rect.width(), AGENT_BAR_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Open agent"));
    let p = ui.painter();
    p.rect_filled(r, 0.0, if resp.hovered() { pal.hover } else { pal.app });
    p.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, pal.border));
    let yc = r.center().y;
    ds::paint_icon(
        p,
        pos2(r.left() + 20.0, yc),
        icons::BOT,
        15.0,
        pal.text_muted,
    );
    let g = p.layout_job(ds::text_job("Agent", 12.5, 550.0, pal.text, false));
    let gw = g.size().x;
    p.galley(pos2(r.left() + 34.0, yc - g.size().y / 2.0), g, pal.text);
    if busy {
        p.circle_filled(pos2(r.left() + 42.0 + gw, yc), 3.5, pal.accent);
    }
    ds::paint_icon(
        p,
        pos2(r.right() - 20.0, yc),
        icons::CHEVRON_UP,
        14.0,
        pal.text_subtle,
    );
    if resp.clicked() {
        acts.push(Action::AgentOpen(true));
    }
}

/// The border between the details and the agent; drag to resize. Returns the vertical drag.
pub fn agent_splitter(ui: &mut Ui, pal: &Palette) -> Option<f32> {
    let rect = ui.available_rect_before_wrap();
    let (r, resp) = ui.allocate_exact_size(vec2(rect.width(), 6.0), Sense::drag());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Resize agent panel")
    });
    let active = resp.hovered() || resp.dragged();
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    ui.painter().hline(
        r.x_range(),
        r.top() + 0.5,
        Stroke::new(1.0, if active { pal.accent } else { pal.border }),
    );
    resp.dragged().then(|| resp.drag_delta().y)
}

/// The conversation with the agent: header (show large or back, close), the messages, and
/// the input. Enter sends, Shift+Enter starts a new line.
pub fn agent_panel(
    ui: &mut Ui,
    conv: &mut crate::agent::Conversation,
    agent: AgentChoice,
    pal: &Palette,
    acts: &mut Vec<Action>,
    large: bool,
) {
    use crate::agent::Role;
    let rect = ui.available_rect_before_wrap();
    let header_h = 36.0;
    let input_h = if large { 120.0 } else { 96.0 };
    let (header, rest) = rect.split_top_bottom_at_y(rect.top() + header_h);
    let (body, input) = rest.split_top_bottom_at_y(rest.bottom() - input_h);
    ui.painter().rect_filled(rect, 0.0, pal.app);

    // Header.
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(header.shrink2(vec2(12.0, 0.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(icons::BOT).size(15.0).color(pal.text_muted));
                ui.label(
                    RichText::new("Agent")
                        .size(13.0)
                        .variation("wght", 600.0)
                        .color(pal.text),
                );
                // The backend's name only when there is room for it beside the controls.
                if ui.available_width() > 360.0 {
                    ui.label(RichText::new(agent.name).size(11.5).color(pal.text_subtle));
                }
                if agent.name != "Preview" {
                    model_picker(ui, conv.model.as_deref(), agent.model, pal, acts);
                    access_picker(ui, agent.access, pal, acts);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let (icon, label, close) = if large {
                        (icons::X, "Close agent", Action::AgentMaximize(false))
                    } else {
                        (icons::CHEVRON_DOWN, "Close agent", Action::AgentOpen(false))
                    };
                    if ds::Button::icon_only(icon, label)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(close);
                    }
                    let (icon, label) = if large {
                        (icons::MINIMIZE_2, "Show in the panel")
                    } else {
                        (icons::MAXIMIZE_2, "Show large")
                    };
                    if ds::Button::icon_only(icon, label)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::AgentMaximize(!large));
                    }
                    if !conv.messages.is_empty()
                        && ds::Button::icon_only(icons::ROTATE_CCW, "New conversation")
                            .quiet()
                            .small()
                            .enabled(!conv.busy())
                            .show(ui, pal)
                            .clicked()
                    {
                        acts.push(Action::AgentClear);
                    }
                });
            });
        },
    );
    ui.painter().hline(
        rect.x_range(),
        header.bottom() - 0.5,
        Stroke::new(1.0, pal.border),
    );

    // Messages, following the end while an answer streams in.
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .id_salt("agent-messages")
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 8.0;
                        if conv.messages.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        "Ask about this repository, or tell the agent what to do. \
                                         It works in this repository and you see what it does.",
                                    )
                                    .size(12.5)
                                    .color(pal.text_muted),
                                )
                                .wrap(),
                            );
                        }
                        for m in conv
                            .messages
                            .iter()
                            .filter(|m| agent.show_tools || m.role != Role::Tool)
                        {
                            match m.role {
                                Role::User => {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Max),
                                        |ui| {
                                            egui::Frame::new()
                                                .fill(pal.accent_soft)
                                                .corner_radius(CornerRadius::same(
                                                    crate::theme::RADIUS_MD,
                                                ))
                                                .inner_margin(egui::Margin::symmetric(10, 6))
                                                .show(ui, |ui| {
                                                    ui.set_max_width(ui.available_width() * 0.85);
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&m.text).color(pal.text),
                                                        )
                                                        .wrap()
                                                        .selectable(true),
                                                    );
                                                });
                                        },
                                    );
                                }
                                Role::Agent => markdown(ui, &m.text, pal),
                                Role::Tool => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&m.text)
                                                .monospace()
                                                .size(11.5)
                                                .color(pal.text_subtle),
                                        )
                                        .wrap(),
                                    );
                                }
                                Role::Error => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&m.text).size(12.5).color(pal.danger),
                                        )
                                        .wrap(),
                                    );
                                }
                            }
                        }
                        // As long as the agent works: what it does now, and for how long.
                        if let Some((what, took)) = conv.status() {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(13.0));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{what} · {} s", took.as_secs()))
                                            .color(pal.text_muted),
                                    )
                                    .truncate(),
                                );
                            });
                        }
                    });
            });
    });

    // Input: the field with Send (or Stop) beside it.
    ui.painter().hline(
        rect.x_range(),
        input.top() + 0.5,
        Stroke::new(1.0, pal.border),
    );
    let busy = conv.busy();
    ui.scope_builder(egui::UiBuilder::new().max_rect(input.shrink(10.0)), |ui| {
        ui.horizontal_top(|ui| {
            let button_w = 32.0;
            let field_w = ui.available_width() - button_w - 8.0;
            let field = ui.add_sized(
                vec2(field_w, ui.available_height()),
                egui::TextEdit::multiline(&mut conv.input)
                    .hint_text(
                        RichText::new("Ask or instruct the agent… (Enter sends)")
                            .color(pal.text_subtle),
                    )
                    .return_key(Some(egui::KeyboardShortcut::new(
                        egui::Modifiers::SHIFT,
                        egui::Key::Enter,
                    )))
                    .frame(egui::Frame::new()),
            );
            // Named, so the MCP (and a screen reader) can find the field.
            field.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::MultilineTextInput,
                    true,
                    "Instruction for the agent",
                )
            });
            let enter = field.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if busy {
                if ds::Button::icon_only(icons::SQUARE, "Stop the agent")
                    .small()
                    .show(ui, pal)
                    .clicked()
                {
                    acts.push(Action::AgentStop);
                }
            } else {
                let empty = conv.input.trim().is_empty();
                if ds::Button::icon_only(icons::SEND_HORIZONTAL, "Send (Enter)")
                    .primary()
                    .small()
                    .enabled(!empty)
                    .show(ui, pal)
                    .clicked()
                    || (enter && !empty)
                {
                    acts.push(Action::AgentSend);
                }
            }
        });
    });
}

/// Agent answers: paragraphs, `#` headings, `-`/`*`/numbered lists, fenced code blocks, and
/// inline `code` and **bold**. Anything else shows as written.
fn markdown(ui: &mut Ui, text: &str, pal: &Palette) {
    let size = ui.style().text_styles[&egui::TextStyle::Body].size;
    let mono = ui.style().text_styles[&egui::TextStyle::Monospace].size;
    let mut lines = text.lines().peekable();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        while let Some(line) = lines.next() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                let mut code = Vec::new();
                for l in lines.by_ref() {
                    if l.trim_start().starts_with("```") {
                        break;
                    }
                    code.push(l);
                }
                egui::Frame::new()
                    .fill(pal.sunken)
                    .corner_radius(CornerRadius::same(crate::theme::RADIUS_SM))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(code.join("\n"))
                                    .monospace()
                                    .size(mono)
                                    .color(pal.text),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    });
                continue;
            }
            if trimmed.is_empty() {
                ui.add_space(2.0);
                continue;
            }
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            let (body, weight, bullet) = if hashes > 0 && trimmed[hashes..].starts_with(' ') {
                (&trimmed[hashes + 1..], 650.0, None)
            } else if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
                (rest, 400.0, Some("•".to_string()))
            } else if let Some((n, rest)) = trimmed.split_once(". ")
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit())
            {
                (rest, 400.0, Some(format!("{n}.")))
            } else {
                (trimmed, 400.0, None)
            };
            let mut job = LayoutJob::default();
            if let Some(b) = &bullet {
                let indent = (line.len() - trimmed.len()) as f32 * 4.0;
                let f = ds::text_job(b, size, 400.0, pal.text_muted, false).sections[0]
                    .format
                    .clone();
                job.append(&format!("{b} "), indent, f);
            }
            inline_markdown(&mut job, body, size, mono, weight, pal);
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap().selectable(true));
        }
    });
}

/// `code` in the mono font on a tint, **bold** heavier, the rest as is.
fn inline_markdown(
    job: &mut LayoutJob,
    text: &str,
    size: f32,
    mono: f32,
    weight: f32,
    pal: &Palette,
) {
    let fmt = |w: f32| {
        ds::text_job("x", size, w, pal.text, false).sections[0]
            .format
            .clone()
    };
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            let mut f = ds::text_job("x", mono, 400.0, pal.text, true).sections[0]
                .format
                .clone();
            f.background = pal.sunken;
            job.append(part, 0.0, f);
            continue;
        }
        for (j, piece) in part.split("**").enumerate() {
            let w = if j % 2 == 1 { 650.0 } else { weight };
            job.append(piece, 0.0, fmt(w));
        }
    }
}

/// The model in the agent panel's header; click to choose another for the next questions.
/// Shows the model that answered last when it is the chosen one, else the choice.
fn model_picker(
    ui: &mut Ui,
    answered: Option<&str>,
    chosen: Option<&str>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::agent::{CLAUDE_MODELS, model_name};
    let shown = answered
        .filter(|m| chosen.is_none_or(|c| m.contains(c)))
        .map(model_name)
        .or_else(|| {
            chosen.map(|c| {
                CLAUDE_MODELS
                    .iter()
                    .find(|(a, _)| *a == c)
                    .map_or(c.to_string(), |(_, n)| n.to_string())
            })
        })
        .unwrap_or_else(|| "Default model".into());
    let resp = ds::Button::new(&shown)
        .icon(icons::CHEVRON_DOWN)
        .quiet()
        .small()
        .label(format!("Model: {shown}"))
        .tooltip("The model for the next questions")
        .show(ui, pal);
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(240.0);
            if ds::menu_choice(ui, pal, chosen.is_none(), "Claude Code's default").clicked() {
                acts.push(Action::AgentModel(None));
                ui.close();
            }
            for (alias, name) in CLAUDE_MODELS {
                if ds::menu_choice(ui, pal, chosen == Some(*alias), name).clicked() {
                    acts.push(Action::AgentModel(Some(alias.to_string())));
                    ui.close();
                }
            }
        });
}

/// How much the agent may do without asking; Full access shows as a red open lock.
fn access_picker(
    ui: &mut Ui,
    access: crate::app::AgentAccess,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::app::AgentAccess;
    let modes = [
        (
            AgentAccess::Supervised,
            icons::LOCK,
            "Supervised",
            "Asks before pushing, rewriting or throwing away work.",
        ),
        (
            AgentAccess::Auto,
            icons::SPARKLES,
            "Auto",
            "Pushes, amends and resets on its own; asks before throwing work away.",
        ),
        (
            AgentAccess::Full,
            icons::LOCK_OPEN,
            "Full access",
            "Never asks, also not for force push, hard reset or discarding changes.",
        ),
    ];
    let (_, icon, name, _) = modes
        .iter()
        .find(|m| m.0 == access)
        .copied()
        .unwrap_or(modes[0]);
    let mut button = ds::Button::icon_only(icon, format!("Access: {name}"))
        .quiet()
        .small()
        .tooltip(format!(
            "Access: {name}. What the agent may do without asking you"
        ));
    if access == AgentAccess::Full {
        button = button.kind(ds::Kind::Danger);
    }
    let resp = button.show(ui, pal);
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(300.0);
            for (mode, icon, name, about) in modes {
                let chosen = mode == access;
                let r = ds::menu_item(
                    ui,
                    pal,
                    icon,
                    name,
                    Some(&format!("{name}: {about}")),
                    false,
                );
                if chosen {
                    ui.painter().text(
                        pos2(r.rect.right() - 14.0, r.rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        icons::CHECK,
                        FontId::proportional(14.0),
                        pal.accent,
                    );
                }
                // The explanation, under the name.
                ui.horizontal(|ui| {
                    ui.add_space(36.0);
                    ui.add(
                        egui::Label::new(RichText::new(about).size(11.5).color(pal.text_subtle))
                            .wrap(),
                    );
                });
                ui.add_space(4.0);
                if r.clicked() {
                    acts.push(Action::AgentAccess(mode));
                    ui.close();
                }
            }
        });
}
