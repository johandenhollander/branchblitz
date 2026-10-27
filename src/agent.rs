//! The agent conversation of a repository tab: what was said, and a backend that answers in a
//! background thread, streaming events back to the UI. Backends: Claude Code (`claude -p` with
//! stream-json output, run in the repository, read-only tools) and a preview that echoes, for
//! tests and when no agent CLI is installed.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::Value;

/// Who answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    Preview,
    Claude(PathBuf),
}

impl Backend {
    /// Claude Code when `claude` is on the PATH, else the preview.
    pub fn detect() -> Backend {
        find_on_path("claude").map_or(Backend::Preview, Backend::Claude)
    }

    pub fn name(&self) -> &'static str {
        match self {
            Backend::Preview => "Preview",
            Backend::Claude(_) => "Claude Code",
        }
    }
}

/// An executable `name` in an absolute PATH directory (a relative entry would depend on the
/// current directory); on Windows `name.exe` or `name.cmd`.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let names = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        vec![name.to_string()]
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Models to choose from (Claude Code's aliases for the latest of each family): alias, name.
pub const CLAUDE_MODELS: &[(&str, &str)] =
    &[("opus", "Opus"), ("sonnet", "Sonnet"), ("haiku", "Haiku")];

/// A model id as people say it: `claude-opus-5-5` → `Opus 5.5` (a date suffix is dropped).
pub fn model_name(id: &str) -> String {
    let mut parts = id
        .split('-')
        .filter(|p| *p != "claude" && !(p.len() == 8 && p.chars().all(|c| c.is_ascii_digit())));
    let Some(family) = parts.next().filter(|f| !f.is_empty()) else {
        return id.to_string();
    };
    let mut name: String = family[..1].to_uppercase() + &family[1..];
    let version: Vec<&str> = parts.collect();
    if !version.is_empty() {
        name.push(' ');
        name.push_str(&version.join("."));
    }
    name
}

/// Read-only: the agent looks at files; changes go through the app, where you see them.
const CLAUDE_TOOLS: &str = "Read,Grep,Glob";
const SYSTEM_PROMPT: &str = "You are the assistant inside BranchBlitz, a desktop git client. \
The user talks to you from a panel next to the commit graph of this repository (your working \
directory). You can read and search files; you cannot change files or run commands. Keep \
answers short; use Markdown sparingly.";
const APP_PROMPT: &str = "You work inside the app: the user sees this repository in \
BranchBlitz next to this conversation, and you act on it with the `branchblitz` tools (start with \
`state`): fetch, pull and push (`sync`), check out, merge, rebase, branches, stage and commit, \
stash, diffs and conflicts. These tools are how you reach git and the remotes. Prefer doing over \
explaining: when a tool can answer a question, use it instead of asking the user (to see whether \
a remote is reachable, fetch and report what happened). Do what the user asks; ask before larger \
steps they did not ask for.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Agent,
    /// What the agent did (a tool call), shown as a muted line.
    Tool,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

/// What a running backend reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// More text of the agent's answer.
    Text(String),
    Tool(String),
    Error(String),
    /// The backend's conversation id, to continue it with the next question.
    Session(String),
    /// The model that answers (e.g. `claude-opus-5-5`).
    Model(String),
    /// The answer is complete.
    Done,
}

#[derive(Default)]
pub struct Conversation {
    pub messages: Vec<Message>,
    pub input: String,
    /// Set while an answer streams in; setting it to true asks the backend to stop.
    running: Option<Arc<AtomicBool>>,
    session: Option<String>,
    /// The model of the last answer.
    pub model: Option<String>,
    /// When the answer coming in was asked for.
    started: Option<std::time::Instant>,
}

