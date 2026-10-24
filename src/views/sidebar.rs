//! The sidebar: local and remote branches as folder trees, tags, stashes, worktrees.

use super::*;

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
    // Branches drag onto another branch to merge them into it.
    let sense = if r.kind == RefKind::Tag {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let resp = ds::row_sense(ui, pal, ROW_H, selected, label, sense);
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
    if r.kind != RefKind::Tag {
        dnd::source(&resp, || dnd::Drag::Branch {
            name: r.name.clone(),
            spec: data.spec(r),
            target: r.target.clone(),
        });
    }
    if r.kind == RefKind::Local {
        dnd::target(ui, &resp, &r.name, data, pal, acts);
    }
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
pub(super) fn checkout_item(
    ui: &mut Ui,
    pal: &Palette,
    name: &str,
    data: &RepoData,
    acts: &mut Vec<Action>,
) {
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
