//! Thin layer over the `git` command line. Everything runs the real git binary so
//! credentials, SSH agents, hooks and config behave exactly like in the terminal.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn command(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    crate::child_env(&mut cmd)
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.quotepath=false", "-c", "color.ui=never"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("LC_ALL", "C")
        // Paths are file names, never globs: `[id].tsx` must not match `i.tsx`.
        .env("GIT_LITERAL_PATHSPECS", "1")
        // Reads (status, log) must not take index.lock while an operation runs.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null());
    // Tests must not depend on the developer's git config (commit signing, hooks).
    #[cfg(test)]
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

fn output(repo: &Path, args: &[&str]) -> Result<Output, String> {
    command(repo)
        .args(args)
        .output()
        .map_err(|e| format!("Could not start git: {e}"))
}

/// Like [`output`], but kills git when it runs longer than `timeout`
/// (a network operation waiting for a password that never comes).
fn output_timeout(repo: &Path, args: &[&str], timeout: Duration) -> Result<Output, String> {
    let mut child = command(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start git: {e}"))?;
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "git {} timed out after {}s (is it waiting for a password? Use an SSH agent or credential helper)",
                args.first().unwrap_or(&""),
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
}

fn result(out: Output) -> Result<String, String> {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let msg = format!("{}\n{}", stdout.trim(), stderr.trim());
        Err(msg.trim().to_string())
    }
}

/// Runs git and returns stdout, or the combined output as error.
pub fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    result(output(repo, args)?)
}

fn git_network(repo: &Path, args: &[&str]) -> Result<String, String> {
    result(output_timeout(repo, args, NETWORK_TIMEOUT)?)
}

const NETWORK_TIMEOUT: Duration = Duration::from_secs(180);

/// Runs git and returns stdout regardless of the exit code (for `diff --no-index`).
fn git_lenient(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = output(repo, args)?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git version 2.47.0`, for the diagnostics in Settings, About.
pub fn version() -> Result<String, String> {
    git(&std::env::temp_dir(), &["--version"]).map(|s| s.trim().to_string())
}

/// Part of a one-file `git diff` as a patch for `git apply --cached --recount`: the changed
/// lines for which `chosen(hunk, line)` holds (`hunk` 0-based, `line` the index in that hunk's
/// body). Staging (a worktree diff): an unchosen removal stays as context and an unchosen addition
/// is left out. Unstaging (a `--cached` diff, applied with `-R`): the other way round. Hunks
/// without a chosen change are left out; `None` when nothing is left.
pub fn partial_patch(
    diff: &str,
    chosen: impl Fn(usize, usize) -> bool,
    unstage: bool,
) -> Option<String> {
    let all: Vec<&str> = diff.split_inclusive('\n').collect();
    let first_hunk = all.iter().position(|l| l.starts_with("@@"))?;
    let header: String = all[..first_hunk].concat();
    if !header.contains("\n--- ") && !header.starts_with("--- ") {
        return None;
    }
    let starts: Vec<usize> = (first_hunk..all.len())
        .filter(|&i| all[i].starts_with("@@"))
        .collect();
    let (drop, as_context) = if unstage { ('-', '+') } else { ('+', '-') };
    let mut patch = header;
    let mut any = false;
    for (h, &start) in starts.iter().enumerate() {
        let end = starts.get(h + 1).copied().unwrap_or(all.len());
        let mut body = String::new();
        let mut changes = 0;
        // Whether the last line was kept: a "\ No newline" note goes with it.
        let mut kept = true;
        for (i, line) in all[start + 1..end].iter().enumerate() {
            match line.chars().next() {
                Some('+' | '-') if chosen(h, i) => {
                    body.push_str(line);
                    changes += 1;
                    kept = true;
                }
                Some(c) if c == drop => kept = false,
                Some(c) if c == as_context => {
                    body.push(' ');
                    body.push_str(&line[1..]);
                    kept = true;
                }
                Some('\\') => {
                    if kept {
                        body.push_str(line);
                    }
                }
                _ => {
                    body.push_str(line);
                    kept = true;
                }
            }
        }
        if changes > 0 {
            patch.push_str(all[start]);
            patch.push_str(&body);
            any = true;
        }
    }
    any.then_some(patch)
}

/// Cloning a large repository takes longer than other network operations.
const CLONE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The folder name git would give a clone of `url`: `…/team/app.git` or `host:team/app` → `app`.
pub fn repo_name_from_url(url: &str) -> Option<String> {
    let last = url
        .trim()
        .trim_end_matches('/')
        .rsplit(['/', ':', '\\'])
        .next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && name != "." && name != ".." && !name.starts_with('-'))
        .then(|| name.to_string())
}

/// `git clone <url> <dest>`; `dest` must not exist yet (or be an empty folder).
pub fn clone_repo(url: &str, dest: &Path) -> Result<PathBuf, String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err(format!("Not a repository URL: {url}"));
    }
    if dest.read_dir().is_ok_and(|mut d| d.next().is_some()) || dest.is_file() {
        return Err(format!("{} already exists", dest.display()));
    }
    let parent = dest.parent().ok_or("No folder to clone into")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    let dest_arg = dest.to_str().ok_or("The folder name is not valid text")?;
    result(output_timeout(
        parent,
        &["clone", "--", url, dest_arg],
        CLONE_TIMEOUT,
    )?)?;
    Ok(dest.to_path_buf())
}

/// The newest release (`vX.Y.Z` tag) of the repository at `url`, e.g. `0.2.0`.
pub fn latest_release(url: &str) -> Result<Option<String>, String> {
    let out = git_network(
        &std::env::temp_dir(),
        &["ls-remote", "--tags", "--refs", "--", url],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| l.split_once("refs/tags/v").map(|(_, v)| v.trim()))
        .filter_map(|v| version_key(v).map(|k| (k, v.to_string())))
        .max()
        .map(|(_, v)| v))
}

/// `1.2.3` as numbers to compare; `None` for anything else (pre-releases included).
pub fn version_key(v: &str) -> Option<(u64, u64, u64)> {
    let mut p = v.split('.').map(|n| n.parse::<u64>().ok());
    let key = (p.next()??, p.next()??, p.next()??);
    p.next().is_none().then_some(key)
}

/// Returns the top level of the working tree that contains `path`.
pub fn toplevel(path: &Path) -> Result<PathBuf, String> {
    git(path, &["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Local,
    Remote,
    Tag,
}

#[derive(Clone, Debug)]
pub struct RefInfo {
    /// Short name: `develop`, `origin/develop`, `v1.0`.
    pub name: String,
    /// Full name: `refs/heads/develop`.
    pub full: String,
    pub kind: RefKind,
    /// Commit the ref points to (peeled for annotated tags).
    pub target: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub upstream_gone: bool,
    /// For remote refs: the remote name (`origin`).
    pub remote: Option<String>,
}

impl RefInfo {
    /// Name without the remote prefix (`origin/develop` -> `develop`).
    pub fn branch_name(&self) -> &str {
        match &self.remote {
            Some(r) => self
                .name
                .strip_prefix(r.as_str())
                .map(|s| s.trim_start_matches('/'))
                .unwrap_or(&self.name),
            None => &self.name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub status: char,
}

#[derive(Clone, Debug, Default)]
pub struct WorkStatus {
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub conflicted: Vec<String>,
}

impl WorkStatus {
    pub fn is_dirty(&self) -> bool {
        !self.staged.is_empty() || !self.unstaged.is_empty() || !self.conflicted.is_empty()
    }
    pub fn count(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.conflicted.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepoState {
    Clean,
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

#[derive(Clone, Debug)]
pub struct Stash {
    pub name: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct RepoData {
    pub head_branch: Option<String>,
    pub head_sha: Option<String>,
    pub refs: Vec<RefInfo>,
    pub commits: Vec<Commit>,
    pub status: WorkStatus,
    pub state: RepoState,
    pub stashes: Vec<Stash>,
    pub remotes: Vec<String>,
    /// Loaded commits that are part of HEAD's history (HEAD included).
    pub head_history: HashSet<String>,
    /// Default branch per remote (`origin/HEAD` → `origin/develop`).
    pub default_branches: Vec<String>,
    pub flow: GitFlow,
    /// Tags on the remotes (name → commit), from the last fetch (`git ls-remote`). `None` until
    /// known, so a tag is never shown as unpushed by mistake.
    pub remote_tags: Option<HashMap<String, String>>,
    /// The worktrees of this repository (main first, this one included) when it has more
    /// than one. Git refuses to check out a branch that another worktree has.
    pub worktrees: Vec<Worktree>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` when detached.
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The repository's main worktree (the one with the `.git` directory).
    pub main: bool,
    /// The worktree this data was loaded from.
    pub current: bool,
    /// `git worktree lock`: kept from being removed or pruned.
    pub locked: bool,
}