/// Closing the tab stops an answer still coming in (and its process).
impl Drop for Conversation {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Conversation {
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Takes the input as the next question and lets `backend` answer it in `dir`; `emit`
    /// receives the events on a background thread. Nothing happens for an empty input or while
    /// busy.
    pub fn send(
        &mut self,
        backend: &Backend,
        dir: &Path,
        app: Option<crate::live::Attach>,
        model: Option<&str>,
        extra: String,
        emit: impl Fn(Event) + Send + 'static,
    ) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy() {
            return;
        }
        self.input.clear();
        self.messages.push(Message {
            role: Role::User,
            text: text.clone(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        self.running = Some(stop.clone());
        self.started = Some(std::time::Instant::now());
        match backend {
            Backend::Preview => start(text, stop, emit),
            Backend::Claude(cli) => start_claude(
                ClaudeRun {
                    cli: cli.clone(),
                    dir: dir.to_path_buf(),
                    session: self.session.clone(),
                    app,
                    model: model.map(str::to_string),
                    extra,
                },
                text,
                stop,
                emit,
            ),
        }
    }

    /// Forgets the conversation (a new one starts with the next question).
    pub fn clear(&mut self) {
        if !self.busy() {
            self.messages.clear();
            self.session = None;
        }
    }

    /// While an answer comes in: what the agent is doing (its last tool call this turn, writing
    /// text, or thinking) and for how long.
    pub fn status(&self) -> Option<(String, Duration)> {
        let started = self.started.filter(|_| self.busy())?;
        let what = match self.messages.last() {
            Some(m) if m.role == Role::Tool => format!("Working: {}", m.text),
            Some(m) if m.role == Role::Agent => "Writing…".to_string(),
            _ => "Thinking…".to_string(),
        };
        Some((what, started.elapsed()))
    }

    pub fn stop(&mut self) {
        if let Some(stop) = &self.running {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// Applies an event from the backend.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Text(t) => match self.messages.last_mut() {
                Some(m) if m.role == Role::Agent => m.text.push_str(&t),
                _ => self.messages.push(Message {
                    role: Role::Agent,
                    text: t,
                }),
            },
            Event::Tool(t) => self.messages.push(Message {
                role: Role::Tool,
                text: t,
            }),
            Event::Error(t) => self.messages.push(Message {
                role: Role::Error,
                text: t,
            }),
            Event::Session(id) => self.session = Some(id),
            Event::Model(m) => self.model = Some(m),
            Event::Done => {
                self.running = None;
                self.started = None;
            }
        }
    }
}

/// The preview backend: answers word by word, so the panel can be tried before an agent CLI
/// is connected.
fn start(question: String, stop: Arc<AtomicBool>, emit: impl Fn(Event) + Send + 'static) {
    std::thread::spawn(move || {
        let answer = format!(
            "**No agent is connected yet.** Once one is:\n\n- it answers here\n- it can use \
             the app, e.g. `Pull`\n\nYou asked: \"{question}\""
        );
        emit(Event::Tool("preview: no agent CLI connected".into()));
        for (i, word) in answer.split(' ').enumerate() {
            if stop.load(Ordering::Relaxed) {
                emit(Event::Error("Stopped".into()));
                break;
            }
            emit(Event::Text(if i == 0 {
                word.to_string()
            } else {
                format!(" {word}")
            }));
            std::thread::sleep(Duration::from_millis(15));
        }
        emit(Event::Done);
    });
}

/// What a `claude -p` run needs besides the question.
struct ClaudeRun {
    cli: PathBuf,
    /// The repository: the working directory.
    dir: PathBuf,
    /// The conversation to continue.
    session: Option<String>,
    /// The app's tools, when offered.
    app: Option<crate::live::Attach>,
    model: Option<String>,
    /// Appended to the system prompt (access level, the user's own instructions).
    extra: String,
}

/// Runs `claude -p` in the repository with the question on stdin and turns its stream-json
/// output into events. Stopping kills the process.
fn start_claude(
    run: ClaudeRun,
    question: String,
    stop: Arc<AtomicBool>,
    emit: impl Fn(Event) + Send + 'static,
) {
    let ClaudeRun {
        cli,
        dir,
        session,
        app,
        model,
        extra,
    } = run;
    std::thread::spawn(move || {
        // The app's own tools when it offers them; no other MCP servers.
        let (mcp, mut prompt) = match &app {
            Some(a) => (a.mcp_config(), format!("{SYSTEM_PROMPT} {APP_PROMPT}")),
            None => (
                r#"{"mcpServers":{}}"#.to_string(),
                SYSTEM_PROMPT.to_string(),
            ),
        };
        if !extra.trim().is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(extra.trim());
        }
        let mut cmd = Command::new(&cli);
        crate::child_env(&mut cmd);
        cmd.args([
            "-p",
            "--input-format",
            "text",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--tools",
            CLAUDE_TOOLS,
            "--strict-mcp-config",
            "--mcp-config",
            &mcp,
            // The app's tools need no permission prompt: what they change, the user sees, and
            // destructive ones ask in the app.
            "--allowedTools",
            "mcp__branchblitz",
            "--append-system-prompt",
            &prompt,
            // Only the user's own settings: a repository's .claude/settings.json could define
            // hooks (commands) and is not trusted just because it was cloned.
            "--setting-sources",
            "user",
        ]);
        if let Some(id) = &session {
            cmd.args(["--resume", id]);
        }
        if let Some(m) = &model {
            cmd.args(["--model", m]);
        }
        cmd.current_dir(&dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                emit(Event::Error(format!("Cannot start {}: {e}", cli.display())));
                emit(Event::Done);
                return;
            }
        };
        // The question goes in on stdin, so nothing in it can be taken for an option.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(question.as_bytes());
        }
        let (tx, rx) = mpsc::channel();
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            // Bytes, not `lines()`: an invalid UTF-8 line must not stop the reading (the pipe
            // would fill up and the process hang).
            let mut out = std::io::BufReader::new(stdout.unwrap());
            let mut buf = Vec::new();
            while out.read_until(b'\n', &mut buf).is_ok_and(|n| n > 0) {
                if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                    break;
                }
                buf.clear();
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_reader = child.stderr.take().map(|mut err| {
            let stderr = stderr.clone();
            std::thread::spawn(move || {
                let mut s = String::new();
                let _ = err.read_to_string(&mut s);
                *stderr.lock().unwrap_or_else(|e| e.into_inner()) = s;
            })
        });
        let mut state = StreamState::default();
        let mut stopped = false;
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    for e in parse_line(&line, &mut state) {
                        emit(e);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if stop.load(Ordering::Relaxed) {
                let _ = child.kill();
                stopped = true;
                break;
            }
        }
        // Output ended: wait for the exit, still listening to Stop.
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Ok(st),
                Ok(None) if stop.load(Ordering::Relaxed) && !stopped => {
                    let _ = child.kill();
                    stopped = true;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => break Err(e),
            }
        };
        if stopped {
            emit(Event::Error("Stopped".into()));
        } else if !state.finished && !status.is_ok_and(|s| s.success()) {
            // The reader may still be busy with the last of stderr; give it a moment (not
            // forever: a leftover child process can keep the pipe open).
            if let Some(r) = &stderr_reader {
                let waited = std::time::Instant::now();
                while !r.is_finished() && waited.elapsed() < Duration::from_secs(1) {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let err = stderr.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            emit(Event::Error(format!(
                "{} stopped unexpectedly. {last}",
                cli.display()
            )));
        }
        emit(Event::Done);
    });
}

