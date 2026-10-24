//! Drag and drop: a commit dropped on a local branch is cherry-picked onto it, a branch dropped
//! on one is merged into it. Both ask first in the alert bar, and check the branch out first.

use super::*;
use eframe::egui::{CursorIcon, Id, LayerId, Order, PointerButton, Response};

/// What is being dragged.
pub(super) enum Drag {
    Commit {
        sha: String,
        subject: String,
        /// Merge commits are not cherry-picked (that needs a choice of parent).
        merge: bool,
    },
    /// `name` for people, `spec` to merge (see `RepoData::spec`), `target` its commit.
    Branch {
        name: String,
        spec: String,
        target: String,
    },
}

/// What the drop target under the pointer would do; the floating label shows it.
fn hint_id() -> Id {
    Id::unique("bb-drop-hint")
}

/// Starts dragging `drag` when `resp` (sensing drags) starts being dragged with the primary
/// button; the secondary one is for the context menu.
pub(super) fn source(resp: &Response, drag: impl FnOnce() -> Drag) {
    if resp.drag_started_by(PointerButton::Primary) {
        egui::DragAndDrop::set_payload(&resp.ctx, drag());
    }
}

/// What dropping `drag` on local branch `branch` does: the alert's title and body and the
/// operation. `None` where it makes no sense: the branch itself, something it already has, a
/// branch checked out in another worktree, a merge commit, or while HEAD is detached or an
/// operation (merge, rebase, ...) is in progress.
fn intent(drag: &Drag, branch: &str, data: &RepoData) -> Option<(String, String, Op)> {
    let tip = &data.local(branch)?.target;
    if data.worktree_of(branch).is_some()
        || data.head_branch.is_none()
        || data.state != RepoState::Clean
    {
        return None;
    }
    let current = data.head_branch.as_deref() == Some(branch);
    let first = if current {
        String::new()
    } else {
        format!("Checks out {branch} first. ")
    };
    let (title, body, then) = match drag {
        Drag::Commit {
            sha,
            subject,
            merge,
        } => {
            if *merge || sha == tip || (current && data.in_head(sha)) {
                return None;
            }
            (
                format!("Cherry-pick {} onto {branch}?", git::short(sha)),
                format!("{first}{subject}"),
                Op::CherryPick(sha.clone()),
            )
        }
        Drag::Branch { name, spec, target } => {
            if name == branch || target == tip || (current && data.in_head(target)) {
                return None;
            }
            (
                format!("Merge {name} into {branch}?"),
                first.trim_end().to_string(),
                Op::Merge {
                    what: spec.clone(),
                    mode: MergeMode::Default,
                },
            )
        }
    };
    Some((
        title,
        body,
        Op::Onto {
            branch: branch.to_string(),
            then: Box::new(then),
        },
    ))
}

/// Makes `resp` a drop target for local branch `branch`: outlined while something that can go
/// there hovers over it, and asks to cherry-pick or merge when it is dropped.
pub(super) fn target(
    ui: &Ui,
    resp: &Response,
    branch: &str,
    data: &RepoData,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    let Some(drag) = resp.dnd_hover_payload::<Drag>() else {
        return;
    };
    let Some((title, body, op)) = intent(&drag, branch, data) else {
        return;
    };
    ui.painter().rect_stroke(
        resp.rect.expand(1.0),
        CornerRadius::same(crate::theme::RADIUS_SM),
        Stroke::new(2.0, pal.accent),
        egui::StrokeKind::Outside,
    );
    ui.ctx().data_mut(|d| d.insert_temp(hint_id(), op.label()));
    if resp.dnd_release_payload::<Drag>().is_some() {
        acts.push(Action::Confirm {
            title,
            body,
            op,
            danger: false,
        });
    }
}