impl Worktree {
    /// The directory name, e.g. `project-hotfix`.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Branch roles from `git flow init` (`gitflow.*` in the repository config), if configured.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitFlow {
    pub master: Option<String>,
    pub develop: Option<String>,
    /// (`feature`, configured prefix such as `feat-`) for feature, bugfix, release, hotfix, support.
    pub prefixes: Vec<(String, String)>,
}

impl GitFlow {
    fn parse(config: &str) -> GitFlow {
        let mut flow = GitFlow::default();
        for line in config.lines() {
            let Some((key, value)) = line.split_once(' ') else {
                continue;
            };
            let value = value.trim().to_string();
            match key {
                "gitflow.branch.master" | "gitflow.branch.main" => flow.master = Some(value),
                "gitflow.branch.develop" => flow.develop = Some(value),
                k => {
                    if let Some(kind) = k.strip_prefix("gitflow.prefix.")
                        && ["feature", "bugfix", "release", "hotfix", "support"].contains(&kind)
                        && !value.is_empty()
                    {
                        flow.prefixes.push((kind.to_string(), value));
                    }
                }
            }
        }
        flow
    }

    /// The conventional git-flow name for `branch`: the configured master/develop branch become
    /// `master`/`develop`, `<prefix>x` becomes `<kind>/x`. Other names are returned unchanged.
    pub fn canonical(&self, branch: &str) -> String {
        if self.master.as_deref() == Some(branch) {
            return "master".into();
        }
        if self.develop.as_deref() == Some(branch) {
            return "develop".into();
        }
        for (kind, prefix) in &self.prefixes {
            if let Some(rest) = branch.strip_prefix(prefix.as_str()) {
                return format!("{kind}/{rest}");
            }
        }
        branch.to_string()
    }
}

impl RepoData {
    /// Whether `r` is (or tracks the name of) a remote's default branch (`origin/HEAD`).
    pub fn is_default_branch(&self, r: &RefInfo) -> bool {
        match r.kind {
            RefKind::Remote => self.default_branches.contains(&r.name),
            RefKind::Local => self.remotes.iter().any(|remote| {
                self.default_branches
                    .iter()
                    .any(|d| d.strip_prefix(remote.as_str()) == Some(&format!("/{}", r.name)))
            }),
            RefKind::Tag => false,
        }
    }

    /// Whether `sha` is already in the checked out branch, so merging it would do nothing.
    /// Commits outside the loaded history count as not merged.
    pub fn in_head(&self, sha: &str) -> bool {
        self.head_history.contains(sha)
    }

    pub fn current_ref(&self) -> Option<&RefInfo> {
        let head = self.head_branch.as_ref()?;
        self.refs
            .iter()
            .find(|r| r.kind == RefKind::Local && &r.name == head)
    }

    /// The other worktree that has local branch `name` checked out.
    pub fn worktree_of(&self, name: &str) -> Option<&Path> {
        self.worktrees
            .iter()
            .find(|w| !w.current && w.branch.as_deref() == Some(name))
            .map(|w| w.path.as_path())
    }

    pub fn local(&self, name: &str) -> Option<&RefInfo> {
        self.refs
            .iter()
            .find(|r| r.kind == RefKind::Local && r.name == name)
    }

    /// Name to hand to git for `r`: the short name, unless another ref shares it
    /// (a tag and a branch both called `x`), then the full name.
    pub fn spec(&self, r: &RefInfo) -> String {
        if self
            .refs
            .iter()
            .any(|o| o.name == r.name && o.full != r.full)
        {
            r.full.clone()
        } else {
            r.name.clone()
        }
    }

    /// Remote to push new branches to: `origin` if present, else the first one.
    pub fn default_remote(&self) -> Option<&str> {
        self.remotes
            .iter()
            .find(|r| r.as_str() == "origin")
            .or_else(|| self.remotes.first())
            .map(|s| s.as_str())
    }
}

