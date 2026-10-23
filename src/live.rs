//! The running app as an MCP server for its own agent panel. The app listens on a Unix socket
//! (in the user's runtime directory); `branchblitz mcp --attach` connects stdio to it, so an
//! agent CLI can use it like any MCP server. Each connection first sends a handshake with a
//! secret token and the id of the tab it works on; tool calls are then handed to the UI thread
//! (`Request`), which acts like the user would and answers once the result is visible.
//!
//! Unix only (Linux, macOS): elsewhere `Live::start` gives `None` and the agent works without
//! the app's tools.

#[cfg(unix)]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
#[cfg(unix)]
use std::time::Duration;

use eframe::egui;
use serde_json::{Value, json};

/// A tool call for the UI thread; `reply` gets the text for the agent (Err: a tool error).
pub struct Request {
    pub tab: u64,
    pub tool: String,
    pub args: Value,
    pub reply: Sender<Result<String, String>>,
}

pub struct Live {
    pub socket: PathBuf,
    pub token: String,
    pub requests: Receiver<Request>,
}

impl Drop for Live {
    fn drop(&mut self) {
        if let Some(dir) = self.socket.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// How the agent CLI starts the bridge to this app for tab `tab`: command and arguments.
pub struct Attach {
    pub exe: PathBuf,
    pub socket: PathBuf,
    pub token: String,
    pub tab: u64,
}

impl Attach {
    /// `--mcp-config` JSON for Claude Code.
    pub fn mcp_config(&self) -> String {
        json!({"mcpServers": {"branchblitz": {
            "command": self.exe,
            "args": ["mcp", "--attach", self.socket, "--token", self.token, "--tab", self.tab.to_string()],
        }}})
        .to_string()
    }
}

impl Live {
    #[cfg(not(unix))]
    pub fn start(_ctx: egui::Context) -> Option<Live> {
        None
    }

    /// Listens on `branchblitz-<pid>-<n>/agent.sock` in the runtime directory; `None` if that fails (the
    /// agent then works without the app's tools).
    #[cfg(unix)]
    pub fn start(ctx: egui::Context) -> Option<Live> {
        use std::os::unix::fs::DirBuilderExt;
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|d| d.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        sweep(&base);
        // A directory only the user can enter, created fresh: the socket is private from the
        // moment it exists.
        // Numbered too: one process may run more than one (tests do).
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = base.join(format!("branchblitz-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
        let socket = dir.join("agent.sock");
        let listener = UnixListener::bind(&socket).ok()?;
        let token = random_token()?;
        let (tx, rx) = mpsc::channel();
        let expected = token.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (tx, ctx, expected) = (tx.clone(), ctx.clone(), expected.clone());
                std::thread::spawn(move || serve(stream, &expected, &tx, &ctx));
            }
        });
        Some(Live {
            socket,
            token,
            requests: rx,
        })
    }
}

#[cfg(unix)]
/// Removes the directories of earlier runs that ended without cleaning up (crash, kill).
fn sweep(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("branchblitz-"))
            .and_then(|rest| rest.split('-').next())
            .and_then(|p| p.parse::<u32>().ok())
        else {
            continue;
        };
        if pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(unix)]
fn random_token() -> Option<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut b)
        .ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

#[cfg(unix)]
/// One agent connection: handshake, then JSON-RPC lines.
fn serve(stream: UnixStream, token: &str, tx: &Sender<Request>, ctx: &egui::Context) {
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut lines = BufReader::new(stream).split(b'\n');
    let Some(Ok(hello)) = lines.next() else {
        return;
    };
    let hello: Value = serde_json::from_slice(&hello).unwrap_or_default();
    let Some(tab) = hello["tab"].as_u64() else {
        return;
    };
    if hello["token"].as_str() != Some(token) {
        return;
    }
    for line in lines {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        let Some(reply) = handle(&msg, tab, tx, ctx) else {
            continue;
        };
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

#[cfg(unix)]
const INSTRUCTIONS: &str = "\
These tools operate BranchBlitz, the git client the user is looking at, on the repository they \
have open (after a switch to another worktree, on that worktree). The user sees every action happen. Start with `state`. Actions run real \
git commands and answer when they are done, with what the user now sees. Depending on the access \
the user gave you, actions that publish, rewrite or throw away work (push, amend, resets, \
discarding changes, dropping a stash, aborting) may not run directly: they then ask the user in a \
red bar and only run when the user clicks it; the answer says so.";

#[cfg(unix)]
fn handle(msg: &Value, tab: u64, tx: &Sender<Request>, ctx: &egui::Context) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str)?;
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "branchblitz", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let (reply, answer) = mpsc::channel();
            let req = Request {
                tab,
                tool: params["name"].as_str().unwrap_or("").to_string(),
                args: params.get("arguments").cloned().unwrap_or(json!({})),
                reply,
            };
            let outcome = if tx.send(req).is_ok() {
                ctx.request_repaint();
                answer
                    .recv_timeout(Duration::from_secs(600))
                    .unwrap_or_else(|_| Err("BranchBlitz did not answer in time".into()))
            } else {
                Err("BranchBlitz is closing".into())
            };
            let (text, error) = match outcome {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            };
            json!({"content": [{"type": "text", "text": text}], "isError": error})
        }
        _ => {
            return Some(json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601, "message": format!("unknown method {method}")}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

#[cfg(unix)]
fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": props, "required": required},
    })
}