/// The label that follows the pointer while dragging; call after all panels are drawn.
pub fn drag_ghost(ctx: &egui::Context, pal: &Palette) {
    let hint = ctx.data_mut(|d| d.remove_temp::<String>(hint_id()));
    let Some(drag) = egui::DragAndDrop::payload::<Drag>(ctx) else {
        return;
    };
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    ctx.set_cursor_icon(CursorIcon::Grabbing);
    let (icon, text) = match (&*drag, &hint) {
        (Drag::Commit { .. }, Some(h)) => (icons::CHERRY, h.clone()),
        (Drag::Branch { .. }, Some(h)) => (icons::GIT_MERGE, h.clone()),
        (Drag::Commit { sha, subject, .. }, None) => (
            icons::GIT_COMMIT_HORIZONTAL,
            format!("{}  {subject}", git::short(sha)),
        ),
        (Drag::Branch { name, .. }, None) => (icons::GIT_BRANCH, name.clone()),
    };
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::unique("bb-drag")));
    let g = ds::galley(&painter, &text, 12.5, 500.0, pal.text, 320.0);
    let rect = Rect::from_min_size(
        pos + vec2(14.0, 10.0),
        vec2(g.size().x + 36.0, g.size().y.max(14.0) + 12.0),
    );
    painter.rect(
        rect,
        CornerRadius::same(crate::theme::RADIUS_SM),
        pal.raised,
        Stroke::new(
            1.0,
            if hint.is_some() {
                pal.accent
            } else {
                pal.border_strong
            },
        ),
        egui::StrokeKind::Inside,
    );
    let color = if hint.is_some() {
        pal.accent
    } else {
        pal.text_muted
    };
    ds::paint_icon(
        &painter,
        pos2(rect.left() + 15.0, rect.center().y),
        icon,
        14.0,
        color,
    );
    painter.galley(
        pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0),
        g,
        pal.text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> RepoData {
        let local = |name: &str, target: &str| RefInfo {
            name: name.into(),
            full: format!("refs/heads/{name}"),
            kind: RefKind::Local,
            target: target.into(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            remote: None,
        };
        RepoData {
            worktrees: Default::default(),
            limit: 0,
            head_branch: Some("main".into()),
            head_sha: Some("aaa".into()),
            refs: vec![local("main", "aaa"), local("develop", "bbb")],
            commits: vec![],
            status: Default::default(),
            state: RepoState::Clean,
            stashes: vec![],
            remotes: vec![],
            head_history: ["aaa".to_string()].into_iter().collect(),
            default_branches: vec![],
            flow: Default::default(),
            remote_tags: None,
        }
    }

    #[test]
    fn dropping_asks_to_check_out_and_pick_or_merge() {
        let d = data();
        let mut commit = Drag::Commit {
            sha: "ccc1234567".into(),
            subject: "Fix login".into(),
            merge: false,
        };
        let (title, body, op) = intent(&commit, "develop", &d).unwrap();
        assert_eq!(title, "Cherry-pick ccc1234 onto develop?");
        assert_eq!(body, "Checks out develop first. Fix login");
        assert_eq!(op.label(), "Cherry-pick ccc1234 onto develop");
        let mut busy = data();
        busy.state = RepoState::Rebasing;
        assert!(intent(&commit, "develop", &busy).is_none(), "mid-rebase");
        busy.state = RepoState::Clean;
        busy.head_branch = None;
        assert!(intent(&commit, "develop", &busy).is_none(), "detached");
        if let Drag::Commit { merge, .. } = &mut commit {
            *merge = true;
        }
        assert!(intent(&commit, "develop", &d).is_none(), "merge commit");

        let branch = Drag::Branch {
            name: "develop".into(),
            spec: "develop".into(),
            target: "bbb".into(),
        };
        assert!(intent(&branch, "develop", &d).is_none(), "onto itself");
        let (title, _, op) = intent(&branch, "main", &d).unwrap();
        assert_eq!(title, "Merge develop into main?");
        assert_eq!(op.label(), "Merge develop into main");
        assert!(intent(&branch, "nope", &d).is_none(), "not a local branch");
        let tip = Drag::Commit {
            sha: "bbb".into(),
            subject: String::new(),
            merge: false,
        };
        assert!(intent(&tip, "develop", &d).is_none(), "its own tip");
    }
}
