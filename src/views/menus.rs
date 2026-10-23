//! Context menus of branches, tags and commits.

use super::*;

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

pub(super) fn commit_menu(
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
