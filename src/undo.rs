//! Undo and redo of local operations, like GitKraken: a commit, checkout, merge, reset, branch
//! or tag change, or a discard can be taken back and done again.
//!
//! Before and after an undoable operation the worker records the refs (branches, tags, HEAD), or
//! for a discard the index and working tree as trees in the object database. Undo puts the
//! recorded state back; redo puts the later one back (a discard is run again). Both refuse when
//! the repository changed since, so nothing made afterwards is overwritten.
//! For ref changes only the refs are checked: a redo of a commit leaves what was staged since
//! in the index. Undoing a pull also removes the tags it fetched (the next fetch brings them).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::git::{self, Op, RepoData, ResetMode, git};

/// Start of the error when the repository no longer is as the operation left it.
pub const STALE: &str = "The repository changed since";

/// One operation that can be undone and redone.
#[derive(Clone, Debug)]
pub struct Change {
    /// Unique within the process, to find it back on the undo and redo stacks.
    pub id: u64,
    /// The operation's label, e.g. "Commit" or "Checkout main".
    pub label: String,
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    Refs {
        before: Snapshot,
        after: Snapshot,
        /// How HEAD's branch is moved between its two commits.
        mode: Move,
        /// Index and working tree before and after a hard reset, which throws changes away.
        files: Option<(Trees, Trees)>,
    },
    /// A discard: `op` does it again; the trees put the changes back.
    Files {
        op: Op,
        paths: Vec<String>,
        before: Trees,
        after: Trees,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Move {
    /// The changes of the commits in between end up staged (undoing a commit).
    Soft,
    Mixed,
    /// `reset --keep`: the working tree follows, local changes stay or the reset refuses.
    Keep,
    /// `reset --hard`, only with the index and working tree recorded to put back.
    Hard,
}

/// Branches and tags (full ref name to object) and HEAD.
#[derive(Clone, Debug, Default, PartialEq)]
struct Snapshot {
    /// `refs/heads/x`, or `None` when detached.
    head_ref: Option<String>,
    head_sha: Option<String>,
    refs: BTreeMap<String, String>,
    /// Upstream per branch (`origin/x`), set again when a deleted branch comes back.
    upstreams: BTreeMap<String, String>,
}

/// The index and the working tree (with untracked files) as trees.
#[derive(Clone, Debug, PartialEq)]
struct Trees {
    index: String,
    work: String,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl Change {
    pub fn undo_label(&self) -> String {
        format!("Undo {}", self.label)
    }

    pub fn redo_label(&self) -> String {
        format!("Redo {}", self.label)
    }

    pub fn undo(&self, repo: &Path) -> Result<String, String> {
        match &self.kind {
            Kind::Refs {
                before,
                after,
                mode,
                files,
            } => {
                if let Some((_, now)) = files {
                    expect_trees(repo, &[], now)?;
                }
                let out = restore(repo, after, before, *mode)?;
                match files {
                    Some((was, now)) => put_trees(repo, &[], was, now),
                    None => Ok(out),
                }
            }
            Kind::Files {
                paths,
                before,
                after,
                ..
            } => {
                expect_trees(repo, paths, after)?;
                put_trees(repo, paths, before, after)
            }
        }
    }

    pub fn redo(&self, repo: &Path) -> Result<String, String> {
        match &self.kind {
            Kind::Refs {
                before,
                after,
                mode,
                files,
            } => {
                if let Some((was, _)) = files {
                    expect_trees(repo, &[], was)?;
                }
                restore(repo, before, after, *mode)
            }
            Kind::Files {
                op, paths, before, ..
            } => {
                expect_trees(repo, paths, before)?;
                op.run(repo, None)
            }
        }
    }
}

/// Runs `op` and, when it can be undone and changed something, what it changed.
pub fn run(
    op: &Op,
    repo: &Path,
    data: Option<&RepoData>,
) -> (Result<String, String>, Option<Change>) {
    let kind = |before| {
        let label = op.label();
        Some((label, before))
    };
    let recorded = match op {
        Op::Undo(c) => return (c.undo(repo), None),
        Op::Redo(c) => return (c.redo(repo), None),
        Op::Discard { path, .. } => paths_trees(repo, std::slice::from_ref(path))
            .ok()
            .map(|t| (vec![path.clone()], t))
            .and_then(|b| kind(Before::Files(b))),
        Op::DiscardAll => paths_trees(repo, &[])
            .ok()
            .map(|t| (vec![], t))
            .and_then(|b| kind(Before::Files(b))),
        _ => match mode(op) {
            Some(m) => {
                // A hard reset throws away changes: keep them to put back.
                let hard = matches!(
                    op,
                    Op::Reset {
                        mode: ResetMode::Hard,
                        ..
                    }
                );
                let trees = hard.then(|| paths_trees(repo, &[]).ok()).flatten();
                snapshot(repo)
                    .ok()
                    .and_then(|s| kind(Before::Refs(s, m, trees)))
            }
            None => None,
        },
    };
    let result = op.run(repo, data);
    let change = match (&result, recorded) {
        (Ok(_), Some((label, before))) => match before {
            Before::Refs(before, mode, trees) => snapshot(repo)
                .ok()
                .filter(|after| {
                    after.head_ref != before.head_ref
                        || after.head_sha != before.head_sha
                        || after.refs != before.refs
                })
                .map(|after| {
                    let files = trees.and_then(|t| Some((t, paths_trees(repo, &[]).ok()?)));
                    Kind::Refs {
                        before,
                        after,
                        mode: if files.is_some() { Move::Hard } else { mode },
                        files,
                    }
                }),
            Before::Files((paths, before)) => paths_trees(repo, &paths)
                .ok()
                .filter(|after| *after != before)
                .map(|after| Kind::Files {
                    op: op.clone(),
                    paths,
                    before,
                    after,
                }),
        }
        .map(|kind| Change {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            label,
            kind,
        }),
        _ => None,
    };
    (result, change)
}

enum Before {
    Refs(Snapshot, Move, Option<Trees>),
    Files((Vec<String>, Trees)),
}

/// How an operation that moves refs is taken back, or `None` when it can't be (push, stash,
/// staging, operations that stop half-way such as continuing a rebase).
fn mode(op: &Op) -> Option<Move> {
    Some(match op {
        Op::Commit { .. } => Move::Soft,
        Op::Reset { mode, .. } => match mode {
            ResetMode::Soft => Move::Soft,
            ResetMode::Mixed => Move::Mixed,
            ResetMode::Hard => Move::Keep,
        },
        Op::Pull(_)
        | Op::Checkout(_)
        | Op::CheckoutRemote { .. }
        | Op::CheckoutCommit(_)
        | Op::CheckoutUpdate { .. }
        | Op::Merge { .. }
        | Op::Rebase(_)
        | Op::CreateBranch { .. }
        | Op::DeleteBranch { .. }
        | Op::FastForward { .. }
        | Op::RenameBranch { .. }
        | Op::CreateTag { .. }
        | Op::DeleteTag(_)
        | Op::UpdateTag { .. }
        | Op::CherryPick(_)
        | Op::Revert(_)
        | Op::Onto { .. }
        | Op::FlowStart { .. }
        | Op::FlowFinish { .. } => Move::Keep,
        _ => return None,
    })
}

fn snapshot(repo: &Path) -> Result<Snapshot, String> {
    let mut s = Snapshot {
        head_ref: git(repo, &["symbolic-ref", "-q", "HEAD"])
            .ok()
            .map(|r| r.trim().to_string()),
        head_sha: git(repo, &["rev-parse", "-q", "--verify", "HEAD"])
            .ok()
            .map(|r| r.trim().to_string()),
        ..Default::default()
    };
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream:short)",
            "refs/heads",
            "refs/tags",
        ],
    )?;
    for line in out.lines() {
        let mut f = line.split('\0');
        let (Some(name), Some(sha)) = (f.next(), f.next()) else {
            continue;
        };
        s.refs.insert(name.to_string(), sha.to_string());
        if let Some(up) = f.next().filter(|u| !u.is_empty()) {
            s.upstreams.insert(name.to_string(), up.to_string());
        }
    }
    Ok(s)
}