#[cfg(unix)]
fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    vec![
        tool(
            "state",
            "What BranchBlitz shows: current branch, branches with ahead/behind, tags, working copy \
             (staged, unstaged, conflicted), repository state (merging, rebasing), recent commits.",
            json!({"commits": {"type": "integer", "description": "How many recent commits (default 30)."}}),
            &[],
        ),
        tool(
            "select_commit",
            "Select a commit in the graph; its details show on the right.",
            json!({"sha": s("Commit hash (or a unique prefix) of a loaded commit.")}),
            &["sha"],
        ),
        tool(
            "show_diff",
            "Open the diff of a file (in a commit, staged, or in the working copy) and return it.",
            json!({
                "path": s("Path in the repository."),
                "commit": s("Commit hash; omit for the working copy."),
                "staged": {"type": "boolean", "description": "The staged version (working copy only)."},
            }),
            &["path"],
        ),
        tool(
            "checkout",
            "Check out a local or remote branch (a remote one gets a local branch), or a tag \
             (detached). A branch checked out in another worktree (git allows one) makes BranchBlitz show \
             that worktree instead, in the same window.",
            json!({"name": s("Branch or tag, e.g. develop or origin/feature/x.")}),
            &["name"],
        ),
        tool(
            "merge",
            "Merge a branch or commit into the current branch. Conflicts are left for resolving.",
            json!({
                "name": s("Branch, tag or commit."),
                "mode": {"type": "string", "enum": ["default", "no_ff", "ff_only", "squash"]},
            }),
            &["name"],
        ),
        tool(
            "rebase",
            "Rebase the current branch onto a branch or commit.",
            json!({"onto": s("Branch, tag or commit.")}),
            &["onto"],
        ),
        tool(
            "branch",
            "Create, delete or rename a local branch, or fast-forward one to its upstream.",
            json!({
                "action": {"type": "string", "enum": ["create", "delete", "rename", "fast_forward"]},
                "name": s("The branch."),
                "at": s("create: start point (default HEAD)."),
                "checkout": {"type": "boolean", "description": "create: check it out (default true)."},
                "new_name": s("rename: the new name."),
            }),
            &["action", "name"],
        ),
        tool(
            "sync",
            "Fetch all remotes, pull the current branch, or push it (force asks the user).",
            json!({
                "action": {"type": "string", "enum": ["fetch", "pull", "push", "force_push"]},
                "pull_mode": {"type": "string", "enum": ["merge", "ff_only", "rebase"]},
            }),
            &["action"],
        ),
        tool(
            "changes",
            "Stage or unstage files, discard a file's changes (asks the user), or commit what is staged.",
            json!({
                "action": {"type": "string", "enum": ["stage", "unstage", "stage_all", "unstage_all", "discard", "commit"]},
                "paths": {"type": "array", "items": {"type": "string"}},
                "message": s("commit: the message (summary, blank line, body)."),
                "amend": {"type": "boolean", "description": "commit: amend the last commit."},
            }),
            &["action"],
        ),
        tool(
            "commit_action",
            "On a commit: cherry-pick, revert, reset the current branch to it (hard asks the \
             user), or tag it.",
            json!({
                "action": {"type": "string", "enum": ["cherry_pick", "revert", "reset_soft", "reset_mixed", "reset_hard", "tag"]},
                "sha": s("The commit."),
                "tag": s("tag: the tag name."),
            }),
            &["action", "sha"],
        ),
        tool(
            "stash",
            "Stash the working copy, or pop, apply or drop (asks the user) a stash.",
            json!({
                "action": {"type": "string", "enum": ["save", "pop", "apply", "drop"]},
                "name": s("pop/apply/drop: e.g. stash@{0} (default the latest)."),
            }),
            &["action"],
        ),
        tool(
            "read_conflict",
            "Open a conflicted file in the conflict view and return its conflict blocks, each \
             with the user's version (mine) and the incoming one.",
            json!({"path": s("A conflicted file.")}),
            &["path"],
        ),
        tool(
            "resolve_conflict",
            "Resolve a conflicted file: one choice per block (mine, incoming or both, in the \
             order of read_conflict), or a whole side. The file is saved and staged.",
            json!({
                "path": s("A conflicted file."),
                "choices": {"type": "array", "items": {"type": "string", "enum": ["mine", "incoming", "both"]}},
                "side": {"type": "string", "enum": ["mine", "incoming"], "description": "Take a whole side instead of choices."},
            }),
            &["path"],
        ),
        tool(
            "operation",
            "Continue or abort the merge, rebase or cherry-pick in progress.",
            json!({"action": {"type": "string", "enum": ["continue", "abort"]}}),
            &["action"],
        ),
    ]
}

#[cfg(unix)]
/// `branchblitz mcp --attach <socket> --token <token> --tab <id>`: connects stdio to the
/// running app. Returns when either side closes.
pub fn attach(args: &[String]) -> Result<(), String> {
    let arg = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
            .ok_or_else(|| format!("missing {flag}"))
    };
    let socket = arg("--attach")?;
    let token = arg("--token")?;
    let tab: u64 = arg("--tab")?.parse().map_err(|_| "bad --tab")?;
    let mut stream = UnixStream::connect(Path::new(&socket))
        .map_err(|e| format!("cannot reach BranchBlitz at {socket}: {e}"))?;
    writeln!(stream, "{}", json!({"token": token, "tab": tab})).map_err(|e| e.to_string())?;
    let mut from_app = stream.try_clone().map_err(|e| e.to_string())?;
    let to_client = std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_app, &mut std::io::stdout().lock());
    });
    let _ = std::io::copy(&mut std::io::stdin().lock(), &mut stream);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = to_client.join();
    Ok(())
}

#[cfg(not(unix))]
pub fn attach(_args: &[String]) -> Result<(), String> {
    Err("the agent's app tools need Unix sockets (Linux, macOS)".into())
}