pub fn load(repo: &Path, limit: usize) -> Result<RepoData, String> {
    let remotes: Vec<String> = git(repo, &["remote"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    let head_branch = git(repo, &["symbolic-ref", "-q", "--short", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let head_sha = git(repo, &["rev-parse", "-q", "--verify", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (refs, default_branches) = load_refs(repo, &remotes)?;
    // Exit status 1 just means "not configured".
    let flow = GitFlow::parse(
        &git_lenient(
            repo,
            &["config", "--get-regexp", r"^gitflow\.(branch|prefix)\."],
        )
        .unwrap_or_default(),
    );
    let commits = if refs.is_empty() && head_sha.is_none() {
        Vec::new()
    } else {
        load_commits(repo, limit, head_sha.is_some())?
    };
    let status = load_status(repo)?;
    let state = load_state(repo);
    let stashes = git(repo, &["stash", "list", "--format=%gd%x1f%s"])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (name, message) = l.split_once('\x1f')?;
            Some(Stash {
                name: name.to_string(),
                message: message.to_string(),
            })
        })
        .collect();

    let head_history = history(&commits, head_sha.as_deref());
    let worktrees = parse_worktrees(
        &git(repo, &["worktree", "list", "--porcelain", "-z"]).unwrap_or_default(),
        repo,
    );
    Ok(RepoData {
        head_branch,
        head_sha,
        refs,
        commits,
        status,
        state,
        stashes,
        remotes,
        head_history,
        default_branches,
        flow,
        remote_tags: None,
        worktrees,
    })
}

/// The worktrees from `git worktree list --porcelain -z`, skipping those whose directory is gone
/// (`prunable`); `repo` marks the current one. Empty for a repository with a single worktree.
fn parse_worktrees(out: &str, repo: &Path) -> Vec<Worktree> {
    let here = repo.canonicalize().ok();
    let mut list = Vec::new();
    // Records end with an empty field; each field is `key value` or a bare flag.
    for record in out.split("\0\0") {
        let mut path = None;
        let mut branch = None;
        let mut head = None;
        let mut prunable = false;
        let mut locked = false;
        for field in record.split('\0') {
            let (key, value) = field.split_once(' ').unwrap_or((field, ""));
            match key {
                "worktree" => path = Some(PathBuf::from(value)),
                "branch" => branch = value.strip_prefix("refs/heads/").map(str::to_string),
                "HEAD" => head = Some(value.to_string()),
                "prunable" => prunable = true,
                "locked" => locked = true,
                _ => {}
            }
        }
        if let Some(path) = path
            && !prunable
        {
            let current = here.is_some() && path.canonicalize().ok() == here;
            list.push(Worktree {
                main: list.is_empty(),
                path,
                branch,
                head,
                current,
                locked,
            });
        }
    }
    if list.len() < 2 {
        list.clear();
    }
    list
}

/// Tags on the remotes (network): name → the commit it points to (annotated tags peeled).
/// Fails if any remote cannot be asked.
pub fn remote_tags(repo: &Path, remotes: &[String]) -> Result<HashMap<String, String>, String> {
    let mut tags = HashMap::new();
    for remote in remotes {
        if remote.starts_with('-') {
            continue;
        }
        let out = git_network(repo, &["ls-remote", "--tags", remote])?;
        for line in out.lines() {
            let Some((sha, name)) = line.split_once('\t') else {
                continue;
            };
            let Some(name) = name.strip_prefix("refs/tags/") else {
                continue;
            };
            // `v1^{}` is the commit an annotated tag `v1` points to; it wins over the tag object.
            match name.strip_suffix("^{}") {
                Some(tag) => {
                    tags.insert(tag.to_string(), sha.to_string());
                }
                None => {
                    tags.entry(name.to_string())
                        .or_insert_with(|| sha.to_string());
                }
            }
        }
    }
    Ok(tags)
}

impl RepoData {
    /// Whether tag `r` is on a remote, at the same commit. `None` if that is not known yet.
    pub fn tag_on_remote(&self, r: &RefInfo) -> Option<bool> {
        self.remote_tags
            .as_ref()
            .map(|t| t.get(&r.name) == Some(&r.target))
    }
}

/// All loaded commits reachable from `head`.
fn history(commits: &[Commit], head: Option<&str>) -> HashSet<String> {
    let index: HashMap<&str, &Commit> = commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut seen = HashSet::new();
    let mut todo: Vec<&str> = head.into_iter().collect();
    while let Some(sha) = todo.pop() {
        if let Some(c) = index.get(sha)
            && seen.insert(sha.to_string())
        {
            todo.extend(c.parents.iter().map(String::as_str));
        }
    }
    seen
}

/// Branches and tags, plus the default branch of each remote (`origin/HEAD`'s target).
fn load_refs(repo: &Path, remotes: &[String]) -> Result<(Vec<RefInfo>, Vec<String>), String> {
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(symref)",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ],
    )?;
    let mut refs = Vec::new();
    let mut defaults = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        if f.len() < 5 {
            continue;
        }
        let (full, obj, peeled, upstream, track) = (f[0], f[1], f[2], f[3], f[4]);
        let target = if peeled.is_empty() { obj } else { peeled }.to_string();
        let (kind, name) = if let Some(n) = full.strip_prefix("refs/heads/") {
            (RefKind::Local, n)
        } else if let Some(n) = full.strip_prefix("refs/remotes/") {
            if n.ends_with("/HEAD") {
                // Full name: `:short` may add `remotes/` when a local name is ambiguous.
                if let Some(target) = f.get(5).and_then(|t| t.strip_prefix("refs/remotes/")) {
                    defaults.push(target.to_string());
                }
                continue;
            }
            (RefKind::Remote, n)
        } else if let Some(n) = full.strip_prefix("refs/tags/") {
            (RefKind::Tag, n)
        } else {
            continue;
        };
        let remote = if kind == RefKind::Remote {
            remotes
                .iter()
                .filter(|r| name.starts_with(&format!("{r}/")))
                .max_by_key(|r| r.len())
                .cloned()
                .or_else(|| name.split('/').next().map(String::from))
        } else {
            None
        };
        let (mut ahead, mut behind) = (0, 0);
        for part in track.split(',') {
            let part = part.trim();
            if let Some(n) = part.strip_prefix("ahead ") {
                ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                behind = n.parse().unwrap_or(0);
            }
        }
        refs.push(RefInfo {
            name: name.to_string(),
            full: full.to_string(),
            kind,
            target,
            upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
            ahead,
            behind,
            upstream_gone: track == "gone",
            remote,
        });
    }
    Ok((refs, defaults))
}

fn load_commits(repo: &Path, limit: usize, has_head: bool) -> Result<Vec<Commit>, String> {
    let n = format!("-n{limit}");
    let mut args = vec![
        "log",
        "--branches",
        "--remotes",
        "--tags",
        "--date-order",
        "-z",
        "--format=%H%x1f%P%x1f%an%x1f%ct%x1f%s",
        n.as_str(),
    ];
    if has_head {
        args.push("HEAD");
    }
    let out = git(repo, &args)?;
    Ok(out
        .split('\0')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches('\n');
            let f: Vec<&str> = rec.splitn(5, '\x1f').collect();
            if f.len() < 5 {
                return None;
            }
            Some(Commit {
                sha: f[0].to_string(),
                parents: f[1].split_whitespace().map(String::from).collect(),
                author: f[2].to_string(),
                time: f[3].parse().unwrap_or(0),
                subject: f[4].to_string(),
            })
        })
        .collect())
}

fn load_status(repo: &Path) -> Result<WorkStatus, String> {
    let out = git(repo, &["status", "--porcelain=v1", "-z"])?;
    let mut st = WorkStatus::default();
    let mut parts = out.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = parts.next() {
        if entry.len() < 4 {
            continue;
        }
        let x = entry.as_bytes()[0] as char;
        let y = entry.as_bytes()[1] as char;
        let path = entry[3..].to_string();
        let old_path = if x == 'R' || x == 'C' {
            parts.next().map(String::from)
        } else {
            None
        };
        let conflict = matches!(
            (x, y),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        );
        if conflict {
            st.conflicted.push(path);
            continue;
        }
        if x == '?' {
            st.unstaged.push(FileChange {
                path,
                old_path: None,
                status: '?',
            });
            continue;
        }
        if x != ' ' {
            st.staged.push(FileChange {
                path: path.clone(),
                old_path: old_path.clone(),
                status: x,
            });
        }
        if y != ' ' {
            st.unstaged.push(FileChange {
                path,
                old_path: None,
                status: y,
            });
        }
    }
    Ok(st)
}

fn load_state(repo: &Path) -> RepoState {
    let exists = |p: &str| {
        git(repo, &["rev-parse", "--git-path", p])
            .map(|s| {
                let p = PathBuf::from(s.trim());
                let p = if p.is_relative() { repo.join(p) } else { p };
                p.exists()
            })
            .unwrap_or(false)
    };
    if exists("rebase-merge") || exists("rebase-apply") {
        RepoState::Rebasing
    } else if exists("MERGE_HEAD") {
        RepoState::Merging
    } else if exists("CHERRY_PICK_HEAD") {
        RepoState::CherryPicking
    } else if exists("REVERT_HEAD") {
        RepoState::Reverting
    } else {
        RepoState::Clean
    }
}

#[derive(Clone, Debug)]
pub struct CommitDetails {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub committer: String,
    pub message: String,
    pub files: Vec<FileChange>,
}