/// Moves the repository from `from` (as it must be now) to `to`.
fn restore(repo: &Path, from: &Snapshot, to: &Snapshot, mode: Move) -> Result<String, String> {
    let now = snapshot(repo)?;
    let changed: BTreeSet<&String> = from
        .refs
        .keys()
        .chain(to.refs.keys())
        .filter(|r| from.refs.get(*r) != to.refs.get(*r))
        .collect();
    if now.head_ref != from.head_ref
        || now.head_sha != from.head_sha
        || changed
            .iter()
            .any(|r| now.refs.get(*r) != from.refs.get(*r))
    {
        return Err(format!("{STALE}; it can't be put back"));
    }
    let short = |r: &str| {
        r.strip_prefix("refs/heads/")
            .or_else(|| r.strip_prefix("refs/tags/"))
            .unwrap_or(r)
            .to_string()
    };
    // Names end up as git arguments (see `Op::names`).
    if let Some(bad) = changed.iter().find(|r| short(r).starts_with('-')) {
        return Err(format!("Refusing to use \"{}\" as a ref name", short(bad)));
    }
    // A branch checked out in another worktree is that worktree's; git itself won't move it.
    let elsewhere: Vec<String> = git(repo, &["worktree", "list", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("branch "))
        .filter(|b| Some(*b) != now.head_ref.as_deref())
        .map(str::to_string)
        .collect();
    if let Some(busy) = changed.iter().find(|r| elsewhere.contains(r)) {
        return Err(format!(
            "{} is checked out in another worktree; it can't be put back",
            short(busy)
        ));
    }
    let set =
        |r: &str, now: Option<&String>| set_ref(repo, r, to.refs.get(r), now, to.upstreams.get(r));
    let mut done = BTreeSet::new();
    let mut head = from.head_ref.clone();
    let mut out = String::new();
    // A rename (one branch gone, another at the same commit) goes back with `git branch -m`,
    // which keeps its settings and HEAD with it.
    let gone: Vec<&&String> = changed
        .iter()
        .filter(|r| r.starts_with("refs/heads/") && !to.refs.contains_key(**r))
        .collect();
    let come: Vec<&&String> = changed
        .iter()
        .filter(|r| r.starts_with("refs/heads/") && !from.refs.contains_key(**r))
        .collect();
    if let ([old], [new]) = (&gone[..], &come[..])
        && from.refs.get(**old) == to.refs.get(**new)
    {
        out += &git(repo, &["branch", "-m", &short(old), &short(new)])?;
        if head.as_ref() == Some(*old) {
            head = Some((**new).clone());
        }
        done.insert((**old).clone());
        done.insert((**new).clone());
    }
    if to.head_ref != head {
        match (&to.head_ref, &to.head_sha) {
            // Back to before the first commit.
            (Some(r), None) => {
                out += &git(repo, &["symbolic-ref", "HEAD", r])?;
            }
            (Some(r), Some(_)) => {
                let set_first = changed.contains(r) && !done.contains(r);
                if set_first {
                    set(r, from.refs.get(r))?;
                    done.insert(r.clone());
                }
                if let Err(e) = git(repo, &["switch", &short(r)]) {
                    // Leave nothing half done: the branch as it was.
                    if set_first {
                        let _ = set_ref(repo, r, from.refs.get(r), to.refs.get(r), None);
                    }
                    return Err(e);
                }
            }
            (None, sha) => {
                let sha = sha.as_deref().ok_or("No commit to go back to")?;
                out += &git(repo, &["switch", "--detach", sha])?;
            }
        }
    } else if to.head_sha != from.head_sha {
        match (&to.head_sha, &from.head_sha, &to.head_ref) {
            (Some(sha), Some(_), head) => {
                let flag = match mode {
                    Move::Soft => "--soft",
                    Move::Mixed => "--mixed",
                    Move::Keep => "--keep",
                    Move::Hard => "--hard",
                };
                out += &git(repo, &["reset", "-q", flag, sha])?;
                done.extend(head.clone());
            }
            // Before or after the first commit: the branch comes or goes, the index stays.
            (_, _, Some(r)) if mode == Move::Soft => {
                set(r, from.refs.get(r))?;
                done.insert(r.clone());
            }
            _ => return Err("This can't be undone".into()),
        }
    }
    for r in changed {
        if !done.contains(r) {
            set(r, from.refs.get(r))?;
        }
    }
    Ok(out)
}

/// Points `name` at `to` (or deletes it), provided it still points at `now`.
fn set_ref(
    repo: &Path,
    name: &str,
    to: Option<&String>,
    now: Option<&String>,
    upstream: Option<&String>,
) -> Result<(), String> {
    let branch = name.strip_prefix("refs/heads/");
    match to {
        Some(sha) => {
            git(repo, &["update-ref", name, sha, now.map_or("", |s| s)])?;
            if now.is_none()
                && let (Some(b), Some(up)) = (branch, upstream)
            {
                let _ = git(repo, &["branch", &format!("--set-upstream-to={up}"), b]);
            }
        }
        None => {
            git(repo, &["update-ref", "-d", name, now.map_or("", |s| s)])?;
            if let Some(b) = branch {
                let _ = git(
                    repo,
                    &["config", "--remove-section", &format!("branch.{b}")],
                );
            }
        }
    }
    Ok(())
}

/// The index and working tree of `paths` (all with none) as trees, through a temporary index.
fn paths_trees(repo: &Path, paths: &[String]) -> Result<Trees, String> {
    let index = git(repo, &["write-tree"])?.trim().to_string();
    let tmp = tempfile_path(repo)?;
    let r = (|| {
        git::git_index(repo, &tmp, &["read-tree", &index])?;
        // A path that is gone and not in the index (a discarded untracked file) matches nothing.
        if let Err(e) = git::git_index(repo, &tmp, &with_paths(&["add", "-A"], paths))
            && (paths.is_empty()
                || paths
                    .iter()
                    .any(|p| repo.join(p).symlink_metadata().is_ok()))
        {
            return Err(e);
        }
        git::git_index(repo, &tmp, &["write-tree"])
    })();
    let _ = std::fs::remove_file(&tmp);
    Ok(Trees {
        index,
        work: r?.trim().to_string(),
    })
}

/// Puts the index and working tree of `paths` back as `want`, from `now`.
fn put_trees(repo: &Path, paths: &[String], want: &Trees, now: &Trees) -> Result<String, String> {
    if want.index != now.index {
        git(
            repo,
            &with_paths(&["restore", "--source", &want.index, "--staged"], paths),
        )?;
    }
    git(
        repo,
        &with_paths(&["restore", "--source", &want.work, "--worktree"], paths),
    )
}

fn expect_trees(repo: &Path, paths: &[String], want: &Trees) -> Result<(), String> {
    match paths_trees(repo, paths) {
        Ok(t) if t == *want => Ok(()),
        _ => Err(format!("{STALE}; it can't be put back")),
    }
}

/// A temporary index in the repository's git directory (not shared with other users).
fn tempfile_path(repo: &Path) -> Result<std::path::PathBuf, String> {
    let name = format!(
        "branchblitz-index-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    );
    let path = git(repo, &["rev-parse", "--git-path", &name])?;
    Ok(repo.join(path.trim()))
}

fn with_paths<'a>(args: &[&'a str], paths: &'a [String]) -> Vec<&'a str> {
    let mut v = args.to_vec();
    v.push("--");
    if paths.is_empty() {
        v.push(".");
    }
    v.extend(paths.iter().map(String::as_str));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bb-undo-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "T"],
            &["config", "user.email", "t@example.com"],
        ] {
            git(&dir, args).unwrap();
        }
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "first"]).unwrap();
        dir
    }

    fn head(dir: &Path) -> String {
        git(dir, &["rev-parse", "HEAD"]).unwrap().trim().to_string()
    }

    fn branch(dir: &Path) -> String {
        git(dir, &["branch", "--show-current"])
            .unwrap()
            .trim()
            .to_string()
    }

    fn status(dir: &Path) -> String {
        git(dir, &["status", "--porcelain"]).unwrap()
    }

    fn commit(msg: &str) -> Op {
        Op::Commit {
            message: msg.into(),
            amend: false,
            no_verify: false,
            extra: vec![],
        }
    }

    fn done(op: Op, dir: &Path) -> Change {
        let (r, c) = run(&op, dir, None);
        r.unwrap();
        c.expect("undoable")
    }

    #[test]
    fn commit_undo_keeps_changes_staged_and_redo_commits_again() {
        let dir = repo();
        let first = head(&dir);
        fs::write(dir.join("a.txt"), "two\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        let c = done(commit("second"), &dir);
        let second = head(&dir);
        c.undo(&dir).unwrap();
        assert_eq!(head(&dir), first);
        assert_eq!(status(&dir), "M  a.txt\n");
        c.redo(&dir).unwrap();
        assert_eq!(head(&dir), second);
        assert_eq!(status(&dir), "");
    }

    #[test]
    fn first_commit_can_be_undone() {
        let dir = std::env::temp_dir().join(format!("bb-undo-first-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        git(&dir, &["config", "user.name", "T"]).unwrap();
        git(&dir, &["config", "user.email", "t@example.com"]).unwrap();
        fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        let c = done(commit("first"), &dir);
        c.undo(&dir).unwrap();
        assert!(git(&dir, &["rev-parse", "-q", "--verify", "HEAD"]).is_err());
        assert_eq!(status(&dir), "A  a.txt\n");
        c.redo(&dir).unwrap();
        assert_eq!(status(&dir), "");
    }

    #[test]
    fn checkout_and_new_branch_are_undone() {
        let dir = repo();
        let c = done(
            Op::CreateBranch {
                name: "feature".into(),
                at: head(&dir),
                checkout: true,
            },
            &dir,
        );
        assert_eq!(branch(&dir), "feature");
        c.undo(&dir).unwrap();
        assert_eq!(branch(&dir), "main");
        assert!(git(&dir, &["rev-parse", "-q", "--verify", "refs/heads/feature"]).is_err());
        c.redo(&dir).unwrap();
        assert_eq!(branch(&dir), "feature");
        let c = done(Op::Checkout("main".into()), &dir);
        c.undo(&dir).unwrap();
        assert_eq!(branch(&dir), "feature");
    }

    #[test]
    fn deleted_branch_and_tag_come_back() {
        let dir = repo();
        git(&dir, &["branch", "old"]).unwrap();
        git(&dir, &["tag", "-a", "v1", "-m", "v1"]).unwrap();
        let tag = git(&dir, &["rev-parse", "v1"]).unwrap();
        let c = done(
            Op::DeleteBranch {
                name: "old".into(),
                force: false,
            },
            &dir,
        );
        c.undo(&dir).unwrap();
        assert_eq!(git(&dir, &["rev-parse", "old"]).unwrap().trim(), head(&dir));
        let c = done(Op::DeleteTag("v1".into()), &dir);
        c.undo(&dir).unwrap();
        // Still the annotated tag object.
        assert_eq!(git(&dir, &["rev-parse", "v1"]).unwrap(), tag);
    }

    #[test]
    fn rename_of_current_branch_is_undone() {
        let dir = repo();
        let c = done(
            Op::RenameBranch {
                old: "main".into(),
                new: "trunk".into(),
            },
            &dir,
        );
        c.undo(&dir).unwrap();
        assert_eq!(branch(&dir), "main");
        assert!(git(&dir, &["rev-parse", "-q", "--verify", "refs/heads/trunk"]).is_err());
    }

    #[test]
    fn merge_and_hard_reset_are_undone() {
        let dir = repo();
        let first = head(&dir);
        git(&dir, &["switch", "-q", "-c", "side"]).unwrap();
        fs::write(dir.join("b.txt"), "b\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        git(&dir, &["commit", "-q", "-m", "side"]).unwrap();
        git(&dir, &["switch", "-q", "main"]).unwrap();
        fs::write(dir.join("a.txt"), "main\n").unwrap();
        git(&dir, &["commit", "-qam", "main"]).unwrap();
        let main = head(&dir);
        let c = done(
            Op::Merge {
                what: "side".into(),
                mode: git::MergeMode::NoFastForward,
            },
            &dir,
        );
        c.undo(&dir).unwrap();
        assert_eq!(head(&dir), main);
        assert!(!dir.join("b.txt").exists());
        let c = done(
            Op::Reset {
                to: first.clone(),
                mode: ResetMode::Hard,
            },
            &dir,
        );
        assert_eq!(head(&dir), first);
        c.undo(&dir).unwrap();
        assert_eq!(head(&dir), main);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "main\n");
    }

    #[test]
    fn discards_come_back_and_go_again() {
        let dir = repo();
        fs::write(dir.join("a.txt"), "changed\n").unwrap();
        fs::write(dir.join("new.txt"), "new\n").unwrap();
        let c = done(
            Op::Discard {
                path: "new.txt".into(),
                untracked: true,
            },
            &dir,
        );
        assert!(!dir.join("new.txt").exists());
        c.undo(&dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join("new.txt")).unwrap(), "new\n");
        c.redo(&dir).unwrap();
        assert!(!dir.join("new.txt").exists());
        c.undo(&dir).unwrap();

        git(&dir, &["add", "a.txt"]).unwrap();
        let before = status(&dir);
        let c = done(Op::DiscardAll, &dir);
        assert_eq!(status(&dir), "");
        c.undo(&dir).unwrap();
        assert_eq!(status(&dir), before);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "changed\n");
    }

    #[test]
    fn refuses_when_the_repository_changed_since() {
        let dir = repo();
        fs::write(dir.join("a.txt"), "two\n").unwrap();
        git(&dir, &["commit", "-qam", "x"]).unwrap();
        let c = done(
            Op::CreateBranch {
                name: "b".into(),
                at: head(&dir),
                checkout: false,
            },
            &dir,
        );
        git(&dir, &["commit", "-q", "--allow-empty", "-m", "later"]).unwrap();
        git(&dir, &["branch", "-f", "b", "HEAD"]).unwrap();
        assert!(c.undo(&dir).unwrap_err().starts_with(STALE));

        fs::write(dir.join("a.txt"), "dirty\n").unwrap();
        let c = done(
            Op::Discard {
                path: "a.txt".into(),
                untracked: false,
            },
            &dir,
        );
        fs::write(dir.join("a.txt"), "edited after\n").unwrap();
        assert!(c.undo(&dir).unwrap_err().starts_with(STALE));
    }

    #[test]
    fn hard_reset_undo_brings_back_thrown_away_changes() {
        let dir = repo();
        let first = head(&dir);
        fs::write(dir.join("a.txt"), "two\n").unwrap();
        git(&dir, &["commit", "-qam", "two"]).unwrap();
        let second = head(&dir);
        fs::write(dir.join("a.txt"), "staged\n").unwrap();
        git(&dir, &["add", "a.txt"]).unwrap();
        fs::write(dir.join("a.txt"), "dirty\n").unwrap();
        fs::write(dir.join("new.txt"), "new\n").unwrap();
        let before = status(&dir);
        let c = done(
            Op::Reset {
                to: first.clone(),
                mode: ResetMode::Hard,
            },
            &dir,
        );
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
        c.undo(&dir).unwrap();
        assert_eq!(head(&dir), second);
        assert_eq!(status(&dir), before);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "dirty\n");
        let staged = git(&dir, &["show", ":a.txt"]).unwrap();
        assert_eq!(staged, "staged\n");
        c.redo(&dir).unwrap();
        assert_eq!(head(&dir), first);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "one\n");
    }

    #[test]
    fn leaves_a_branch_of_another_worktree_alone() {
        let dir = repo();
        let c = done(
            Op::CreateBranch {
                name: "feat".into(),
                at: head(&dir),
                checkout: false,
            },
            &dir,
        );
        let other = dir.with_extension("wt");
        let _ = fs::remove_dir_all(&other);
        git(
            &dir,
            &["worktree", "add", "-q", other.to_str().unwrap(), "feat"],
        )
        .unwrap();
        assert!(c.undo(&dir).unwrap_err().contains("another worktree"));
        assert!(git(&dir, &["rev-parse", "-q", "--verify", "refs/heads/feat"]).is_ok());
    }

    #[test]
    fn rename_before_the_first_commit_is_undone() {
        let dir = std::env::temp_dir().join(format!("bb-undo-unborn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]).unwrap();
        let c = done(
            Op::RenameBranch {
                old: "main".into(),
                new: "trunk".into(),
            },
            &dir,
        );
        c.undo(&dir).unwrap();
        assert_eq!(branch(&dir), "main");
        c.redo(&dir).unwrap();
        assert_eq!(branch(&dir), "trunk");
    }

    #[test]
    fn rename_undo_keeps_branch_settings() {
        let dir = repo();
        git(&dir, &["branch", "topic"]).unwrap();
        git(&dir, &["config", "branch.topic.description", "keep me"]).unwrap();
        let c = done(
            Op::RenameBranch {
                old: "topic".into(),
                new: "renamed".into(),
            },
            &dir,
        );
        c.undo(&dir).unwrap();
        let d = git(&dir, &["config", "branch.topic.description"]).unwrap();
        assert_eq!(d.trim(), "keep me");
        assert!(git(&dir, &["config", "branch.renamed.description"]).is_err());
        c.redo(&dir).unwrap();
        assert!(git(&dir, &["rev-parse", "-q", "--verify", "refs/heads/renamed"]).is_ok());
    }

    #[test]
    fn push_and_staging_are_not_undoable() {
        let dir = repo();
        fs::write(dir.join("a.txt"), "two\n").unwrap();
        let (r, c) = run(&Op::StageAll, &dir, None);
        r.unwrap();
        assert!(c.is_none());
    }
}
