//! Drag and drop: a commit dropped on a branch is cherry-picked onto it, a branch dropped on one
//! is merged into it. Both ask first in the alert bar, and check the branch out first. Dropped on
//! a remote branch, the local one is brought up to date with it (or created) first.

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

/// The commit a drop would land on top of; the graph marks it.
fn landing_id() -> Id {
    Id::unique("bb-drop-landing")
}

/// Starts dragging `drag` when `resp` (sensing drags) starts being dragged with the primary
/// button; the secondary one is for the context menu.
pub(super) fn source(resp: &Response, drag: impl FnOnce() -> Drag) {
    if resp.drag_started_by(PointerButton::Primary) {
        egui::DragAndDrop::set_payload(&resp.ctx, drag());
    }
}

/// What dropping `drag` on branch `r` (local or remote) does: the alert's title and body, the
/// operation, and the commit the result lands on. `None` where it makes no sense: the branch
/// itself, something it already has, a branch checked out in another worktree, a merge commit,
/// a local branch that has diverged from the remote one, or while HEAD is detached or an
/// operation (merge, rebase, ...) is in progress.
fn intent(drag: &Drag, r: &RefInfo, data: &RepoData) -> Option<(String, String, Op, String)> {
    if data.head_branch.is_none() || data.state != RepoState::Clean {
        return None;
    }
    let branch = r.branch_name();
    let local = data.local(branch);
    // A remote branch: pull it into the local one first, unless that has it already.
    let (pull, tip) = match (r.kind, local) {
        (RefKind::Local, _) => (None, &r.target),
        (RefKind::Remote, Some(l)) if l.target == r.target => (None, &l.target),
        (RefKind::Remote, Some(l)) if l.upstream.as_deref() == Some(&r.name) => {
            match (l.ahead, l.behind) {
                (_, 0) => (None, &l.target),
                (0, _) => (Some(r), &r.target),
                _ => return None,
            }
        }
        // A local branch that follows something else (or nothing): not moved to this one.
        (RefKind::Remote, Some(_)) => return None,
        (RefKind::Remote, None) => (Some(r), &r.target),
        (RefKind::Tag, _) => return None,
    };
    if data.worktree_of(branch).is_some() {
        return None;
    }
    let current = data.head_branch.as_deref() == Some(branch);
    let first = match (pull, local.is_some(), current) {
        (None, _, true) => String::new(),
        (None, _, false) => format!("Checks out {branch} first. "),
        (Some(p), true, true) => format!("Pulls {} into {branch} first. ", p.name),
        (Some(p), true, false) => format!("Checks out {branch} and pulls {} first. ", p.name),
        (Some(p), false, _) => format!("Creates {branch} from {} first. ", p.name),
    };
    // Without a pull, `in_head` knows what the checked out branch has.
    let has = |sha: &str| sha == tip || (current && pull.is_none() && data.in_head(sha));
    let (title, body, then) = match drag {
        Drag::Commit {
            sha,
            subject,
            merge,
        } => {
            if *merge || has(sha) {
                return None;
            }
            (
                format!("Cherry-pick {} onto {branch}?", git::short(sha)),
                format!("{first}{subject}"),
                Op::CherryPick(sha.clone()),
            )
        }
        Drag::Branch { name, spec, target } => {
            if name == branch || *name == r.name || has(target) {
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
    let op = Op::Onto {
        branch: branch.to_string(),
        pull: pull.map(|p| (p.remote.clone().unwrap_or_default(), branch.to_string())),
        then: Box::new(then),
    };
    Some((title, body, op, tip.clone()))
}

/// Makes `resp` a drop target for branch `r`: outlined while something that can go there hovers
/// over it, and asks to cherry-pick or merge when it is dropped.
pub(super) fn target(
    ui: &Ui,
    resp: &Response,
    r: &RefInfo,
    data: &RepoData,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    let Some(drag) = resp.dnd_hover_payload::<Drag>() else {
        return;
    };
    let Some((title, body, op, tip)) = intent(&drag, r, data) else {
        return;
    };
    ui.painter().rect_stroke(
        resp.rect.expand(1.0),
        CornerRadius::same(crate::theme::RADIUS_SM),
        Stroke::new(2.0, pal.accent),
        egui::StrokeKind::Outside,
    );
    ui.ctx().data_mut(|d| {
        d.insert_temp(hint_id(), op.label());
        d.insert_temp(landing_id(), tip);
    });
    // The graph draws the landing spot from this; it may already be drawn this frame.
    ui.ctx().request_repaint();
    if resp.dnd_release_payload::<Drag>().is_some() {
        acts.push(Action::Confirm {
            title,
            body,
            op,
            danger: false,
        });
    }
}

/// The commit a drop on the hovered branch lands on top of, while dragging; once per frame.
pub(super) fn take_landing(ctx: &egui::Context) -> Option<String> {
    let sha = ctx.data_mut(|d| d.remove_temp::<String>(landing_id()));
    let sha = sha.filter(|_| egui::DragAndDrop::has_payload_of_type::<Drag>(ctx));
    if sha.is_some() {
        // Set again next frame while still over the target; gone once the pointer left it.
        ctx.request_repaint();
    }
    sha
}

/// The label that follows the pointer while dragging; call after all panels are drawn.
pub fn drag_ghost(ctx: &egui::Context, pal: &Palette) {
    let hint = ctx.data_mut(|d| d.remove_temp::<String>(hint_id()));
    let Some(drag) = egui::DragAndDrop::payload::<Drag>(ctx) else {
        // Dropped (or the graph was not drawn): no landing spot left for the next drag.
        ctx.data_mut(|d| d.remove::<String>(landing_id()));
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
    // Not over a target yet: say where it can go.
    let help = hint.is_none().then(|| {
        let what = match &*drag {
            Drag::Commit { .. } => "cherry-pick",
            Drag::Branch { .. } => "merge",
        };
        ds::galley(
            &painter,
            &format!("Drop on a branch label to {what}"),
            11.5,
            400.0,
            pal.text_muted,
            320.0,
        )
    });
    let help_h = help.as_ref().map_or(0.0, |h| h.size().y + 2.0);
    let width = g.size().x.max(help.as_ref().map_or(0.0, |h| h.size().x));
    let rect = Rect::from_min_size(
        pos + vec2(14.0, 10.0),
        vec2(width + 36.0, g.size().y.max(14.0) + help_h + 12.0),
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
    let line_y = rect.top() + 6.0 + g.size().y.max(14.0) / 2.0;
    ds::paint_icon(
        &painter,
        pos2(rect.left() + 15.0, line_y),
        icon,
        14.0,
        color,
    );
    let below = line_y + g.size().y / 2.0 + 2.0;
    painter.galley(
        pos2(rect.left() + 28.0, line_y - g.size().y / 2.0),
        g,
        pal.text,
    );
    if let Some(h) = help {
        painter.galley(pos2(rect.left() + 28.0, below), h, pal.text_muted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, kind: RefKind, target: &str) -> RefInfo {
        RefInfo {
            name: name.into(),
            full: String::new(),
            kind,
            target: target.into(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            remote: (kind == RefKind::Remote).then(|| "origin".into()),
        }
    }

    /// main (checked out) at aaa; develop at bbb, behind origin/develop at ddd;
    /// origin/fresh at eee with no local branch.
    fn data() -> RepoData {
        let mut develop = r("develop", RefKind::Local, "bbb");
        develop.upstream = Some("origin/develop".into());
        develop.behind = 1;
        RepoData {
            worktrees: Default::default(),
            limit: 0,
            head_branch: Some("main".into()),
            head_sha: Some("aaa".into()),
            refs: vec![
                r("main", RefKind::Local, "aaa"),
                develop,
                r("origin/develop", RefKind::Remote, "ddd"),
                r("origin/fresh", RefKind::Remote, "eee"),
            ],
            commits: vec![],
            status: Default::default(),
            state: RepoState::Clean,
            stashes: vec![],
            remotes: vec!["origin".into()],
            head_history: ["aaa".to_string()].into_iter().collect(),
            default_branches: vec![],
            flow: Default::default(),
            remote_tags: None,
        }
    }

    fn commit(sha: &str, merge: bool) -> Drag {
        Drag::Commit {
            sha: sha.into(),
            subject: "Fix login".into(),
            merge,
        }
    }

    #[test]
    fn dropping_asks_to_check_out_and_pick_or_merge() {
        let d = data();
        let at = |i: usize| &d.refs[i];
        let (title, body, op, tip) = intent(&commit("ccc1234567", false), at(1), &d).unwrap();
        assert_eq!(title, "Cherry-pick ccc1234 onto develop?");
        assert_eq!(body, "Checks out develop first. Fix login");
        assert_eq!(op.label(), "Cherry-pick ccc1234 onto develop");
        assert_eq!(tip, "bbb");

        let develop = Drag::Branch {
            name: "develop".into(),
            spec: "develop".into(),
            target: "bbb".into(),
        };
        assert!(intent(&develop, at(1), &d).is_none(), "onto itself");
        assert!(intent(&develop, at(2), &d).is_none(), "onto its own remote");
        let (title, body, op, _) = intent(&develop, at(0), &d).unwrap();
        assert_eq!(title, "Merge develop into main?");
        assert_eq!(body, "");
        assert_eq!(op.label(), "Merge develop into main");
        assert!(
            intent(&commit("bbb", false), at(1), &d).is_none(),
            "its tip"
        );
        assert!(intent(&commit("ccc", true), at(1), &d).is_none(), "merge");

        let mut busy = data();
        busy.state = RepoState::Rebasing;
        assert!(intent(&commit("ccc", false), &busy.refs[1], &busy).is_none());
        busy.state = RepoState::Clean;
        busy.head_branch = None;
        assert!(intent(&commit("ccc", false), &busy.refs[1], &busy).is_none());
    }

    #[test]
    fn dropping_on_a_remote_branch_pulls_it_first() {
        let d = data();
        let (_, body, op, tip) = intent(&commit("ccc", false), &d.refs[2], &d).unwrap();
        assert_eq!(
            body,
            "Checks out develop and pulls origin/develop first. Fix login"
        );
        assert_eq!(tip, "ddd", "lands on the remote tip");
        assert!(matches!(
            op,
            Op::Onto { ref branch, pull: Some((ref remote, ref b)), .. }
                if branch == "develop" && remote == "origin" && b == "develop"
        ));
        let (_, body, op, _) = intent(&commit("ccc", false), &d.refs[3], &d).unwrap();
        assert_eq!(body, "Creates fresh from origin/fresh first. Fix login");
        assert_eq!(op.label(), "Cherry-pick ccc onto fresh");

        // Diverged: nowhere to land without a real pull.
        let mut diverged = data();
        diverged.refs[1].ahead = 1;
        assert!(intent(&commit("ccc", false), &diverged.refs[2], &diverged).is_none());
        // Following another remote, or nothing: not a target.
        let mut other = data();
        other.refs[1].upstream = Some("upstream/develop".into());
        assert!(intent(&commit("ccc", false), &other.refs[2], &other).is_none());
        other.refs[1].upstream = None;
        assert!(intent(&commit("ccc", false), &other.refs[2], &other).is_none());
        // Local ahead of the remote: just the local branch.
        diverged.refs[1].behind = 0;
        let (_, body, _, tip) =
            intent(&commit("ccc", false), &diverged.refs[2], &diverged).unwrap();
        assert_eq!(
            (body.as_str(), tip.as_str()),
            ("Checks out develop first. Fix login", "bbb")
        );
    }
}