pub fn commit_details(repo: &Path, sha: &str) -> Result<CommitDetails, String> {
    let head = git(
        repo,
        &[
            "show",
            "-s",
            "--format=%H%x00%P%x00%an%x00%ae%x00%ct%x00%cn%x00%B",
            sha,
        ],
    )?;
    let f: Vec<&str> = head.splitn(7, '\0').collect();
    if f.len() < 7 {
        return Err("Unexpected output from git show".into());
    }
    let files_out = git(
        repo,
        &[
            "show",
            "--format=",
            "--name-status",
            "-z",
            "--diff-merges=first-parent",
            sha,
        ],
    )?;
    let mut files = Vec::new();
    let mut parts = files_out
        .split('\0')
        .map(|s| s.trim_matches('\n'))
        .filter(|s| !s.is_empty());
    while let Some(status) = parts.next() {
        let c = status.chars().next().unwrap_or('M');
        if c == 'R' || c == 'C' {
            let old = parts.next().unwrap_or_default().to_string();
            let new = parts.next().unwrap_or_default().to_string();
            files.push(FileChange {
                path: new,
                old_path: Some(old),
                status: c,
            });
        } else if let Some(p) = parts.next() {
            files.push(FileChange {
                path: p.to_string(),
                old_path: None,
                status: c,
            });
        }
    }
    Ok(CommitDetails {
        sha: f[0].to_string(),
        parents: f[1].split_whitespace().map(String::from).collect(),
        author: f[2].to_string(),
        email: f[3].to_string(),
        time: f[4].parse().unwrap_or(0),
        committer: f[5].to_string(),
        message: f[6].trim().to_string(),
        files,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiffTarget {
    Commit {
        sha: String,
        path: String,
    },
    Unstaged {
        path: String,
        untracked: bool,
    },
    Staged {
        path: String,
    },
    /// A conflicted file as it is in the working tree, markers and all.
    Conflict {
        path: String,
    },
}

impl DiffTarget {
    pub fn path(&self) -> &str {
        match self {
            DiffTarget::Commit { path, .. }
            | DiffTarget::Unstaged { path, .. }
            | DiffTarget::Staged { path }
            | DiffTarget::Conflict { path } => path,
        }
    }
}

pub fn diff(repo: &Path, target: &DiffTarget) -> Result<String, String> {
    match target {
        DiffTarget::Commit { sha, path } => git(
            repo,
            &[
                "show",
                "--format=",
                "--diff-merges=first-parent",
                sha,
                "--",
                path,
            ],
        ),
        DiffTarget::Unstaged {
            path,
            untracked: true,
        } => git_lenient(repo, &["diff", "--no-index", "--", "/dev/null", path]),
        DiffTarget::Unstaged { path, .. } => git(repo, &["diff", "--", path]),
        DiffTarget::Staged { path } => git(repo, &["diff", "--cached", "--", path]),
        // Not lossy: the view writes this text back, so other encodings must not get there.
        DiffTarget::Conflict { path } => {
            let bytes =
                std::fs::read(repo.join(path)).map_err(|e| format!("Cannot read {path}: {e}"))?;
            String::from_utf8(bytes).map_err(|_| {
                format!("{path} is not UTF-8 text; take a whole side or open it in an editor")
            })
        }
    }
}

/// Where to write the resolution of conflicted file `path`: a plain file inside the repository,
/// not under `.git`, not through a symlink, and (when the status is known) conflicted.
fn writable_conflict(repo: &Path, path: &str, data: Option<&RepoData>) -> Result<PathBuf, String> {
    let refuse = || Err(format!("Refusing to write {path}"));
    let rel = Path::new(path);
    if rel.components().any(|c| match c {
        std::path::Component::Normal(n) => n.eq_ignore_ascii_case(".git"),
        _ => true,
    }) {
        return refuse();
    }
    if data.is_some_and(|d| !d.status.conflicted.iter().any(|c| c == path)) {
        return refuse();
    }
    let file = repo.join(rel);
    let root = repo.canonicalize().map_err(|e| e.to_string())?;
    let parent = file
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(|| format!("Cannot find the directory of {path}"))?;
    let link = std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_symlink());
    if link || !parent.starts_with(&root) {
        return refuse();
    }
    Ok(file)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeMode {
    Default,
    NoFastForward,
    FastForwardOnly,
    Squash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullMode {
    Merge,
    FastForwardOnly,
    Rebase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
}

/// A mutating git operation. Runs on the repo worker thread.
#[derive(Clone, Debug)]
pub enum Op {
    Fetch,
    Pull(PullMode),
    Push {
        force: bool,
    },
    Checkout(String),
    CheckoutRemote {
        remote_ref: String,
        local: String,
    },
    CheckoutCommit(String),
    Merge {
        what: String,
        mode: MergeMode,
    },
    Rebase(String),
    CreateBranch {
        name: String,
        at: String,
        checkout: bool,
    },
    DeleteBranch {
        name: String,
        force: bool,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
    PushBranch {
        remote: String,
        branch: String,
    },
    FastForward {
        local: String,
        to: String,
    },
    RenameBranch {
        old: String,
        new: String,
    },
    CreateTag {
        name: String,
        at: String,
    },
    DeleteTag(String),
    PushTag {
        remote: String,
        name: String,
    },
    CherryPick(String),
    Revert(String),
    Reset {
        to: String,
        mode: ResetMode,
    },
    Stash,
    StashPop(Option<String>),
    StashApply(String),
    StashDrop(String),
    Stage(Vec<String>),
    StageAll,
    /// Stages (or with `unstage` takes out of the index) a patch made by `partial_patch`.
    ApplyPatch {
        patch: String,
        unstage: bool,
    },
    Unstage(Vec<String>),
    UnstageAll,
    Discard {
        path: String,
        untracked: bool,
    },
    /// Staged, unstaged and untracked changes (not ignored files).
    DiscardAll,
    Commit {
        message: String,
        amend: bool,
    },
    MarkResolved(String),
    TakeSide {
        path: String,
        ours: bool,
    },
    AbortMerge,
    AbortRebase,
    ContinueRebase,
    AbortCherryPick,
    ContinueMerge,
    /// `git worktree remove`, then `git branch -d` when `branch` is given.
    RemoveWorktree {
        path: String,
        branch: Option<String>,
    },
    LockWorktree {
        path: String,
        lock: bool,
    },
    /// Writes the resolved content of a conflicted file and stages it, provided the file still
    /// holds `expected` (what the choices were made on).
    ResolveFile {
        path: String,
        content: String,
        expected: String,
    },
}

impl Op {
    pub fn label(&self) -> String {
        match self {
            Op::Fetch => "Fetch".into(),
            Op::Pull(_) => "Pull".into(),
            Op::Push { force: false } => "Push".into(),
            Op::Push { force: true } => "Force push".into(),
            Op::Checkout(b) => format!("Checkout {b}"),
            Op::CheckoutRemote { local, .. } => format!("Checkout {local}"),
            Op::CheckoutCommit(s) => format!("Checkout {}", short(s)),
            Op::Merge { what, .. } => format!("Merge {what}"),
            Op::Rebase(b) => format!("Rebase onto {b}"),
            Op::CreateBranch { name, .. } => format!("Create branch {name}"),
            Op::DeleteBranch { name, .. } => format!("Delete {name}"),
            Op::DeleteRemoteBranch { remote, branch } => format!("Delete {remote}/{branch}"),
            Op::PushBranch { remote, branch } => format!("Push {branch} to {remote}"),
            Op::FastForward { local, to } => format!("Fast-forward {local} to {to}"),
            Op::RenameBranch { new, .. } => format!("Rename to {new}"),
            Op::CreateTag { name, .. } => format!("Create tag {name}"),
            Op::DeleteTag(t) => format!("Delete tag {t}"),
            Op::PushTag { name, .. } => format!("Push tag {name}"),
            Op::CherryPick(s) => format!("Cherry-pick {}", short(s)),
            Op::Revert(s) => format!("Revert {}", short(s)),
            Op::Reset { to, .. } => format!("Reset to {}", short(to)),
            Op::Stash => "Stash".into(),
            Op::StashPop(_) => "Pop stash".into(),
            Op::StashApply(s) => format!("Apply {s}"),
            Op::StashDrop(s) => format!("Drop {s}"),
            Op::Stage(_) | Op::StageAll => "Stage".into(),
            Op::ApplyPatch { unstage: false, .. } => "Stage selection".into(),
            Op::ApplyPatch { unstage: true, .. } => "Unstage selection".into(),
            Op::Unstage(_) | Op::UnstageAll => "Unstage".into(),
            Op::Discard { path, .. } => format!("Discard {path}"),
            Op::DiscardAll => "Discard all changes".into(),
            Op::Commit { .. } => "Commit".into(),
            Op::MarkResolved(p) => format!("Resolve {p}"),
            Op::TakeSide { path, .. } => format!("Resolve {path}"),
            Op::AbortMerge => "Abort merge".into(),
            Op::AbortRebase => "Abort rebase".into(),
            Op::ContinueRebase => "Continue rebase".into(),
            Op::AbortCherryPick => "Abort cherry-pick".into(),
            Op::ContinueMerge => "Commit merge".into(),
            Op::RemoveWorktree { branch: None, .. } => "Remove worktree".into(),
            Op::RemoveWorktree {
                branch: Some(_), ..
            } => "Remove worktree and branch".into(),
            Op::LockWorktree { lock: true, .. } => "Lock worktree".into(),
            Op::LockWorktree { lock: false, .. } => "Unlock worktree".into(),
            Op::ResolveFile { path, .. } => format!("Resolve {path}"),
        }
    }

    /// Ref names, commits and other names that end up as positional git arguments.
    fn names(&self) -> Vec<&str> {
        match self {
            Op::Checkout(a)
            | Op::CheckoutCommit(a)
            | Op::Rebase(a)
            | Op::DeleteTag(a)
            | Op::CherryPick(a)
            | Op::Revert(a) => vec![a],
            Op::CheckoutRemote { remote_ref, local } => vec![remote_ref, local],
            Op::Merge { what, .. } => vec![what],
            Op::CreateBranch { name, at, .. } | Op::CreateTag { name, at } => vec![name, at],
            Op::DeleteBranch { name, .. } => vec![name],
            Op::DeleteRemoteBranch { remote, branch } | Op::PushBranch { remote, branch } => {
                vec![remote, branch]
            }
            Op::FastForward { local, to } => vec![local, to],
            Op::RenameBranch { old, new } => vec![old, new],
            Op::PushTag { remote, name } => vec![remote, name],
            Op::Reset { to, .. } => vec![to],
            Op::StashPop(Some(s)) | Op::StashApply(s) | Op::StashDrop(s) => vec![s],
            Op::RemoveWorktree { path, branch } => {
                let mut v = vec![path.as_str()];
                v.extend(branch.as_deref());
                v
            }
            Op::LockWorktree { path, .. } => vec![path],
            _ => vec![],
        }
    }

    pub fn run(&self, repo: &Path, data: Option<&RepoData>) -> Result<String, String> {
        // A ref called `--exec=...` (e.g. a tag fetched from a remote) must never become an option.
        if let Some(bad) = self
            .names()
            .into_iter()
            .find(|n| n.starts_with('-') || n.is_empty())
        {
            return Err(format!("Refusing to use \"{bad}\" as a ref name"));
        }
        let g = |args: &[&str]| git(repo, args);
        let net = |args: &[&str]| git_network(repo, args);
        let unborn = data.is_some_and(|d| d.head_sha.is_none());
        match self {
            Op::Fetch => net(&["fetch", "--all", "--prune"]),
            Op::Pull(mode) => net(&[
                "pull",
                match mode {
                    PullMode::Merge => "--no-rebase",
                    PullMode::FastForwardOnly => "--ff-only",
                    PullMode::Rebase => "--rebase",
                },
            ]),
            Op::Push { force } => {
                // Ask git for HEAD now: it may have changed since the last reload.
                let branch = g(&["symbolic-ref", "-q", "--short", "HEAD"])
                    .map(|s| s.trim().to_string())
                    .map_err(|_| "HEAD is detached; nothing to push".to_string())?;
                let has_upstream = g(&[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ])
                .is_ok();
                let mut args = vec!["push"];
                if *force {
                    args.push("--force-with-lease");
                }
                if !has_upstream {
                    let remote = data
                        .and_then(|d| d.default_remote())
                        .ok_or("No remote configured")?;
                    args.extend(["-u", remote, branch.as_str()]);
                }
                net(&args)
            }
            Op::Checkout(b) => g(&["switch", b]),
            Op::CheckoutRemote { remote_ref, local } => {
                g(&["switch", "-c", local, "--track", remote_ref])
            }
            Op::CheckoutCommit(s) => g(&["switch", "--detach", s]),
            Op::Merge { what, mode } => {
                let mut args = vec!["merge", "--no-edit"];
                match mode {
                    MergeMode::Default => {}
                    MergeMode::NoFastForward => args.push("--no-ff"),
                    MergeMode::FastForwardOnly => args.push("--ff-only"),
                    MergeMode::Squash => args.push("--squash"),
                }
                args.push(what);
                g(&args)
            }
            Op::Rebase(b) => g(&["rebase", b]),
            Op::CreateBranch { name, at, checkout } => {
                if *checkout {
                    g(&["switch", "-c", name, at])
                } else {
                    g(&["branch", name, at])
                }
            }
            Op::DeleteBranch { name, force } => {
                g(&["branch", if *force { "-D" } else { "-d" }, name])
            }
            Op::DeleteRemoteBranch { remote, branch } => net(&["push", remote, "--delete", branch]),
            Op::PushBranch { remote, branch } => net(&["push", "-u", remote, branch]),
            Op::FastForward { local, to } => g(&["fetch", ".", &format!("{to}:{local}")]),
            Op::RenameBranch { old, new } => g(&["branch", "-m", old, new]),
            Op::CreateTag { name, at } => g(&["tag", name, at]),
            Op::DeleteTag(t) => g(&["tag", "-d", t]),
            Op::PushTag { remote, name } => net(&["push", remote, &format!("refs/tags/{name}")]),
            Op::CherryPick(s) => g(&["cherry-pick", s]),
            Op::Revert(s) => g(&["revert", "--no-edit", s]),
            Op::Reset { to, mode } => g(&[
                "reset",
                match mode {
                    ResetMode::Soft => "--soft",
                    ResetMode::Mixed => "--mixed",
                    ResetMode::Hard => "--hard",
                },
                to,
            ]),
            Op::Stash => g(&["stash", "push", "--include-untracked"]),
            Op::StashPop(None) => g(&["stash", "pop"]),
            Op::StashPop(Some(s)) => g(&["stash", "pop", s]),
            Op::StashApply(s) => g(&["stash", "apply", s]),
            Op::StashDrop(s) => g(&["stash", "drop", s]),
            Op::Stage(paths) => {
                let mut args = vec!["add", "--"];
                args.extend(paths.iter().map(String::as_str));
                g(&args)
            }
            Op::StageAll => g(&["add", "-A"]),
            Op::ApplyPatch { patch, unstage } => {
                // Through a file: the git layer keeps stdin closed.
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let file = std::env::temp_dir()
                    .join(format!("branchblitz-{}-{n}.patch", std::process::id()));
                std::fs::write(&file, patch).map_err(|e| e.to_string())?;
                let path = file
                    .to_str()
                    .ok_or("temp path is not valid text")?
                    .to_string();
                let mut args = vec!["apply", "--cached", "--recount", "--whitespace=nowarn"];
                if *unstage {
                    args.push("-R");
                }
                args.push(&path);
                let r = g(&args);
                let _ = std::fs::remove_file(&file);
                r
            }
            Op::Unstage(paths) => {
                // Without a first commit there is no HEAD to restore from.
                let mut args = if unborn {
                    vec!["rm", "--cached", "-r", "-q", "-f", "--"]
                } else {
                    vec!["restore", "--staged", "--"]
                };
                args.extend(paths.iter().map(String::as_str));
                g(&args)
            }
            Op::UnstageAll if unborn => g(&["rm", "--cached", "-r", "-q", "-f", "--", "."]),
            Op::UnstageAll => g(&["reset", "-q"]),
            Op::Discard {
                path,
                untracked: true,
            } => g(&["clean", "-fd", "--", path]),
            Op::Discard {
                path,
                untracked: false,
            } => g(&["restore", "--", path]),
            Op::DiscardAll => {
                // Checked on disk, not in the loaded data: a merge may have started since.
                if load_state(repo) != RepoState::Clean {
                    return Err("Finish or abort the merge, rebase or cherry-pick first".into());
                }
                g(&["reset", "--hard", "-q", "HEAD"]).and_then(|_| g(&["clean", "-fdq"]))
            }
            Op::Commit { message, amend } => {
                let mut args = vec!["commit", "-m", message.as_str()];
                if *amend {
                    args.push("--amend");
                }
                g(&args)
            }
            Op::MarkResolved(p) => g(&["add", "--", p]),
            Op::TakeSide { path, ours } => {
                g(&[
                    "checkout",
                    if *ours { "--ours" } else { "--theirs" },
                    "--",
                    path,
                ])?;
                g(&["add", "--", path])
            }
            Op::AbortMerge => g(&["merge", "--abort"]),
            Op::AbortRebase => g(&["rebase", "--abort"]),
            Op::ContinueRebase => g(&["rebase", "--continue"]),
            Op::AbortCherryPick => g(&["cherry-pick", "--abort"]),
            Op::RemoveWorktree { path, branch } => {
                let out = g(&["worktree", "remove", path])?;
                match branch {
                    Some(b) => g(&["branch", "-d", b]),
                    None => Ok(out),
                }
            }
            Op::ResolveFile {
                path,
                content,
                expected,
            } => {
                let file = writable_conflict(repo, path, data)?;
                let now = std::fs::read(&file).map_err(|e| format!("Cannot read {path}: {e}"))?;
                if now != expected.as_bytes() {
                    return Err(format!(
                        "{path} changed on disk; reload (F5) and choose again"
                    ));
                }
                std::fs::write(&file, content).map_err(|e| format!("Cannot write {path}: {e}"))?;
                g(&["add", "--", path])
            }
            Op::LockWorktree { path, lock } => {
                g(&["worktree", if *lock { "lock" } else { "unlock" }, path])
            }
            Op::ContinueMerge => g(&["commit", "--no-edit"]),
        }
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// Groups refs by the commit they point to.
pub fn refs_by_commit(refs: &[RefInfo]) -> HashMap<String, Vec<usize>> {
    let mut map: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in refs.iter().enumerate() {
        map.entry(r.target.clone()).or_default().push(i);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway repository with one commit on `main`.
    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bb-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "t@example.com"],
        ] {
            git(&dir, args).unwrap();
        }
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(&dir, &["add", "a.txt"]).unwrap();
        git(&dir, &["commit", "-qm", "first"]).unwrap();
        dir
    }

    #[test]
    fn lists_tags_on_the_remote() {
        let dir = repo("rtags");
        let bare = dir.with_extension("remote.git");
        let _ = std::fs::remove_dir_all(&bare);
        git(&dir, &["init", "-q", "--bare", bare.to_str().unwrap()]).unwrap();
        git(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]).unwrap();
        git(&dir, &["tag", "v1"]).unwrap();
        git(&dir, &["tag", "-a", "v2", "-m", "annotated"]).unwrap();
        git(
            &dir,
            &["push", "-q", "origin", "refs/tags/v1", "refs/tags/v2"],
        )
        .unwrap();
        git(&dir, &["tag", "local-only"]).unwrap();
        let tags = remote_tags(&dir, &["origin".into()]).unwrap();
        let head = git(&dir, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(tags.get("v1"), Some(&head), "{tags:?}");
        assert_eq!(
            tags.get("v2"),
            Some(&head),
            "annotated tag peeled to its commit"
        );
        assert!(!tags.contains_key("local-only"));
        assert!(!tags.keys().any(|t| t.ends_with("^{}")));
        assert!(remote_tags(&dir, &["nope".into()]).is_err());
        let _ = std::fs::remove_dir_all(&bare);
    }

    #[test]
    fn knows_branches_checked_out_in_other_worktrees() {
        let dir = repo("worktrees");
        let wt = dir.with_extension("wt");
        let gone = dir.with_extension("gone");
        let _ = std::fs::remove_dir_all(&wt);
        let _ = std::fs::remove_dir_all(&gone);
        git(&dir, &["branch", "other"]).unwrap();
        git(&dir, &["branch", "old"]).unwrap();
        git(
            &dir,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "other"],
        )
        .unwrap();
        git(
            &dir,
            &["worktree", "add", "-q", gone.to_str().unwrap(), "old"],
        )
        .unwrap();
        std::fs::remove_dir_all(&gone).unwrap();

        let data = load(&dir, 100).unwrap();
        assert_eq!(
            data.worktree_of("other").map(|p| p.canonicalize().unwrap()),
            Some(wt.canonicalize().unwrap())
        );
        assert_eq!(data.worktree_of("main"), None, "this worktree's own branch");
        assert_eq!(data.worktree_of("old"), None, "directory is gone");
        // From the other worktree, main is the one checked out elsewhere.
        let there = load(&wt, 100).unwrap();
        assert!(there.worktree_of("main").is_some());
        assert_eq!(there.worktree_of("other"), None);
        // The list: main first, the gone one left out, the current one marked.
        let list: Vec<_> = there
            .worktrees
            .iter()
            .map(|w| (w.branch.as_deref(), w.main, w.current))
            .collect();
        assert_eq!(
            list,
            [(Some("main"), true, false), (Some("other"), false, true)]
        );
        assert_eq!(
            there.worktrees[1].name(),
            wt.file_name().unwrap().to_str().unwrap()
        );
        assert!(
            load(&repo("single"), 10).unwrap().worktrees.is_empty(),
            "one worktree: no list"
        );
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn locks_and_removes_worktrees() {
        let dir = repo("wt-ops");
        let wt = |n: &str| dir.with_extension(n);
        for (name, branch) in [("a", "wa"), ("b", "wb")] {
            let _ = std::fs::remove_dir_all(wt(name));
            git(
                &dir,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    wt(name).to_str().unwrap(),
                ],
            )
            .unwrap();
        }
        let path = |n: &str| wt(n).to_str().unwrap().to_string();
        let locked = |d: &RepoData, n: &str| {
            d.worktrees
                .iter()
                .find(|w| w.branch.as_deref() == Some(n))
                .unwrap()
                .locked
        };

        Op::LockWorktree {
            path: path("a"),
            lock: true,
        }
        .run(&dir, None)
        .unwrap();
        let data = load(&dir, 10).unwrap();
        assert!(locked(&data, "wa") && !locked(&data, "wb"));
        assert!(
            Op::RemoveWorktree {
                path: path("a"),
                branch: None
            }
            .run(&dir, None)
            .is_err(),
            "locked"
        );
        Op::LockWorktree {
            path: path("a"),
            lock: false,
        }
        .run(&dir, None)
        .unwrap();
        assert!(!locked(&load(&dir, 10).unwrap(), "wa"));

        // Remove keeps the branch; with the branch, it goes too (merged: same commit as main).
        Op::RemoveWorktree {
            path: path("a"),
            branch: None,
        }
        .run(&dir, None)
        .unwrap();
        Op::RemoveWorktree {
            path: path("b"),
            branch: Some("wb".into()),
        }
        .run(&dir, None)
        .unwrap();
        assert!(!wt("a").exists() && !wt("b").exists());
        let data = load(&dir, 10).unwrap();
        assert!(data.worktrees.is_empty());
        assert!(data.local("wa").is_some());
        assert!(data.local("wb").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_file_writes_and_stages_inside_the_repo_only() {
        let dir = repo("resolve");
        let resolve = |path: &str, expected: &str| {
            Op::ResolveFile {
                path: path.into(),
                content: "resolved\n".into(),
                expected: expected.into(),
            }
            .run(&dir, None)
        };
        assert!(resolve("a.txt", "other\n").is_err(), "changed on disk");
        resolve("a.txt", "a\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "resolved\n"
        );
        let data = load(&dir, 10).unwrap();
        assert!(data.status.staged.iter().any(|f| f.path == "a.txt"));

        // Outside the repo, into .git, or through a symlink.
        let outside = dir.with_extension("outside");
        std::fs::write(&outside, "a\n").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();
        std::fs::create_dir_all(dir.join(".git/hooks")).unwrap();
        for bad in [
            "../escape.txt",
            "/tmp/escape.txt",
            ".git/hooks/pre-commit",
            "link.txt",
        ] {
            assert!(resolve(bad, "a\n").is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "a\n");
        assert!(!dir.join(".git/hooks/pre-commit").exists());
        // With the status known, only conflicted files.
        let r = Op::ResolveFile {
            path: "a.txt".into(),
            content: "x".into(),
            expected: "resolved\n".into(),
        }
        .run(&dir, Some(&data));
        assert!(r.is_err(), "a.txt is not conflicted");
    }

    #[test]
    fn finds_the_newest_release_tag() {
        assert_eq!(version_key("0.10.0"), Some((0, 10, 0)));
        assert_eq!(version_key("1.0.0-beta"), None);
        assert_eq!(version_key("1.0"), None);
        let dir = repo("releases");
        for t in ["v0.2.0", "v0.10.0", "v1.0.0-beta", "other"] {
            git(&dir, &["tag", t]).unwrap();
        }
        let url = dir.to_str().unwrap();
        assert_eq!(latest_release(url).unwrap().as_deref(), Some("0.10.0"));
        assert!(latest_release("/nonexistent/repo").is_err());
    }

    /// Against GitHub (network): `cargo test latest_release_on_github -- --ignored`.
    #[test]
    #[ignore]
    fn latest_release_on_github() {
        let v = latest_release(crate::REPO_URL).unwrap();
        assert!(v.as_deref().and_then(version_key).is_some(), "{v:?}");
    }

    #[test]
    fn clones_into_a_new_folder() {
        assert_eq!(
            repo_name_from_url("https://github.com/a/b.git").as_deref(),
            Some("b")
        );
        assert_eq!(
            repo_name_from_url("git@host:team/app").as_deref(),
            Some("app")
        );
        assert_eq!(
            repo_name_from_url("/srv/repos/thing/").as_deref(),
            Some("thing")
        );
        assert_eq!(repo_name_from_url("https://host/"), Some("host".into()));
        assert_eq!(repo_name_from_url("--upload-pack=x"), None);
        let src = repo("clone-src");
        let parent = src.with_extension("clones");
        let _ = std::fs::remove_dir_all(&parent);
        let dest = parent.join("copy");
        let got = clone_repo(src.to_str().unwrap(), &dest).unwrap();
        assert!(got.join(".git").exists() && got.join("a.txt").exists());
        assert!(
            clone_repo(src.to_str().unwrap(), &dest)
                .unwrap_err()
                .contains("already exists")
        );
        assert!(clone_repo("--upload-pack=touch x", &parent.join("evil")).is_err());
        assert!(clone_repo("/nonexistent/repo", &parent.join("none")).is_err());
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn stages_and_unstages_parts_of_a_file() {
        let dir = repo("partial");
        let base: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("f.txt"), &base).unwrap();
        git(&dir, &["add", "f.txt"]).unwrap();
        git(&dir, &["commit", "-qm", "base"]).unwrap();
        // Two hunks: line 2 changed (and line 3 added after it), line 18 changed.
        let changed = base
            .replace("line 2\n", "line 2 changed\nline 2b added\n")
            .replace("line 18\n", "line 18 changed\n");
        std::fs::write(dir.join("f.txt"), &changed).unwrap();
        let worktree = || git(&dir, &["diff", "--", "f.txt"]).unwrap();
        let staged = || git(&dir, &["diff", "--cached", "--", "f.txt"]).unwrap();
        let apply = |patch: String, unstage: bool| {
            Op::ApplyPatch { patch, unstage }.run(&dir, None).unwrap();
        };

        // The second hunk only.
        apply(
            partial_patch(&worktree(), |h, _| h == 1, false).unwrap(),
            false,
        );
        assert!(staged().contains("+line 18 changed") && !staged().contains("line 2 changed"));
        // From the first hunk only the added line (body: " line 1", "-line 2", "+line 2 changed",
        // "+line 2b added", ...): index 3.
        apply(
            partial_patch(&worktree(), |h, i| h == 0 && i == 3, false).unwrap(),
            false,
        );
        let s = staged();
        assert!(
            s.contains("+line 2b added") && !s.contains("+line 2 changed"),
            "{s}"
        );
        assert!(worktree().contains("+line 2 changed"));
        // Unstage the hunk with line 18 again (it is the second hunk of the staged diff).
        apply(partial_patch(&staged(), |h, _| h == 1, true).unwrap(), true);
        let s = staged();
        assert!(
            !s.contains("line 18") && s.contains("+line 2b added"),
            "{s}"
        );
        // The working tree is untouched throughout.
        assert_eq!(std::fs::read_to_string(dir.join("f.txt")).unwrap(), changed);
        // Nothing chosen that changes anything, or no such hunk: no patch.
        assert!(partial_patch(&worktree(), |h, i| h == 0 && i == 0, false).is_none());
        assert!(partial_patch(&worktree(), |h, _| h == 5, false).is_none());
    }

    #[test]
    fn partial_patch_keeps_the_no_newline_note() {
        let dir = repo("partial-eof");
        std::fs::write(dir.join("g.txt"), "a\nb").unwrap();
        git(&dir, &["add", "g.txt"]).unwrap();
        git(&dir, &["commit", "-qm", "base"]).unwrap();
        std::fs::write(dir.join("g.txt"), "a\nc").unwrap();
        let diff = git(&dir, &["diff", "--", "g.txt"]).unwrap();
        Op::ApplyPatch {
            patch: partial_patch(&diff, |_, _| true, false).unwrap(),
            unstage: false,
        }
        .run(&dir, None)
        .unwrap();
        assert_eq!(git(&dir, &["show", ":g.txt"]).unwrap(), "a\nc");
    }

    #[test]
    fn reads_git_flow_config() {
        let flow = GitFlow::parse(
            "gitflow.branch.master production\ngitflow.branch.develop dev\n\
             gitflow.prefix.feature feat-\ngitflow.prefix.versiontag v\n",
        );
        assert_eq!(flow.master.as_deref(), Some("production"));
        assert_eq!(flow.canonical("dev"), "develop");
        assert_eq!(flow.canonical("feat-x"), "feature/x");
        assert_eq!(flow.canonical("other"), "other");
    }

    #[test]
    fn refuses_names_that_look_like_options() {
        let dir = repo("opts");
        let marker = dir.join("pwned");
        let evil = format!("--exec=touch {}", marker.display());
        for op in [
            Op::Rebase(evil.clone()),
            Op::Merge {
                what: evil.clone(),
                mode: MergeMode::Default,
            },
            Op::DeleteBranch {
                name: "-f".into(),
                force: false,
            },
            Op::RenameBranch {
                old: "main".into(),
                new: "-M".into(),
            },
        ] {
            assert!(op.run(&dir, None).unwrap_err().contains("Refusing"));
        }
        assert!(!marker.exists());
    }

    #[test]
    fn discard_treats_paths_literally() {
        let dir = repo("pathspec");
        std::fs::create_dir_all(dir.join("p")).unwrap();
        std::fs::write(dir.join("p/[id].txt"), "1\n").unwrap();
        std::fs::write(dir.join("p/i.txt"), "1\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "files"]).unwrap();
        std::fs::write(dir.join("p/[id].txt"), "changed\n").unwrap();
        std::fs::write(dir.join("p/i.txt"), "keep me\n").unwrap();
        Op::Discard {
            path: "p/[id].txt".into(),
            untracked: false,
        }
        .run(&dir, None)
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("p/[id].txt")).unwrap(),
            "1\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("p/i.txt")).unwrap(),
            "keep me\n"
        );
    }

    #[test]
    fn discard_all_keeps_ignored_files() {
        let dir = repo("discard-all");
        std::fs::write(dir.join(".gitignore"), "local.env\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "ignore"]).unwrap();
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        git(&dir, &["add", "staged.txt"]).unwrap();
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        std::fs::write(dir.join("local.env"), "secret\n").unwrap();
        Op::DiscardAll.run(&dir, None).unwrap();
        assert!(
            git(&dir, &["status", "--porcelain"])
                .unwrap()
                .trim()
                .is_empty()
        );
        assert!(dir.join("local.env").exists(), "ignored files stay");
    }

    #[test]
    fn discard_all_refuses_during_a_merge_and_without_commits() {
        let dir = repo("discard-merge");
        git(&dir, &["switch", "-qc", "side"]).unwrap();
        std::fs::write(dir.join("a.txt"), "side\n").unwrap();
        git(&dir, &["commit", "-qam", "side"]).unwrap();
        git(&dir, &["switch", "-q", "main"]).unwrap();
        std::fs::write(dir.join("a.txt"), "main\n").unwrap();
        git(&dir, &["commit", "-qam", "main"]).unwrap();
        assert!(git(&dir, &["merge", "side"]).is_err(), "conflict expected");
        std::fs::write(dir.join("new.txt"), "keep\n").unwrap();
        assert!(Op::DiscardAll.run(&dir, None).is_err());
        assert!(dir.join("MERGE_HEAD").exists() || dir.join(".git/MERGE_HEAD").exists());
        assert!(dir.join("new.txt").exists());

        let empty =
            std::env::temp_dir().join(format!("bb-test-discard-unborn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&empty).unwrap();
        git(&empty, &["init", "-q"]).unwrap();
        std::fs::write(empty.join("new.txt"), "keep\n").unwrap();
        assert!(Op::DiscardAll.run(&empty, None).is_err());
        assert!(
            empty.join("new.txt").exists(),
            "nothing cleaned without a commit"
        );
    }

    #[test]
    fn loads_refs_status_and_commits() {
        let dir = repo("load");
        git(&dir, &["branch", "feature/x"]).unwrap();
        git(&dir, &["tag", "-a", "v1", "-m", "v1"]).unwrap();
        std::fs::write(dir.join("a.txt"), "b\n").unwrap();
        std::fs::write(dir.join("new file.txt"), "n\n").unwrap();
        git(&dir, &["mv", "a.txt", "b.txt"]).unwrap();
        git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
        git(
            &dir,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        )
        .unwrap();
        git(&dir, &["config", "gitflow.branch.develop", "dev"]).unwrap();
        let d = load(&dir, 100).unwrap();
        assert_eq!(d.head_branch.as_deref(), Some("main"));
        assert_eq!(d.default_branches, vec!["origin/main".to_string()]);
        assert_eq!(d.flow.develop.as_deref(), Some("dev"));
        assert!(!d.refs.iter().any(|r| r.name.ends_with("/HEAD")));
        assert_eq!(d.commits.len(), 1);
        let head = d.head_sha.clone().unwrap();
        let tag = d.refs.iter().find(|r| r.kind == RefKind::Tag).unwrap();
        assert_eq!(tag.target, head, "annotated tag is peeled to its commit");
        assert!(
            d.refs
                .iter()
                .any(|r| r.kind == RefKind::Local && r.name == "feature/x")
        );
        let renamed = d.status.staged.iter().find(|f| f.status == 'R').unwrap();
        assert_eq!(renamed.path, "b.txt");
        assert_eq!(renamed.old_path.as_deref(), Some("a.txt"));
        assert!(
            d.status
                .unstaged
                .iter()
                .any(|f| f.path == "new file.txt" && f.status == '?')
        );
    }

    #[test]
    fn unstage_works_before_the_first_commit() {
        let dir = std::env::temp_dir().join(format!("bb-test-unborn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(&dir, &["add", "a.txt"]).unwrap();
        // Edited again after staging: index, worktree and (missing) HEAD all differ.
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        let d = load(&dir, 100).unwrap();
        Op::Unstage(vec!["a.txt".into()])
            .run(&dir, Some(&d))
            .unwrap();
        let d = load(&dir, 100).unwrap();
        assert!(d.status.staged.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "changed\n"
        );
        git(&dir, &["add", "a.txt"]).unwrap();
        Op::UnstageAll.run(&dir, Some(&d)).unwrap();
        assert!(load(&dir, 100).unwrap().status.staged.is_empty());
    }
}