/// Where in the stream-json output we are.
#[derive(Default)]
struct StreamState {
    /// The current message's text came in as deltas (so skip it in the full message).
    streamed: bool,
    /// The last thing shown was text: a new message's text starts a new paragraph.
    after_text: bool,
    new_message: bool,
    /// The `result` line arrived.
    finished: bool,
}

/// The events in one line of `claude -p --output-format stream-json` output.
fn parse_line(line: &str, st: &mut StreamState) -> Vec<Event> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let text = |t: &str, st: &mut StreamState, out: &mut Vec<Event>| {
        if t.is_empty() {
            return;
        }
        let gap = std::mem::take(&mut st.new_message) && st.after_text;
        out.push(Event::Text(if gap {
            format!("\n\n{t}")
        } else {
            t.to_string()
        }));
        st.after_text = true;
    };
    match v["type"].as_str() {
        Some("system") if v["subtype"] == "init" => {
            if let Some(id) = v["session_id"].as_str() {
                out.push(Event::Session(id.to_string()));
            }
            if let Some(m) = v["model"].as_str() {
                out.push(Event::Model(m.to_string()));
            }
        }
        Some("stream_event") => {
            let e = &v["event"];
            match e["type"].as_str() {
                Some("message_start") => {
                    st.streamed = false;
                    st.new_message = true;
                }
                Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                    st.streamed = true;
                    text(e["delta"]["text"].as_str().unwrap_or(""), st, &mut out);
                }
                _ => {}
            }
        }
        Some("assistant") => {
            for c in v["message"]["content"].as_array().into_iter().flatten() {
                match c["type"].as_str() {
                    Some("text") if !st.streamed => {
                        text(c["text"].as_str().unwrap_or(""), st, &mut out)
                    }
                    Some("tool_use") => {
                        out.push(Event::Tool(describe_tool(
                            c["name"].as_str().unwrap_or("tool"),
                            &c["input"],
                        )));
                        st.after_text = false;
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            st.finished = true;
            if v["is_error"] == true {
                let why = v["result"]
                    .as_str()
                    .or(v["subtype"].as_str())
                    .unwrap_or("The agent failed");
                out.push(Event::Error(why.to_string()));
            }
        }
        _ => {}
    }
    out
}

/// A tool call in a few words: its name and its main argument.
fn describe_tool(name: &str, input: &Value) -> String {
    // MCP tools are called `mcp__<server>__<tool>`.
    let short = name.rsplit("__").next().unwrap_or(name);
    let arg = ["file_path", "pattern", "label", "path", "query", "command"]
        .iter()
        .find_map(|k| input[*k].as_str())
        .or_else(|| {
            input
                .as_object()
                .and_then(|o| o.values().find_map(Value::as_str))
        });
    match arg {
        Some(a) => {
            let a: String = a.chars().take(80).collect();
            format!("{short} {a}")
        }
        None => short.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn streams_an_answer_and_can_be_stopped() {
        let mut c = Conversation::default();
        let (tx, rx) = mpsc::channel();
        c.input = "  hello  ".into();
        let t = tx.clone();
        let dir = std::env::temp_dir();
        c.send(
            &Backend::Preview,
            &dir,
            None,
            None,
            String::new(),
            move |e| t.send(e).unwrap(),
        );
        assert!(c.busy() && c.input.is_empty());
        assert_eq!(c.messages[0].text, "hello");
        assert_eq!(c.status().map(|s| s.0).as_deref(), Some("Thinking…"));
        c.input = "ignored while busy".into();
        c.send(&Backend::Preview, &dir, None, None, String::new(), |_| {});
        assert_eq!(c.messages.len(), 1);
        while c.busy() {
            c.apply(rx.recv().unwrap());
        }
        assert_eq!(
            c.messages.len(),
            3,
            "a tool line, then one agent message appended to"
        );
        assert_eq!(c.messages[1].role, Role::Tool);
        assert!(c.messages[2].text.ends_with("\"hello\""));
        assert_eq!(c.status(), None, "done");

        c.input = "again".into();
        let t = tx.clone();
        c.send(
            &Backend::Preview,
            &dir,
            None,
            None,
            String::new(),
            move |e| t.send(e).unwrap(),
        );
        c.stop();
        while c.busy() {
            c.apply(rx.recv().unwrap());
        }
        assert_eq!(c.messages.last().unwrap().role, Role::Error);
    }

    /// Lines as Claude Code 2.1 writes them: session, a tool call, then text streamed in
    /// deltas (and repeated in the full message, which must not double it).
    #[test]
    fn parses_claude_stream_json() {
        let lines = [
            r#"{"type":"system","subtype":"hook_started","session_id":"s1"}"#,
            r#"{"type":"system","subtype":"init","session_id":"s1","model":"claude-opus-5-5"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Let me look."}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me look."},{"type":"tool_use","name":"Glob","input":{"pattern":"*.rs"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"a.rs"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"One "}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"file."}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"One file."}]}}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"One file.","session_id":"s1"}"#,
            "not json",
        ];
        let mut st = StreamState::default();
        let events: Vec<Event> = lines.iter().flat_map(|l| parse_line(l, &mut st)).collect();
        assert_eq!(
            events,
            [
                Event::Session("s1".into()),
                Event::Model("claude-opus-5-5".into()),
                Event::Text("Let me look.".into()),
                Event::Tool("Glob *.rs".into()),
                Event::Text("One ".into()),
                Event::Text("file.".into()),
                Event::Text("\n\nDone.".into()),
            ]
        );
        assert!(st.finished);
        let mut st = StreamState::default();
        assert_eq!(
            parse_line(
                r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#,
                &mut st
            ),
            [Event::Error("error_max_turns".into())]
        );
        assert_eq!(
            describe_tool(
                "mcp__branchblitz__click",
                &serde_json::json!({"label": "Pull"})
            ),
            "click Pull"
        );
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("claude-haiku-5-5"), "Haiku 5.5");
        assert_eq!(
            describe_tool("Read", &serde_json::json!({"file_path": "/a/b.rs"})),
            "Read /a/b.rs"
        );
    }

    /// Writes an executable script for a test to run. A child process writes it: a write handle
    /// in this process can leak into a child another test forks meanwhile, and running the
    /// script then fails with "Text file busy".
    #[cfg(unix)]
    pub(crate) fn fake_script(path: &Path, text: &str) {
        let mut sh = Command::new("sh")
            .args(["-c", "cat > \"$0\" && chmod 755 \"$0\""])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        sh.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        assert!(sh.wait().unwrap().success());
    }

    /// The process side, with a fake `claude`: Stop ends a hanging answer quickly, and a crash
    /// shows its last stderr line.
    #[cfg(unix)]
    #[test]
    fn claude_process_stops_and_reports_failures() {
        let dir = std::env::temp_dir().join(format!("bb-fake-claude-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = |name: &str, body: &str| {
            let p = dir.join(name);
            fake_script(&p, &format!("#!/bin/sh\ncat > /dev/null\n{body}\n"));
            Backend::Claude(p)
        };
        let run = |backend: &Backend, stop_after_text: bool| {
            let mut c = Conversation::default();
            let (tx, rx) = mpsc::channel();
            c.input = "q".into();
            c.send(backend, &dir, None, None, String::new(), move |e| {
                tx.send(e).unwrap()
            });
            let started = std::time::Instant::now();
            while c.busy() {
                let e = rx.recv_timeout(Duration::from_secs(10)).expect("an event");
                let text = matches!(e, Event::Text(_));
                c.apply(e);
                if text && stop_after_text {
                    c.stop();
                }
            }
            (c, started.elapsed())
        };

        let hang = script(
            "hang",
            r#"echo '{"type":"system","subtype":"init","session_id":"s9"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}'
sleep 30"#,
        );
        let (c, took) = run(&hang, true);
        assert!(took < Duration::from_secs(5), "stopped in {took:?}");
        assert_eq!(c.session.as_deref(), Some("s9"), "{:?}", c.messages);
        assert_eq!(c.messages.last().unwrap().text, "Stopped");

        let crash = script("crash", "echo 'boom: no credentials' >&2\nexit 3");
        let (c, _) = run(&crash, false);
        let last = c.messages.last().unwrap();
        assert_eq!(last.role, Role::Error);
        assert!(last.text.contains("boom: no credentials"), "{}", last.text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against the installed Claude Code (costs a little): an answer, then a follow-up in the
    /// same session. `cargo test claude_live -- --ignored`.
    #[test]
    #[ignore]
    fn claude_live() {
        let Backend::Claude(cli) = Backend::detect() else {
            panic!("claude not on PATH")
        };
        let backend = Backend::Claude(cli);
        let mut c = Conversation::default();
        let (tx, rx) = mpsc::channel();
        let ask = |c: &mut Conversation, q: &str| {
            c.input = q.into();
            let t = tx.clone();
            c.send(
                &backend,
                &std::env::temp_dir(),
                None,
                Some("haiku"),
                String::new(),
                move |e| t.send(e).unwrap(),
            );
            while c.busy() {
                c.apply(rx.recv_timeout(Duration::from_secs(180)).unwrap());
            }
        };
        ask(&mut c, "Reply with exactly the word: pong");
        assert!(c.session.is_some(), "{:?}", c.messages);
        assert!(
            c.model.as_deref().is_some_and(|m| m.contains("haiku")),
            "{:?}",
            c.model
        );
        ask(
            &mut c,
            "Which word did you reply with just now? Answer with that word only.",
        );
        let answers: Vec<_> = c
            .messages
            .iter()
            .filter(|m| m.role == Role::Agent)
            .collect();
        assert_eq!(answers.len(), 2, "{:?}", c.messages);
        assert!(
            answers
                .iter()
                .all(|m| m.text.to_lowercase().contains("pong")),
            "{:?}",
            c.messages
        );
    }
}
