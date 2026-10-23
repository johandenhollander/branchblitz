//! `branchblitz mcp`: a Model Context Protocol server (stdio) that runs the real app without a
//! window, so an AI assistant can open repositories, click, type, read state and take screenshots.
//!
//! The app runs inside an `egui_kittest` harness: widgets are found through the accessibility tree
//! and input is injected as egui events. Nothing is shown on screen and the user's own settings
//! (open tabs, theme) are neither read nor written.

use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;
use serde_json::{Value, json};

use crate::app::BranchBlitz;
use crate::theme::ThemeChoice;

const DEMO_SCRIPT: &str = include_str!("../scripts/demo-repo.sh");
const DEFAULT_SIZE: (f32, f32) = (1400.0, 860.0);

const INSTRUCTIONS: &str = "\
BranchBlitz is a desktop git client. This server runs it without a window and lets you use it like \
a person would. Typical loop: open_repo (or demo_repo for a throwaway repository) -> screenshot -> \
widgets to find labels -> click / type_text / key -> wait_idle -> screenshot or state. \
Right-click (button=right) a branch in the sidebar or a label in the graph for branch actions such as \
merging it into the current branch. Actions run real git commands on the repository, including push. \
Destructive actions (force push, deleting remote branches, hard reset, discarding changes) ask first \
in a red alert bar under the toolbar; its button only works with confirm_dangerous=true, which you must only pass when the \
user asked for exactly that action.";

/// Protocol versions this server has been checked against, newest first.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

pub fn run() {
    let mut server = Server::default();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    // Split on bytes so a line with invalid UTF-8 becomes a parse error instead of ending the loop.
    for line in stdin.lock().split(b'\n') {
        let Ok(line) = line else { break };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some(reply) = server.handle_line(&line)
            && writeln!(stdout, "{reply}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            break;
        }
    }
}

#[derive(Default)]
pub struct Server {
    session: Option<Harness<'static, BranchBlitz>>,
    /// Demo repositories created by this server; removed when it stops.
    demo_dirs: Vec<PathBuf>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.session = None;
        for dir in &self.demo_dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

impl Server {
    /// Handles one line of input from the client.
    pub fn handle_line(&mut self, line: &[u8]) -> Option<Value> {
        match serde_json::from_slice::<Value>(line) {
            Ok(Value::Array(_)) => Some(rpc_error(
                Value::Null,
                -32600,
                "batches are not supported; send one message per line",
            )),
            Ok(msg) => self.handle(&msg),
            Err(e) => Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}"))),
        }
    }

    /// Handles one JSON-RPC message; notifications get no reply.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        // Notifications have no id; responses (we never send requests) have no method.
        let id = msg.get("id").cloned()?;
        let method = msg.get("method").and_then(Value::as_str)?;
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .filter(|v| PROTOCOL_VERSIONS.contains(v))
                    .unwrap_or(PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "branchblitz", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tool_list()}),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                self.call(name, &args)
            }
            _ => return Some(rpc_error(id, -32601, &format!("unknown method {method}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// Runs a tool; failures (including panics in the UI) become an `isError` result.
    pub fn call(&mut self, name: &str, args: &Value) -> Value {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.dispatch(name, args)));
        if let Some(h) = &mut self.session {
            h.state_mut().set_confirm_danger(false);
        }
        let (content, error) = match outcome {
            Ok(Ok(content)) => (content, false),
            Ok(Err(e)) => (vec![text(e)], true),
            Err(panic) => {
                // The harness may be in a broken state; start fresh on the next call.
                self.session = None;
                let why = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                (
                    vec![text(format!(
                        "BranchBlitz crashed ({why}); the session was reset, open the repo again"
                    ))],
                    true,
                )
            }
        };
        json!({"content": content, "isError": error})
    }

    fn harness(&mut self) -> &mut Harness<'static, BranchBlitz> {
        self.session.get_or_insert_with(|| {
            let mut h = Harness::builder()
                .with_size(vec2(DEFAULT_SIZE.0, DEFAULT_SIZE.1))
                .with_theme(egui::Theme::Dark)
                .with_step_dt(1.0 / 30.0)
                .with_accessibility_check(false)
                .allow_missing_glyphs()
                .build_eframe(|cc| {
                    let mut app = BranchBlitz::new(cc, None);
                    app.set_headless();
                    app
                });
            h.run_steps(2);
            h
        })
    }

    fn dispatch(&mut self, name: &str, args: &Value) -> Result<Vec<Value>, String> {
        let h = self.harness();
        match name {
            "open_repo" => {
                let path = str_arg(args, "path")?;
                h.state_mut().open_for_automation(&expand_home(path))?;
                settle(h, Duration::from_secs(20));
            }
            "demo_repo" => {
                let dir = demo_repo()?;
                self.demo_dirs.push(dir.clone());
                let h = self.harness();
                let work = dir.join("work");
                h.state_mut().open_for_automation(&work)?;
                settle(h, Duration::from_secs(20));
                return Ok(vec![text(format!(
                    "Created and opened a throwaway demo repository at {} (remote: {}). {}",
                    work.display(),
                    dir.join("origin.git").display(),
                    h.state().summary()
                ))]);
            }
            "state" => {
                let n = args.get("commits").and_then(Value::as_u64).unwrap_or(20) as usize;
                let v = h.state().snapshot(n);
                return Ok(vec![text(
                    serde_json::to_string_pretty(&v).unwrap_or_default(),
                )]);
            }
            "screenshot" => return Ok(vec![screenshot(h)?]),
            "widgets" => return Ok(vec![text(widgets(h, args))]),
            "click" | "hover" => {
                let pos = target(h, args)?;
                let button = match args.get("button").and_then(Value::as_str) {
                    None | Some("left") => PointerButton::Primary,
                    Some("right") => PointerButton::Secondary,
                    Some("middle") => PointerButton::Middle,
                    Some(b) => return Err(format!("unknown button {b}")),
                };
                let mods = modifiers(args)?;
                let double = args.get("double").and_then(Value::as_bool).unwrap_or(false);
                let allow = args.get("confirm_dangerous").and_then(Value::as_bool) == Some(true);
                if allow && double {
                    return Err("confirm_dangerous cannot be combined with double".into());
                }
                h.event(Event::PointerMoved(pos));
                h.step();
                if name == "click" {
                    // Only this click may confirm; `call` clears the flag again whatever happens.
                    h.state_mut().set_confirm_danger(allow);
                    for _ in 0..if double { 2 } else { 1 } {
                        for pressed in [true, false] {
                            h.event(Event::PointerButton {
                                pos,
                                button,
                                pressed,
                                modifiers: mods,
                            });
                        }
                        h.step();
                    }
                    h.step();
                    h.state_mut().set_confirm_danger(false);
                }
                settle(h, Duration::from_secs(2));
            }
            "type_text" => {
                if args.get("label").is_some() || args.get("x").is_some() {
                    let pos = target(h, args)?;
                    h.event(Event::PointerMoved(pos));
                    for pressed in [true, false] {
                        h.event(Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        });
                    }
                    h.step();
                }
                h.event(Event::Text(str_arg(args, "text")?.to_owned()));
                settle(h, Duration::from_secs(2));
            }
            "key" => {
                let name = str_arg(args, "key")?;
                let key = egui::Key::from_name(name).ok_or_else(|| {
                    format!("unknown key {name}; use names like Enter, Escape, ArrowDown, Tab, A")
                })?;
                h.key_press_modifiers(modifiers(args)?, key);
                settle(h, Duration::from_secs(2));
            }
            "scroll" => {
                let pos = if args.get("label").is_some() || args.get("x").is_some() {
                    target(h, args)?
                } else {
                    Pos2::new(DEFAULT_SIZE.0 / 2.0, DEFAULT_SIZE.1 / 2.0)
                };
                let num = |k: &str| args.get(k).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                h.event(Event::PointerMoved(pos));
                h.event(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    // Positive `dy` scrolls down (shows later content), like a scroll bar.
                    delta: vec2(-num("dx"), -num("dy")),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: Modifiers::NONE,
                });
                settle(h, Duration::from_secs(2));
            }
            "resize" => {
                let w = args
                    .get("width")
                    .and_then(Value::as_f64)
                    .ok_or("width is required")?;
                let ht = args
                    .get("height")
                    .and_then(Value::as_f64)
                    .ok_or("height is required")?;
                if !(400.0..=2560.0).contains(&w) || !(300.0..=1600.0).contains(&ht) {
                    return Err("size out of range (400..2560 x 300..1600)".into());
                }
                h.set_size(vec2(w as f32, ht as f32));
                settle(h, Duration::from_secs(2));
            }
            "set_theme" => {
                let choice = match str_arg(args, "theme")? {
                    "dark" => ThemeChoice::Dark,
                    "light" => ThemeChoice::Light,
                    t => return Err(format!("unknown theme {t}; use dark or light")),
                };
                h.state_mut().set_theme_choice(choice);
                settle(h, Duration::from_secs(1));
            }
            "wait_idle" => {
                let ms = match args.get("timeout_ms") {
                    None => 30_000,
                    Some(v) => v.as_u64().ok_or("timeout_ms must be a whole number")?,
                };
                if !settle(h, Duration::from_millis(ms.min(600_000))) {
                    return Err(format!("still busy after {ms} ms: {}", h.state().summary()));
                }
            }
            _ => return Err(format!("unknown tool {name}")),
        }
        let mut out = vec![text(h.state().summary())];
        if args
            .get("screenshot")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            out.push(screenshot(h)?);
        }
        Ok(out)
    }
}

/// Steps the UI until background work is done (or `max` passes). Returns whether it is idle.
fn settle(h: &mut Harness<'static, BranchBlitz>, max: Duration) -> bool {
    let start = Instant::now();
    h.run_steps(2);
    loop {
        h.step();
        if !h.state().is_busy() {
            // A few more frames so menus, layout and results of the last message are drawn.
            h.run_steps(3);
            return true;
        }
        if start.elapsed() >= max {
            return false;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn text(s: impl Into<String>) -> Value {
    json!({"type": "text", "text": s.into()})
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is required"))
}

fn modifiers(args: &Value) -> Result<Modifiers, String> {
    let mut m = Modifiers::NONE;
    for v in args
        .get("modifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match v.as_str().unwrap_or("") {
            "ctrl" => {
                m.ctrl = true;
                m.command = true;
            }
            "shift" => m.shift = true,
            "alt" => m.alt = true,
            other => return Err(format!("unknown modifier {other}; use ctrl, shift or alt")),
        }
    }
    Ok(m)
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

/// A visible widget from the accessibility tree.
struct Widget {
    role: String,
    label: String,
    value: Option<String>,
    rect: Rect,
    disabled: bool,
    focused: bool,
}

fn collect_widgets(h: &Harness<'static, BranchBlitz>) -> Vec<Widget> {
    let ppp = h.ctx.pixels_per_point();
    let screen = h.ctx.content_rect();
    h.root()
        .children_recursive()
        .filter_map(|n| {
            let a = n.accesskit_node();
            let label = a.label().unwrap_or_default();
            let value = a.value().filter(|v| !v.is_empty());
            let role = format!("{:?}", a.role());
            // Text fields often have only a hint text, which accesskit does not expose; keep them.
            let unnamed = label.is_empty() && value.is_none() && !role.ends_with("TextInput");
            if a.is_hidden() || unnamed {
                return None;
            }
            let b = a.bounding_box()?;
            let rect = Rect::from_min_max(
                Pos2::new(b.x0 as f32 / ppp, b.y0 as f32 / ppp),
                Pos2::new(b.x1 as f32 / ppp, b.y1 as f32 / ppp),
            );
            if !rect.intersects(screen) || rect.area() <= 0.0 {
                return None;
            }
            Some(Widget {
                role,
                label,
                value,
                rect,
                disabled: a.is_disabled(),
                focused: a.is_focused(),
            })
        })
        .collect()
}

fn describe(i: usize, w: &Widget) -> String {
    let mut s = format!(
        "[{i}] {} {:?} at ({:.0},{:.0}) {:.0}x{:.0}",
        w.role,
        w.label,
        w.rect.min.x,
        w.rect.min.y,
        w.rect.width(),
        w.rect.height()
    );
    if let Some(v) = &w.value {
        s += &format!(" value={v:?}");
    }
    if w.disabled {
        s += " disabled";
    }
    if w.focused {
        s += " focused";
    }
    s
}

fn widgets(h: &Harness<'static, BranchBlitz>, args: &Value) -> String {
    let needle = args
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let role = args
        .get("role")
        .and_then(Value::as_str)
        .map(str::to_lowercase);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
    let all = collect_widgets(h);
    let hits: Vec<_> = all
        .iter()
        .filter(|w| {
            needle.as_ref().is_none_or(|n| {
                w.label.to_lowercase().contains(n)
                    || w.value
                        .as_ref()
                        .is_some_and(|v| v.to_lowercase().contains(n))
            }) && role.as_ref().is_none_or(|r| w.role.to_lowercase() == *r)
        })
        .collect();
    let mut out: Vec<String> = hits
        .iter()
        .take(limit)
        .enumerate()
        .map(|(i, w)| describe(i, w))
        .collect();
    if hits.len() > limit {
        out.push(format!(
            "... {} more (raise limit or filter by label)",
            hits.len() - limit
        ));
    }
    if out.is_empty() {
        out.push("no matching widgets on screen".into());
    }
    out.join("\n")
}

/// Screen position for a tool call: `x`/`y`, or the centre of the widget matching `label`
/// (exact match preferred over substring; `nth` picks among several, counted top to bottom).
fn target(h: &Harness<'static, BranchBlitz>, args: &Value) -> Result<Pos2, String> {
    if let (Some(x), Some(y)) = (
        args.get("x").and_then(Value::as_f64),
        args.get("y").and_then(Value::as_f64),
    ) {
        return Ok(Pos2::new(x as f32, y as f32));
    }
    let label = str_arg(args, "label").map_err(|_| "give a label, or x and y".to_string())?;
    let all = collect_widgets(h);
    let mut hits: Vec<&Widget> = all.iter().filter(|w| w.label == label).collect();
    if hits.is_empty() {
        let l = label.to_lowercase();
        hits = all
            .iter()
            .filter(|w| w.label.to_lowercase().contains(&l))
            .collect();
    }
    // Drop-downs and text fields often have only a value (the current choice).
    if hits.is_empty() {
        hits = all
            .iter()
            .filter(|w| w.label.is_empty() && w.value.as_deref() == Some(label))
            .collect();
    }
    hits.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
    match (hits.len(), args.get("nth").and_then(Value::as_u64)) {
        (0, _) => Err(format!(
            "no widget labelled {label:?} on screen; call widgets to see what is there"
        )),
        (1, None) => Ok(hits[0].rect.center()),
        (n, Some(k)) if (k as usize) < n => Ok(hits[k as usize].rect.center()),
        (n, k) => {
            let list: Vec<String> = hits
                .iter()
                .enumerate()
                .map(|(i, w)| describe(i, w))
                .collect();
            Err(format!(
                "{} {n} widgets match {label:?}; pass nth (0-based, top to bottom):\n{}",
                if k.is_some() {
                    "nth out of range:"
                } else {
                    "ambiguous:"
                },
                list.join("\n")
            ))
        }
    }
}

fn screenshot(h: &mut Harness<'static, BranchBlitz>) -> Result<Value, String> {
    // Without a usable GPU adapter wgpu may panic; that should not cost the open repositories.
    let img = catch_unwind(AssertUnwindSafe(|| h.render()))
        .map_err(|_| "screenshots unavailable: no usable graphics adapter (wgpu)".to_string())?
        .map_err(|e| format!("screenshot failed: {e}"))?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(json!({
        "type": "image",
        "mimeType": "image/png",
        "data": base64::engine::general_purpose::STANDARD.encode(&png),
    }))
}

/// Builds a fresh demo repository (bare origin + clones with branches, merges, a tag and WIP).
fn demo_repo() -> Result<PathBuf, String> {
    use std::hash::{BuildHasher, RandomState};
    // Unpredictable name, created by us (fails if it exists), so nobody can plant a symlink there.
    let dir = (0..5)
        .map(|_| {
            let r = RandomState::new().hash_one(std::time::SystemTime::now());
            Path::new("/tmp").join(format!("branchblitz-demo-{r:016x}"))
        })
        .find(|d| std::fs::create_dir(d).is_ok())
        .ok_or("cannot create a directory in /tmp")?;
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(DEMO_SCRIPT)
        .arg("demo-repo.sh")
        .arg(&dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("cannot run bash: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!(
            "demo repo script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(dir)
}

fn tool_list() -> Value {
    let target = json!({
        "label": {"type": "string", "description": "Accessibility label of the widget (exact, else substring; unlabelled drop-downs match their current value). See the widgets tool."},
        "nth": {"type": "integer", "description": "Which match when several widgets share the label, 0-based from the top."},
        "x": {"type": "number", "description": "X in points (alternative to label)."},
        "y": {"type": "number", "description": "Y in points (alternative to label)."},
    });
    let shot =
        json!({"type": "boolean", "description": "Also return a screenshot after the action."});
    let with = |extra: Value| -> Value {
        let mut props = target.as_object().cloned().unwrap_or_default();
        props.insert("screenshot".into(), shot.clone());
        if let Some(e) = extra.as_object() {
            props.extend(e.clone());
        }
        Value::Object(props)
    };
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": {"type": "object", "properties": props, "required": required},
        })
    };
    json!([
        tool(
            "open_repo",
            "Open (or switch to) a git repository in a tab. Fetches from the remotes like the app does.",
            json!({"path": {"type": "string", "description": "Path inside the repository; ~/ is expanded."}, "screenshot": shot}),
            &["path"]
        ),
        tool(
            "demo_repo",
            "Create a throwaway repository under /tmp (with its own remote, branches, merges, a tag and uncommitted changes) and open it. Safe for experiments; removed when the server exits normally.",
            json!({}),
            &[]
        ),
        tool(
            "state",
            "What the app shows, as JSON: tabs, current branch, local/remote branches with ahead/behind, tags, working copy, recent commits with their labels, open dialog, status line.",
            json!({"commits": {"type": "integer", "description": "How many recent commits to include (default 20)."}}),
            &[]
        ),
        tool("screenshot", "PNG of the whole window.", json!({}), &[]),
        tool(
            "widgets",
            "List visible widgets (role, label, position) from the accessibility tree, to find what to click.",
            json!({"label": {"type": "string", "description": "Only widgets whose label or value contains this (case-insensitive)."},
                   "role": {"type": "string", "description": "Only this role, e.g. Button, TextInput, CheckBox, Label."},
                   "limit": {"type": "integer", "description": "Max lines (default 200)."}}),
            &[]
        ),
        tool(
            "click",
            "Click a widget (by label) or a position. button=right opens context menus (e.g. merge a branch into the current one).",
            with(
                json!({"button": {"type": "string", "enum": ["left", "right", "middle"]},
                        "double": {"type": "boolean", "description": "Double-click (e.g. checkout a branch)."},
                        "confirm_dangerous": {"type": "boolean", "description": "Allow this click to confirm a destructive action in the red alert bar. Only when the user asked for exactly that action."},
                        "modifiers": {"type": "array", "items": {"type": "string", "enum": ["ctrl", "shift", "alt"]}}})
            ),
            &[]
        ),
        tool(
            "hover",
            "Move the mouse over a widget or position (shows tooltips).",
            with(json!({})),
            &[]
        ),
        tool(
            "type_text",
            "Type text into the focused field, or click the field given by label/x,y first.",
            with(json!({"text": {"type": "string"}})),
            &["text"]
        ),
        tool(
            "key",
            "Press a key, e.g. Enter, Escape, ArrowDown, Tab.",
            json!({"key": {"type": "string"}, "modifiers": {"type": "array", "items": {"type": "string", "enum": ["ctrl", "shift", "alt"]}}, "screenshot": shot}),
            &["key"]
        ),
        tool(
            "scroll",
            "Scroll the area under a widget or position (default: window centre).",
            with(
                json!({"dy": {"type": "number", "description": "Points; positive scrolls down."}, "dx": {"type": "number"}})
            ),
            &[]
        ),
        tool(
            "resize",
            "Change the window size in points.",
            json!({"width": {"type": "number"}, "height": {"type": "number"}, "screenshot": shot}),
            &["width", "height"]
        ),
        tool(
            "set_theme",
            "Switch between dark and light theme.",
            json!({"theme": {"type": "string", "enum": ["dark", "light"]}, "screenshot": shot}),
            &["theme"]
        ),
        tool(
            "wait_idle",
            "Wait until git operations, loading and diffs are finished.",
            json!({"timeout_ms": {"type": "integer", "description": "Default 30000."}, "screenshot": shot}),
            &[]
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, InputKind};

    /// Fetch through the UI: "Fetch All" in the Pull button's menu runs it once.
    fn fetch(s: &mut Server) {
        let (err, out) = call(s, "click", json!({"label": "Pull options"}));
        assert!(!err, "{out}");
        let (err, out) = call(s, "click", json!({"label": "Fetch All", "role": "Button"}));
        assert!(!err, "{out}");
        call(s, "wait_idle", json!({}));
    }

    fn call(s: &mut Server, name: &str, args: Value) -> (bool, String) {
        let r = s.call(name, &args);
        let text = r["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        (r["isError"].as_bool().unwrap(), text)
    }

    #[test]
    fn speaks_json_rpc() {
        let mut s = Server::default();
        let init = s
            .handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {}}}))
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert!(
            s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        let list = s
            .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}))
            .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"click") && names.contains(&"screenshot"),
            "{names:?}"
        );
        let unknown = s
            .handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}))
            .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);

        let future = s
            .handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "initialize",
                "params": {"protocolVersion": "2099-01-01"}}))
            .unwrap();
        assert_eq!(future["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
        // A response (no method) is not answered.
        assert!(
            s.handle(&json!({"jsonrpc": "2.0", "id": 5, "result": {}}))
                .is_none()
        );
        assert_eq!(s.handle_line(b"{nope").unwrap()["error"]["code"], -32700);
        assert_eq!(s.handle_line(b"\xff\xfe").unwrap()["error"]["code"], -32700);
        assert_eq!(s.handle_line(b"[]").unwrap()["error"]["code"], -32600);
    }

    /// The core user flow through the MCP tools: right-click a remote branch, merge it.
    #[test]
    fn merges_a_remote_branch_via_the_context_menu() {
        let mut s = Server::default();
        let (err, out) = call(&mut s, "demo_repo", json!({}));
        assert!(!err, "{out}");
        let (err, out) = call(&mut s, "open_repo", json!({"path": "/definitely/not/here"}));
        assert!(err, "{out}");

        // `develop` is in the sidebar twice (local and remote) and once as a label in the graph;
        // the remote one in the sidebar is the lowest.
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "develop", "button": "right"}),
        );
        assert!(err && out.contains("ambiguous"), "{out}");
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "develop", "button": "right", "nth": 2}),
        );
        assert!(!err, "{out}");
        let (_, menu) = call(&mut s, "widgets", json!({"label": "Merge origin/develop"}));
        assert!(menu.contains("into feature/PROJ-523-ai-search"), "{menu}");
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Merge origin/develop into feature/PROJ-523-ai-search"}),
        );
        assert!(!err, "{out}");
        let (err, out) = call(&mut s, "wait_idle", json!({}));
        assert!(!err, "{out}");

        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 50})).1).unwrap();
        // Commits made in the same second may come in any order; find HEAD itself.
        let tab = &state["tabs"][0];
        let head = tab["commits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["sha"] == tab["head"])
            .unwrap();
        assert_eq!(head["parents"], 2, "{state:#}");
        assert!(
            head["subject"].as_str().unwrap().contains("develop"),
            "{head}"
        );

        let (err, out) = call(&mut s, "key", json!({"key": "NoSuchKey"}));
        assert!(err, "{out}");
    }

    fn remote_branches(s: &mut Server) -> Vec<String> {
        let state: Value =
            serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
        state["tabs"][0]["remote_branches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// Merging only offers what would bring something in: no tags, no branches already in HEAD.
    #[test]
    fn merge_menu_hides_pointless_merges() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        // Local develop is the base of the checked out feature branch.
        call(
            &mut s,
            "click",
            json!({"label": "develop", "button": "right", "nth": 0}),
        );
        let (_, menu) = call(
            &mut s,
            "widgets",
            json!({"label": "feature/PROJ-523-ai-search"}),
        );
        assert!(
            menu.contains("Already in feature/PROJ-523-ai-search"),
            "{menu}"
        );
        assert!(!menu.contains("Merge develop into"), "{menu}");
        call(&mut s, "key", json!({"key": "Escape"}));

        call(
            &mut s,
            "click",
            json!({"label": "v3.1.4", "button": "right"}),
        );
        let (_, menu) = call(&mut s, "widgets", json!({"label": "Merge"}));
        assert!(!menu.contains("Merge v3.1.4"), "{menu}");
        // v3.1.4 is already in this branch (via develop), so rebasing onto it does nothing either.
        let (_, menu) = call(&mut s, "widgets", json!({"label": "Rebase"}));
        assert!(!menu.contains("onto v3.1.4"), "{menu}");
        call(&mut s, "key", json!({"key": "Escape"}));

        // origin/develop has a new commit, so it can still be merged.
        call(
            &mut s,
            "click",
            json!({"label": "develop", "button": "right", "nth": 2}),
        );
        let (_, menu) = call(&mut s, "widgets", json!({"label": "Merge origin/develop"}));
        assert!(menu.contains("into feature/PROJ-523-ai-search"), "{menu}");
    }

    /// The desktop's theme (Omarchy) is on offer, decides light or dark, and is followed when the
    /// user switches to another one.
    #[test]
    fn system_theme_is_chosen_and_followed() {
        use crate::system_theme::{NAME, SAMPLE_DARK, SAMPLE_LIGHT};
        let dir = std::env::temp_dir().join(format!("bb-omarchy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("colors.toml");
        std::fs::write(&file, SAMPLE_LIGHT).unwrap();
        crate::system_theme::use_file(file.clone());

        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        call(&mut s, "click", json!({"label": "Settings"}));
        let (err, out) = call(&mut s, "click", json!({"label": NAME}));
        assert!(!err, "{out}");
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(state["theme_name"], NAME);
        call(&mut s, "key", json!({"key": "Escape"}));
        let h = s.session.as_mut().unwrap();
        h.run_steps(2);
        assert_eq!(
            h.ctx.theme(),
            egui::Theme::Light,
            "a light theme makes the app light"
        );
        let canvas = h.ctx.style_of(egui::Theme::Light).visuals.extreme_bg_color;
        assert_eq!(canvas, egui::Color32::from_rgb(0xef, 0xf1, 0xf5));

        // The user switches to a dark Omarchy theme.
        std::fs::write(&file, format!("{SAMPLE_DARK}\n# switched\n")).unwrap();
        crate::system_theme::expire();
        h.run_steps(2);
        assert_eq!(h.ctx.theme(), egui::Theme::Dark, "follows the switch");
        let canvas = h.ctx.style_of(egui::Theme::Dark).visuals.extreme_bg_color;
        assert_eq!(canvas, egui::Color32::from_rgb(0x1e, 0x1e, 0x2e));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn settings_window_picks_a_theme_and_closes_outside() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let (_, out) = call(&mut s, "click", json!({"label": "Settings"}));
        assert!(out.contains("dialog: Settings"), "{out}");
        call(&mut s, "click", json!({"label": "Ocean"}));
        call(&mut s, "click", json!({"label": "Light"}));
        let (_, out) = call(&mut s, "click", json!({"x": 10, "y": 845}));
        assert!(!out.contains("dialog"), "{out}");
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(state["theme_name"], "Ocean");
        assert_eq!(state["theme"], "Light");
    }

    /// Amending starts from the previous commit's message, so an empty description does not
    /// silently wipe its body; switching amend off again clears an untouched prefill.
    #[test]
    fn amend_prefills_the_previous_message() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let dir = s.demo_dirs[0].join("work");
        let head = crate::git::git(&dir, &["log", "-1", "--format=%B"]).unwrap();
        let (summary, body) = crate::app::split_message(&head);
        call(&mut s, "click", json!({"label": "WIP"}));
        call(&mut s, "click", json!({"label": "Amend previous commit"}));
        let msg = |s: &mut Server| {
            let h = s.session.as_mut().unwrap();
            for _ in 0..100 {
                h.step();
                if !h.state_mut().tabs_mut()[0].commit_msg.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let t = &h.state_mut().tabs_mut()[0];
            (t.commit_msg.clone(), t.commit_body.clone())
        };
        assert_eq!(msg(&mut s), (summary, body));
        call(&mut s, "click", json!({"label": "Amend previous commit"}));
        let t = &s.session.as_mut().unwrap().state_mut().tabs_mut()[0];
        assert!(t.commit_msg.is_empty() && t.commit_body.is_empty());
    }

    /// After a fetch BranchBlitz knows which tags are on the remote; only the others are drawn
    /// as unpushed (dashed).
    #[test]
    fn knows_which_tags_are_pushed() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let ok = std::process::Command::new("git")
            .args(["-C", &work, "tag", "local-only"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap();
        assert!(ok.success());
        fetch(&mut s);
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        let on_remote = |name: &str| {
            state["tabs"][0]["tags"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == name)
                .map(|t| t["on_remote"].clone())
                .unwrap()
        };
        assert_eq!(on_remote("v3.1.4"), json!(true), "{state:#}");
        assert_eq!(on_remote("local-only"), json!(false));

        // Moved locally to another commit: no longer the tag that is on the remote.
        let ok = std::process::Command::new("git")
            .args(["-C", &work, "tag", "-f", "v3.1.4", "HEAD"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(ok.status.success());
        fetch(&mut s);
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        let moved = state["tabs"][0]["tags"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "v3.1.4")
            .unwrap()["on_remote"]
            .clone();
        assert_eq!(moved, json!(false));
    }

    /// With two remotes a branch that is the same on both shows as one chip; where they differ,
    /// each chip names its remote (checked visually; the accessible names are the full refs).
    #[test]
    fn branch_chips_with_two_remotes() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let azure = format!("{work}/../azure.git");
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&work)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                ok.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&ok.stderr)
            );
        };
        git(&["init", "-q", "--bare", &azure]);
        git(&["remote", "add", "azure", &azure]);
        // main is equal on both remotes; azure/develop lags behind origin/develop.
        git(&["push", "-q", "azure", "main", "develop"]);
        fetch(&mut s);
        let (_, chips) = call(&mut s, "widgets", json!({"role": "Button", "limit": 400}));
        assert!(
            chips.contains("\"main, azure/main, origin/main\""),
            "{chips}"
        );
        assert!(chips.contains("\"origin/develop\""), "{chips}");
        assert!(chips.contains("\"develop, azure/develop\""), "{chips}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-two-remotes.png"), png).unwrap();
        }
    }

    /// A branch checked out in another worktree can't be checked out here; its menu and a
    /// double-click switch the tab to that worktree instead, and back again.
    #[test]
    fn branch_in_another_worktree_switches_to_it() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        let head = state["tabs"][0]["head_branch"]
            .as_str()
            .unwrap()
            .to_string();
        let wt = format!("{work}/../main-wt");
        let ok = std::process::Command::new("git")
            .args(["-C", &work, "worktree", "add", "-q", &wt, "main"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        fetch(&mut s);

        call(
            &mut s,
            "click",
            json!({"label": "main", "button": "right", "nth": 0}),
        );
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-worktree.png"), png).unwrap();
        }
        let (_, w) = call(&mut s, "widgets", json!({"label": "Switch to worktree"}));
        assert!(w.contains("main-wt"), "{w}");
        assert!(!w.contains("Check out main"), "{w}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "another worktree"}));
        assert!(w.contains("disabled"), "{w}");
        call(&mut s, "key", json!({"key": "Escape"}));

        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "main", "nth": 0, "double": true}),
        );
        assert!(!err, "{out}");
        let tabs = |s: &mut Server| -> Vec<Value> {
            call(s, "wait_idle", json!({}));
            let state: Value =
                serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
            state["tabs"].as_array().unwrap().clone()
        };
        let t = tabs(&mut s);
        assert_eq!(t.len(), 1, "same tab: {t:#?}");
        assert_eq!(t[0]["head_branch"], "main", "{t:#?}");

        // The branch of the main worktree switches back.
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": head, "nth": 0, "double": true}),
        );
        assert!(!err, "{out}");
        let t = tabs(&mut s);
        assert_eq!(t.len(), 1, "{t:#?}");
        assert_eq!(t[0]["head_branch"], head.as_str(), "{t:#?}");

        // The Worktrees section lists both; double-click switches.
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Worktree main-wt (main)", "double": true}),
        );
        assert!(!err, "{out}");
        let t = tabs(&mut s);
        assert_eq!(t[0]["head_branch"], "main", "{t:#?}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Worktree "}));
        assert!(w.contains(&format!("({head})")), "{w}");

        // The menu of the main worktree: open it, but not remove or lock it.
        call(
            &mut s,
            "click",
            json!({"label": format!("Worktree work ({head})"), "button": "right"}),
        );
        let (_, w) = call(&mut s, "widgets", json!({"role": "Button", "limit": 400}));
        assert!(w.contains("\"Open this worktree\""), "{w}");
        assert!(w.contains("\"Open worktree in new tab\""), "{w}");
        assert!(w.contains("Remove (main worktree)"), "{w}");
        assert!(!w.contains("Lock this worktree"), "{w}");
        call(&mut s, "key", json!({"key": "Escape"}));

        // Back in the main worktree, remove the other one (after confirming).
        call(
            &mut s,
            "click",
            json!({"label": format!("Worktree work ({head})"), "double": true}),
        );
        tabs(&mut s);
        call(
            &mut s,
            "click",
            json!({"label": "Worktree main-wt (main)", "button": "right"}),
        );
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-worktree-menu.png"), png).unwrap();
        }
        let (err, out) = call(&mut s, "click", json!({"label": "Remove this worktree"}));
        assert!(!err, "{out}");
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Remove worktree", "role": "Button", "confirm_dangerous": true}),
        );
        assert!(!err, "{out}");
        let t = tabs(&mut s);
        assert_eq!(t[0]["head_branch"], head.as_str(), "{t:#?}");
        assert!(!std::path::Path::new(&wt).exists(), "directory removed");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Worktree "}));
        assert!(!w.contains("main-wt"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-worktrees.png"), png).unwrap();
        }
    }

    /// Ctrl F finds commits by message or hash; Enter steps through the matches, Esc closes.
    #[test]
    fn find_commits_by_message_or_hash() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let (err, out) = call(&mut s, "key", json!({"key": "F", "modifiers": ["ctrl"]}));
        assert!(!err, "{out}");
        call(&mut s, "type_text", json!({"text": "proj-483"}));
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-find.png"), png).unwrap();
        }
        let (_, w) = call(&mut s, "widgets", json!({"label": " of 3"}));
        assert!(
            w.contains("\"1 of 3\""),
            "typing jumps to the first match: {w}"
        );
        call(&mut s, "key", json!({"key": "Enter"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": " of 3"}));
        assert!(w.contains("\"2 of 3\""), "{w}");
        call(
            &mut s,
            "key",
            json!({"key": "Enter", "modifiers": ["shift"]}),
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": " of 3"}));
        assert!(w.contains("\"1 of 3\""), "{w}");

        // A hash prefix finds its commit.
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 20})).1).unwrap();
        let sha = state["tabs"][0]["commits"].as_array().unwrap()[4]["sha"]
            .as_str()
            .unwrap()[..7]
            .to_string();
        for _ in 0..8 {
            call(&mut s, "key", json!({"key": "Backspace"}));
        }
        call(&mut s, "type_text", json!({"text": sha}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "1 of 1"}));
        assert!(w.contains("\"1 of 1\""), "{sha}: {w}");

        call(&mut s, "key", json!({"key": "Escape"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Close find"}));
        assert!(!w.contains("Close find"), "{w}");
    }

    /// With more commits than the limit: scrolling to the end loads more, and find offers
    /// matches in commits that are not loaded yet, loading until the first one is shown.
    #[test]
    fn loads_more_commits_and_finds_older_ones() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let ok = std::process::Command::new("sh")
            .args([
                "-c",
                "for i in $(seq 250); do git commit -q --allow-empty -m \"Bulk change $i\"; done",
            ])
            .current_dir(&work)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_commit_limit(100);
        call(&mut s, "key", json!({"key": "F5"}));
        call(&mut s, "wait_idle", json!({}));
        let loaded = |s: &mut Server| {
            let st: Value =
                serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
            st["tabs"][0]["commits_loaded"].as_u64().unwrap()
        };
        assert_eq!(loaded(&mut s), 100);

        // Scrolling to the end of the graph loads the next 100.
        call(&mut s, "scroll", json!({"dy": 3000}));
        call(&mut s, "wait_idle", json!({}));
        assert_eq!(loaded(&mut s), 200);

        // Find: the demo's own commits are all older than the 200 loaded ones.
        call(&mut s, "key", json!({"key": "F", "modifiers": ["ctrl"]}));
        call(&mut s, "type_text", json!({"text": "proj-483"}));
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "older history"}));
        assert!(w.contains("3 more in older history"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-find-older.png"), png).unwrap();
        }
        call(&mut s, "click", json!({"label": "Show older match"}));
        call(&mut s, "wait_idle", json!({}));
        assert!(loaded(&mut s) > 200, "loaded until the match");
        let (_, w) = call(&mut s, "widgets", json!({"label": " of 3"}));
        assert!(w.contains("\"1 of 3\""), "the match is selected: {w}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "older history"}));
        assert!(!w.contains("more in older history"), "{w}");
    }

    /// A demo repo whose current branch and `clash` both changed two separate lines of c.txt
    /// (mine/mine2 against theirs/theirs2). Returns the working directory and the branch.
    fn conflicting_demo(s: &mut Server) -> (String, String) {
        let (_, out) = call(s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let ok = std::process::Command::new("sh")
            .args([
                "-c",
                "git stash -u -q \
                 && printf 'a\\nshared\\nb\\nb\\nb\\nb\\nshared2\\nc\\n' > c.txt \
                 && git add c.txt && git commit -qm base \
                 && git checkout -q -b clash \
                 && printf 'a\\ntheirs\\nb\\nb\\nb\\nb\\ntheirs2\\nc\\n' > c.txt \
                 && git commit -qam clash \
                 && git checkout -q - \
                 && printf 'a\\nmine\\nb\\nb\\nb\\nb\\nmine2\\nc\\n' > c.txt \
                 && git commit -qam mine",
            ])
            .current_dir(&work)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        call(s, "key", json!({"key": "F5"}));
        call(s, "wait_idle", json!({}));
        let st: Value = serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
        let head = st["tabs"][0]["head_branch"].as_str().unwrap().to_string();
        (work, head)
    }

    /// During a rebase git's "ours" is the branch rebased onto; "Mine" still takes my change.
    #[test]
    fn resolve_a_rebase_conflict_keeps_mine_mine() {
        let mut s = Server::default();
        let (work, head) = conflicting_demo(&mut s);
        call(
            &mut s,
            "click",
            json!({"label": "clash", "button": "right", "nth": 1}),
        );
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": format!("Rebase {head} onto clash")}),
        );
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "conflicts chosen"}));
        assert!(w.contains("0 of 2"), "{w}");
        call(&mut s, "click", json!({"label": "Conflict 1: take mine"}));
        call(
            &mut s,
            "click",
            json!({"label": "Conflict 2: take incoming"}),
        );
        call(&mut s, "click", json!({"label": "Save & mark resolved"}));
        call(&mut s, "wait_idle", json!({}));
        assert_eq!(
            std::fs::read_to_string(format!("{work}/c.txt")).unwrap(),
            "a\nmine\nb\nb\nb\nb\ntheirs2\nc\n"
        );
    }

    /// A merge that stops on a conflict opens the conflict view (no error dialog); choosing a
    /// side per block and saving resolves the file, after which the merge can be committed.
    #[test]
    fn resolve_a_merge_conflict_per_block() {
        let mut s = Server::default();
        let (work, head) = conflicting_demo(&mut s);
        call(
            &mut s,
            "click",
            json!({"label": "clash", "button": "right", "nth": 1}),
        );
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": format!("Merge clash into {head}")}),
        );
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let st: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert!(
            st.get("dialog").is_none_or(Value::is_null),
            "no error dialog: {st:#}"
        );

        let (_, w) = call(&mut s, "widgets", json!({"label": "conflicts chosen"}));
        assert!(
            w.contains("0 of 2 conflicts chosen"),
            "conflict view opens: {w}"
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": "Save & mark resolved"}));
        assert!(w.contains("disabled"), "{w}");
        call(
            &mut s,
            "click",
            json!({"label": "Conflict 1: take incoming"}),
        );
        call(&mut s, "click", json!({"label": "Conflict 2: take both"}));
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-conflict.png"), png).unwrap();
        }
        let (err, out) = call(&mut s, "click", json!({"label": "Save & mark resolved"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        assert_eq!(
            std::fs::read_to_string(format!("{work}/c.txt")).unwrap(),
            "a\ntheirs\nb\nb\nb\nb\nmine2\ntheirs2\nc\n"
        );
        let st: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(st["tabs"][0]["conflicted"], json!([]), "{st:#}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Commit merge"}));
        assert!(!w.contains("disabled"), "{w}");
    }

    /// The agent panel opens at the bottom of the right panel, answers what you send, can be
    /// shown large and back, and closes to its bar.
    #[test]
    fn agent_panel_opens_answers_and_resizes() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let (err, out) = call(&mut s, "click", json!({"label": "Open agent"}));
        assert!(!err, "{out}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Send (Enter)"}));
        assert!(w.contains("disabled"), "nothing typed yet: {w}");
        call(
            &mut s,
            "type_text",
            json!({"label": "Instruction for the agent", "text": "What changed?"}),
        );
        call(&mut s, "key", json!({"key": "Enter"}));
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "You asked"}));
        assert!(w.contains("What changed?"), "answer streamed in: {w}");
        let (_, w) = call(
            &mut s,
            "widgets",
            json!({"label": "no agent CLI connected"}),
        );
        assert!(
            !w.contains("no agent CLI"),
            "tool calls hidden by default: {w}"
        );
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent.png"), png).unwrap();
        }

        call(&mut s, "click", json!({"label": "Show large"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Show in the panel"}));
        assert!(w.contains("Show in the panel"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent-large.png"), png).unwrap();
        }
        call(&mut s, "click", json!({"label": "Show in the panel"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "You asked"}));
        assert!(w.contains("What changed?"), "conversation kept: {w}");
        call(&mut s, "click", json!({"label": "Close agent"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Open agent"}));
        assert!(w.contains("Open agent"), "{w}");
    }

    #[cfg(unix)]
    /// The agent's tools over the app's socket, as `mcp --attach` uses them: a merge that
    /// stops on a conflict, read and resolved per block, then continued; a force push only asks.
    #[test]
    fn agent_tools_over_the_socket() {
        use std::io::{BufRead, BufReader, Write};
        let mut s = Server::default();
        let (work, head) = conflicting_demo(&mut s);
        let h = s.session.as_mut().unwrap();
        h.state_mut().start_live();
        let attach = h.state().agent_attach(0).expect("live endpoint");
        let (socket, token, tab) = (attach.socket.clone(), attach.token.clone(), attach.tab);

        let client = std::thread::spawn(move || {
            let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
            writeln!(stream, "{}", json!({"token": token, "tab": tab})).unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
            let mut id = 0;
            let mut rpc = |method: &str, params: Value| -> Value {
                id += 1;
                let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
                writeln!(stream, "{msg}").unwrap();
                serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap()
            };
            let call = |rpc: &mut dyn FnMut(&str, Value) -> Value, name: &str, args: Value| {
                let r = rpc("tools/call", json!({"name": name, "arguments": args}));
                let text = r["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                (r["result"]["isError"] == true, text)
            };
            let init = rpc("initialize", json!({"protocolVersion": "2025-06-18"}));
            let tools = rpc("tools/list", json!({}));
            let mut out = vec![init.to_string(), tools.to_string()];
            for (name, args) in [
                ("merge", json!({"name": "clash"})),
                ("read_conflict", json!({"path": "c.txt"})),
                (
                    "resolve_conflict",
                    json!({"path": "c.txt", "choices": ["incoming", "both"]}),
                ),
                ("operation", json!({"action": "continue"})),
                ("state", json!({"commits": 1})),
                ("sync", json!({"action": "force_push"})),
                ("checkout", json!({"name": "no-such-branch"})),
                ("sync", json!({"action": "push"})),
                ("changes", json!({"action": "discard", "paths": ["."]})),
                (
                    "commit_action",
                    json!({"action": "reset_hard", "sha": "HEAD~1"}),
                ),
            ] {
                let (err, text) = call(&mut rpc, name, args);
                out.push(format!("{name} err={err}: {text}"));
            }
            out
        });
        let started = Instant::now();
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "agent tools timed out"
            );
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = client.join().unwrap();
        let all = out.join("\n");
        assert!(out[0].contains("branchblitz"), "{all}");
        assert!(out[1].contains("resolve_conflict"), "{all}");
        assert!(
            out[2].contains("merge err=false") && out[2].contains("stopped on conflicts"),
            "{all}"
        );
        assert!(
            out[3].contains("\"mine\": \"mine\\n\"")
                && out[3].contains("\"incoming\": \"theirs\\n\""),
            "{all}"
        );
        assert!(out[4].contains("resolve_conflict err=false"), "{all}");
        assert!(
            out[5].contains("operation err=false") && out[5].contains("0 conflicted"),
            "{all}"
        );
        assert!(
            out[6].contains(&format!("\"head_branch\": \"{head}\"")),
            "{all}"
        );
        assert!(out[7].contains("Asked the user in the bar"), "{all}");
        assert!(out[8].contains("checkout err=true"), "{all}");
        assert!(
            out[9].contains("sync err=true") && out[9].contains("question open"),
            "{all}"
        );
        assert!(
            out[10].contains("changes err=true") && out[10].contains("no unstaged changes"),
            "{all}"
        );
        assert!(
            out[11].contains("commit_action err=true") && out[11].contains("No loaded commit"),
            "{all}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{work}/c.txt")).unwrap(),
            "a\ntheirs\nb\nb\nb\nb\nmine2\ntheirs2\nc\n"
        );
        assert!(
            h.state().snapshot(0)["tabs"][0]["alert"]["title"]
                .as_str()
                .is_some_and(|t| t.starts_with("Force push"))
        );
    }

    #[cfg(unix)]
    /// Claude Code driving the app through `branchblitz mcp --attach` (costs a little; needs
    /// `cargo build` first): `cargo test claude_drives_the_app -- --ignored`.
    #[test]
    #[ignore]
    fn claude_drives_the_app() {
        use crate::agent::{Backend, Conversation, Role};
        let Backend::Claude(cli) = Backend::detect() else {
            panic!("claude not on PATH")
        };
        let mut s = Server::default();
        let (work, _) = conflicting_demo(&mut s);
        let h = s.session.as_mut().unwrap();
        h.state_mut().start_live();
        let mut attach = h.state().agent_attach(0).unwrap();
        attach.exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/branchblitz");
        let mut c = Conversation::default();
        c.input = "Merge the branch clash into the current branch. If that gives conflicts, take \
                   the incoming version of every conflict block and finish the merge."
            .into();
        let (tx, rx) = std::sync::mpsc::channel();
        c.send(
            &Backend::Claude(cli),
            Path::new(&work),
            Some(attach),
            None,
            String::new(),
            move |e| tx.send(e).unwrap(),
        );
        let started = Instant::now();
        while c.busy() {
            assert!(
                started.elapsed() < Duration::from_secs(300),
                "{:?}",
                c.messages
            );
            h.step();
            while let Ok(e) = rx.try_recv() {
                c.apply(e);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let log: Vec<String> = c
            .messages
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.text))
            .collect();
        let log = log.join("\n");
        assert!(
            c.messages
                .iter()
                .any(|m| m.role == Role::Tool && m.text.starts_with("merge")),
            "{log}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{work}/c.txt")).unwrap(),
            "a\ntheirs\nb\nb\nb\nb\ntheirs2\nc\n",
            "{log}"
        );
        let st = h.state().snapshot(0);
        assert_eq!(st["tabs"][0]["repo_state"], "Clean", "{log}");
        eprintln!("{log}");
    }

    /// With Claude Code as the agent, the panel's header offers the model for the next
    /// questions.
    #[test]
    fn agent_model_is_chosen_in_the_panel() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_agent_backend(crate::agent::Backend::Claude("/nonexistent/claude".into()));
        call(&mut s, "click", json!({"label": "Open agent"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Model: Default model"}));
        assert!(!err, "{out}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent-model.png"), png).unwrap();
        }
        let (err, out) = call(&mut s, "click", json!({"label": "Sonnet"}));
        assert!(!err, "{out}");
        assert_eq!(
            s.session.as_ref().unwrap().state().agent_model(),
            Some("sonnet")
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": "Model: Sonnet"}));
        assert!(w.contains("Model: Sonnet"), "{w}");
    }

    #[cfg(unix)]
    /// Agent tool calls over the app's socket while the app keeps running; (is_error, text).
    fn agent_calls(s: &mut Server, calls: Vec<(&'static str, Value)>) -> Vec<(bool, String)> {
        use std::io::{BufRead, BufReader, Write};
        let h = s.session.as_mut().unwrap();
        if h.state().agent_attach(0).is_none() {
            h.state_mut().start_live();
        }
        let a = h.state().agent_attach(0).unwrap();
        let client = std::thread::spawn(move || {
            let mut stream = std::os::unix::net::UnixStream::connect(&a.socket).unwrap();
            writeln!(stream, "{}", json!({"token": a.token, "tab": a.tab})).unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
            calls
                .into_iter()
                .enumerate()
                .map(|(id, (name, args))| {
                    let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                        "params": {"name": name, "arguments": args}});
                    writeln!(stream, "{msg}").unwrap();
                    let r: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
                    let text = r["result"]["content"][0]["text"].as_str().unwrap_or("");
                    (r["result"]["isError"] == true, format!("{name}: {text}"))
                })
                .collect()
        });
        let started = Instant::now();
        while !client.is_finished() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "agent calls timed out"
            );
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        client.join().unwrap()
    }

    #[cfg(unix)]
    /// What asks the user depends on the agent's access: Supervised asks for a push, Auto only
    /// for what throws work away, Full access never.
    #[test]
    fn agent_access_decides_what_asks() {
        use crate::app::AgentAccess;
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let alert = |s: &mut Server| {
            let st = s.session.as_ref().unwrap().state().snapshot(0);
            st["tabs"][0]["alert"]["title"].as_str().map(str::to_string)
        };
        let dismiss = |s: &mut Server| {
            call(s, "wait_idle", json!({}));
            let (err, out) = call(s, "click", json!({"label": "Cancel", "role": "Button"}));
            assert!(!err, "{out}");
        };
        assert_eq!(
            s.session.as_ref().unwrap().state().agent_access(),
            AgentAccess::Supervised
        );
        let r = agent_calls(&mut s, vec![("sync", json!({"action": "push"}))]);
        assert!(r[0].1.contains("Asked the user"), "{r:?}");
        dismiss(&mut s);
        assert_eq!(alert(&mut s), None);

        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_agent_access(AgentAccess::Auto);
        let r = agent_calls(
            &mut s,
            vec![
                ("sync", json!({"action": "push"})),
                ("sync", json!({"action": "force_push"})),
            ],
        );
        assert!(!r[0].0 && !r[0].1.contains("Asked"), "push runs: {r:?}");
        assert!(r[1].1.contains("Asked the user"), "force push asks: {r:?}");
        dismiss(&mut s);

        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_agent_access(AgentAccess::Full);
        let r = agent_calls(&mut s, vec![("sync", json!({"action": "force_push"}))]);
        assert!(
            !r[0].0 && !r[0].1.contains("Asked"),
            "force push runs: {r:?}"
        );
        assert_eq!(alert(&mut s), None);
    }

    /// The access picker in the panel header; Full access shows as such.
    #[test]
    fn agent_access_is_chosen_in_the_panel() {
        use crate::app::AgentAccess;
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_agent_backend(crate::agent::Backend::Claude("/nonexistent/claude".into()));
        call(&mut s, "click", json!({"label": "Open agent"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Access: Supervised"}));
        assert!(!err, "{out}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent-access.png"), png).unwrap();
        }
        let (err, out) = call(&mut s, "click", json!({"label": "Full access: Never asks"}));
        assert!(!err, "{out}");
        assert_eq!(
            s.session.as_ref().unwrap().state().agent_access(),
            AgentAccess::Full
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": "Access: Full access"}));
        assert!(w.contains("Access: Full access"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent-full.png"), png).unwrap();
        }
    }

    /// Settings has an Agents page: show tool calls, extra instructions (and model, access).
    #[test]
    fn agents_settings_page() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        call(&mut s, "click", json!({"label": "Settings"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Agents"}));
        assert!(!err, "{out}");
        call(
            &mut s,
            "click",
            json!({"label": "Show what the agent does"}),
        );
        call(
            &mut s,
            "click",
            json!({"label": "Full access", "role": "RadioButton"}),
        );
        call(
            &mut s,
            "type_text",
            json!({"label": "Agent instructions", "text": "Answer in Dutch."}),
        );
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-settings-agents.png"), png).unwrap();
        }
        let app = s.session.as_ref().unwrap().state();
        assert_eq!(app.agent_access(), crate::app::AgentAccess::Full);
        let prompt = app.agent_prompt_for_tests();
        assert!(prompt.starts_with("Access: full."), "{prompt}");
        assert!(
            prompt.ends_with("The user's instructions:\nAnswer in Dutch."),
            "{prompt}"
        );

        // Tool calls now show in the conversation.
        call(&mut s, "key", json!({"key": "Escape"}));
        call(&mut s, "click", json!({"label": "Open agent"}));
        call(
            &mut s,
            "type_text",
            json!({"label": "Instruction for the agent", "text": "hi"}),
        );
        call(&mut s, "key", json!({"key": "Enter"}));
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(
            &mut s,
            "widgets",
            json!({"label": "no agent CLI connected"}),
        );
        assert!(w.contains("no agent CLI"), "{w}");
    }

    #[cfg(unix)]
    /// With the app's instructions the agent uses the tools instead of asking: whether a remote
    /// is reachable is answered by fetching. `cargo build` first; costs a little.
    #[test]
    #[ignore]
    fn claude_fetches_to_check_a_remote() {
        use crate::agent::{Backend, Conversation, Role};
        let Backend::Claude(cli) = Backend::detect() else {
            panic!("claude not on PATH")
        };
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let h = s.session.as_mut().unwrap();
        h.state_mut().start_live();
        let mut attach = h.state().agent_attach(0).unwrap();
        attach.exe = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/branchblitz");
        let extra = h.state().agent_prompt_for_tests();
        let mut c = Conversation::default();
        c.input = "Check of de origin remote beschikbaar is".into();
        let (tx, rx) = std::sync::mpsc::channel();
        c.send(
            &Backend::Claude(cli),
            Path::new(&work),
            Some(attach),
            Some("haiku"),
            extra,
            move |e| tx.send(e).unwrap(),
        );
        let started = Instant::now();
        while c.busy() {
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "{:?}",
                c.messages
            );
            h.step();
            while let Ok(e) = rx.try_recv() {
                c.apply(e);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let log: Vec<String> = c
            .messages
            .iter()
            .map(|m| format!("{:?}: {}", m.role, m.text))
            .collect();
        let log = log.join("\n");
        eprintln!("{log}");
        assert!(
            c.messages
                .iter()
                .any(|m| m.role == Role::Tool && m.text.starts_with("sync")),
            "{log}"
        );
    }

    #[cfg(unix)]
    /// While the agent works, the conversation ends in a status line (its current tool, also
    /// when tool calls are hidden, and the time); Stop ends it.
    #[test]
    fn agent_shows_that_it_is_working() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bb-busy-claude-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\ncat > /dev/null\n\
             echo '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"mcp__branchblitz__sync\",\"input\":{\"action\":\"fetch\"}}]}}'\n\
             sleep 30\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_agent_backend(crate::agent::Backend::Claude(script));
        call(&mut s, "click", json!({"label": "Open agent"}));
        call(
            &mut s,
            "type_text",
            json!({"label": "Instruction for the agent", "text": "fetch"}),
        );
        call(&mut s, "key", json!({"key": "Enter"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Working: sync fetch"}));
        assert!(w.contains("Working: sync fetch ·"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-agent-working.png"), png).unwrap();
        }
        call(&mut s, "click", json!({"label": "Stop the agent"}));
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Working:"}));
        assert!(!w.contains("Working:"), "{w}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    /// Checking out a branch that another worktree has: the agent hears that BranchBlitz now
    /// shows that worktree (no talk of tabs) and keeps working there.
    #[test]
    fn agent_checkout_into_another_worktree() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let wt = format!("{work}/../main-wt");
        let ok = std::process::Command::new("git")
            .args(["-C", &work, "worktree", "add", "-q", &wt, "main"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(ok.status.success());
        call(&mut s, "key", json!({"key": "F5"}));
        call(&mut s, "wait_idle", json!({}));
        let r = agent_calls(
            &mut s,
            vec![
                ("checkout", json!({"name": "main"})),
                ("state", json!({"commits": 0})),
            ],
        );
        assert!(!r[0].0, "{r:?}");
        assert!(
            r[0].1.contains("same window") && !r[0].1.contains("tab"),
            "{r:?}"
        );
        assert!(
            !r[1].0 && r[1].1.contains("\"head_branch\": \"main\""),
            "{r:?}"
        );
    }

    /// Settings, General: the GPU choice says it applies at the next start; the GPU in use shows.
    #[test]
    fn gpu_choice_in_settings() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        call(&mut s, "click", json!({"label": "Settings"}));
        call(&mut s, "click", json!({"label": "General"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Rendering"}));
        assert!(w.contains("Rendering"), "{w}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Applies when"}));
        assert!(!w.contains("Applies when"), "nothing changed yet: {w}");
        let (err, out) = call(&mut s, "click", json!({"label": "High performance"}));
        assert!(!err, "{out}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Applies when"}));
        assert!(w.contains("Applies when BranchBlitz starts again"), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-settings-gpu.png"), png).unwrap();
        }
    }

    /// Settings, About: the diagnostics for a bug report and the buttons to send them.
    #[test]
    fn about_page_shows_diagnostics() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        call(&mut s, "click", json!({"label": "Settings"}));
        let (err, out) = call(&mut s, "click", json!({"label": "About"}));
        assert!(!err, "{out}");
        let (_, w) = call(&mut s, "widgets", json!({"limit": 400}));
        for want in [
            "Report a bug",
            "Copy diagnostics",
            "Git: git version",
            "Rendering:",
            "Agent:",
        ] {
            assert!(w.contains(want), "{want}: {w}");
        }
        assert!(
            w.contains(&format!("BranchBlitz {}", env!("CARGO_PKG_VERSION"))),
            "{w}"
        );
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-about.png"), png).unwrap();
        }
    }

    /// After a crash the next start says so and points at the log.
    #[test]
    fn crash_is_reported_at_the_next_start() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        s.session.as_mut().unwrap().state_mut().report_crash();
        let (_, out) = call(&mut s, "wait_idle", json!({}));
        assert!(
            out.contains("dialog: BranchBlitz quit unexpectedly"),
            "{out}"
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": "Report a bug"}));
        assert!(w.contains("Report a bug"), "{w}");
    }

    /// A newer release shows as a button in the toolbar and on the About page.
    #[test]
    fn newer_release_is_offered() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Update: v"}));
        assert!(!w.contains("Update: v"), "nothing seen yet: {w}");
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_latest_version("99.0.0");
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Update: v"}));
        assert!(w.contains("Update: v99.0.0"), "{w}");
        call(&mut s, "click", json!({"label": "Settings"}));
        call(&mut s, "click", json!({"label": "About"}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "is available"}));
        assert!(w.contains("Version 99.0.0 is available."), "{w}");
        // Not newer than this build: no button.
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_latest_version("0.0.1");
        call(&mut s, "wait_idle", json!({}));
        call(&mut s, "key", json!({"key": "Escape"}));
        call(&mut s, "wait_idle", json!({}));
        let (_, w) = call(&mut s, "widgets", json!({"label": "Update: v"}));
        assert!(!w.contains("Update: v"), "{w}");
    }

    /// Clone from the welcome screen: the clone opens as a tab; a failed clone shows git's error
    /// in the dialog.
    #[test]
    fn clone_from_the_welcome_screen() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let demo = Path::new(&work).parent().unwrap().to_path_buf();
        let origin = demo.join("origin.git").display().to_string();
        // The dialog starts in this folder (set directly: typing over a field's text is unreliable).
        s.session
            .as_mut()
            .unwrap()
            .state_mut()
            .set_clone_parent(demo.join("clones"));
        call(&mut s, "click", json!({"label": "Open repository"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Clone repository…"}));
        assert!(!err, "{out}");
        call(
            &mut s,
            "type_text",
            json!({"label": "Repository URL", "text": origin}),
        );
        let want = demo.join("clones").join("origin").display().to_string();
        let (_, w) = call(&mut s, "widgets", json!({"label": "Clones into"}));
        assert!(w.contains(&want), "{w}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-clone.png"), png).unwrap();
        }
        call(&mut s, "click", json!({"label": "Clone", "role": "Button"}));
        call(&mut s, "wait_idle", json!({}));
        let st: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        let tabs = st["tabs"].as_array().unwrap();
        assert_eq!(tabs.len(), 2, "{st:#}");
        assert_eq!(tabs[1]["path"].as_str(), Some(want.as_str()), "{st:#}");
        assert!(st["dialog"].is_null(), "{st:#}");

        // Cloning what isn't a repository: the dialog stays with git's message.
        call(&mut s, "click", json!({"label": "Open repository"}));
        call(&mut s, "click", json!({"label": "Clone repository…"}));
        let missing = demo.join("no-such-repo").display().to_string();
        call(
            &mut s,
            "type_text",
            json!({"label": "Repository URL", "text": missing}),
        );
        call(&mut s, "click", json!({"label": "Clone", "role": "Button"}));
        call(&mut s, "wait_idle", json!({}));
        let st: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(st["dialog"]["title"], "Clone repository", "{st:#}");
        assert!(
            st["dialog"]["error"]
                .as_str()
                .is_some_and(|e| !e.is_empty()),
            "{st:#}"
        );
        assert!(!demo.join("clones").join("no-such-repo").exists());
    }

    /// Settings, Appearance, Scale changes the zoom; under WSL a hint leads there once.
    #[test]
    fn scale_setting_and_wsl_hint() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        s.session.as_mut().unwrap().state_mut().show_wsl_hint();
        let (_, out) = call(&mut s, "wait_idle", json!({}));
        assert!(out.contains("dialog: Running under WSL"), "{out}");
        let (err, out) = call(&mut s, "click", json!({"label": "Open settings"}));
        assert!(!err, "{out}");
        assert!(out.contains("dialog: Settings"), "{out}");
        let (err, out) = call(&mut s, "click", json!({"label": "Interface scale"}));
        assert!(!err, "{out}");
        let (err, out) = call(&mut s, "click", json!({"label": "150%"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let zoom = s.session.as_ref().unwrap().ctx.zoom_factor();
        assert!((zoom - 1.5).abs() < 0.01, "{zoom}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-scale.png"), png).unwrap();
        }
    }

    /// With BB_SAVE_SHOTS set: the current screen as `name` in the temp dir.
    fn save_shot(s: &mut Server, name: &str) {
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join(name), png).unwrap();
        }
    }

    /// From a file's diff: its history (a commit opens that commit's diff, Esc goes back) and
    /// its blame (a line selects the commit that last changed it).
    #[test]
    fn file_history_and_blame() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(&work)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let mut text = String::new();
        for (n, line) in ["first", "second", "third"].iter().enumerate() {
            text += &format!("{line} line\n");
            std::fs::write(format!("{work}/h.txt"), &text).unwrap();
            git(&["add", "h.txt"]);
            git(&["commit", "-qm", &format!("h step {}", n + 1)]);
        }
        let step1 = git(&["rev-parse", "HEAD~2"]);
        std::fs::write(format!("{work}/h.txt"), text + "fourth line\n").unwrap();
        call(&mut s, "key", json!({"key": "F5"}));
        call(&mut s, "wait_idle", json!({}));
        call(&mut s, "click", json!({"label": "WIP"}));
        call(&mut s, "click", json!({"label": "h.txt"}));
        call(&mut s, "wait_idle", json!({}));
        let state = |s: &mut Server| -> Value {
            let st: Value =
                serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
            st["tabs"][0].clone()
        };

        let (err, out) = call(&mut s, "click", json!({"label": "File history"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let t = state(&mut s);
        assert_eq!(t["file_view"]["kind"], "History", "{t}");
        assert_eq!(t["file_view"]["rows"], 3, "{t}");
        assert!(t["diff_open"].is_null());
        save_shot(&mut s, "bb-file-history.png");
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": format!("{} h step 1", &step1[..7])}),
        );
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let t = state(&mut s);
        assert!(t["diff_open"].as_str().unwrap().contains(&step1), "{t}");
        assert_eq!(t["selected"], step1.as_str());
        call(&mut s, "key", json!({"key": "Escape"}));
        let t = state(&mut s);
        assert!(
            t["diff_open"].is_null() && t["file_view"]["kind"] == "History",
            "{t}"
        );

        call(&mut s, "click", json!({"label": "Show Blame"}));
        call(&mut s, "wait_idle", json!({}));
        let t = state(&mut s);
        assert_eq!(t["file_view"]["kind"], "Blame", "{t}");
        assert_eq!(t["file_view"]["rows"], 4, "{t}");
        let (_, w) = call(&mut s, "widgets", json!({"label": "Blame line"}));
        assert!(w.contains("Blame line 4: 0000000 Not committed yet"), "{w}");
        save_shot(&mut s, "bb-blame.png");
        call(&mut s, "click", json!({"label": "Blame line 3"}));
        call(&mut s, "click", json!({"label": "Blame line 1"}));
        call(&mut s, "wait_idle", json!({}));
        assert_eq!(state(&mut s)["selected"], step1.as_str());
        call(&mut s, "key", json!({"key": "Escape"}));
        assert!(state(&mut s)["file_view"].is_null());
    }

    /// Part of a file: a whole hunk from its header, then one chosen line, into the index.
    #[test]
    fn stage_a_hunk_and_a_line() {
        let mut s = Server::default();
        let (_, out) = call(&mut s, "demo_repo", json!({}));
        let work = out
            .split(" at ")
            .nth(1)
            .unwrap()
            .split(" (remote")
            .next()
            .unwrap()
            .to_string();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(&work)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        let base: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        std::fs::write(format!("{work}/h.txt"), &base).unwrap();
        git(&["add", "h.txt"]);
        git(&["commit", "-qm", "h base"]);
        let changed = base
            .replace("line 2\n", "line 2 changed\nline 2b added\n")
            .replace("line 18\n", "line 18 changed\n");
        std::fs::write(format!("{work}/h.txt"), &changed).unwrap();
        call(&mut s, "key", json!({"key": "F5"}));
        call(&mut s, "wait_idle", json!({}));
        call(&mut s, "click", json!({"label": "WIP"}));
        let (err, out) = call(&mut s, "click", json!({"label": "h.txt"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let (err, out) = call(&mut s, "click", json!({"label": "Stage hunk 2"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let staged = git(&["diff", "--cached", "--", "h.txt"]);
        assert!(
            staged.contains("+line 18 changed") && !staged.contains("line 2 changed"),
            "{staged}"
        );

        // The worktree diff now has one hunk; choose its added line 2b only.
        let (err, out) = call(&mut s, "click", json!({"label": "Line 3: line 2b added"}));
        assert!(!err, "{out}");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-partial.png"), png).unwrap();
        }
        let (err, out) = call(&mut s, "click", json!({"label": "Stage 1 line"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let staged = git(&["diff", "--cached", "--", "h.txt"]);
        assert!(
            staged.contains("+line 2b added") && !staged.contains("+line 2 changed"),
            "{staged}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{work}/h.txt")).unwrap(),
            changed
        );
    }

    /// The checked out branch cannot be deleted; its menu says why, other branches delete.
    #[test]
    fn delete_branch_from_its_menu() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let state = |s: &mut Server| -> Value {
            serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap()
        };
        let head = state(&mut s)["tabs"][0]["head_branch"]
            .as_str()
            .unwrap()
            .to_string();
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": head, "button": "right", "nth": 1}),
        );
        assert!(!err, "{out}");
        let (_, w) = call(
            &mut s,
            "widgets",
            json!({"label": "check out another branch first"}),
        );
        assert!(w.contains("disabled"), "{w}");
        call(&mut s, "key", json!({"key": "Escape"}));

        // Another branch's menu names it in Check out (no check mark: that means current).
        call(
            &mut s,
            "click",
            json!({"label": "main", "button": "right", "nth": 0}),
        );
        let (_, w) = call(&mut s, "widgets", json!({"label": "Check out main"}));
        assert!(w.contains("\"Check out main\""), "{w}");
        call(&mut s, "key", json!({"key": "Escape"}));

        // A branch that is not checked out: Delete runs.
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "main", "button": "right", "nth": 0}),
        );
        assert!(!err, "{out}");
        let (err, out) = call(&mut s, "click", json!({"label": "Delete main"}));
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let locals: Vec<String> = state(&mut s)["tabs"][0]["local_branches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!locals.contains(&"main".to_string()), "{locals:?}");
    }

    /// "Branch" opens a name field in the graph: Enter creates and checks out, Escape or a click
    /// elsewhere creates nothing.
    #[test]
    fn new_branch_is_named_in_the_graph() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let state = |s: &mut Server| -> Value {
            serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap()
        };
        let locals = |s: &mut Server| -> Vec<String> {
            state(s)["tabs"][0]["local_branches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["name"].as_str().unwrap().to_string())
                .collect()
        };
        let open = |s: &mut Server| {
            let (err, out) = call(s, "click", json!({"label": "Branch", "role": "Button"}));
            assert!(!err, "{out}");
            let st = state(s);
            assert!(st["dialog"].is_null(), "no dialog: {st}");
            assert!(
                !st["tabs"][0]["new_branch"].is_null(),
                "field should be open: {st}"
            );
        };
        let before = locals(&mut s);

        // Escape: no branch.
        open(&mut s);
        call(&mut s, "type_text", json!({"text": "feature/escaped"}));
        call(&mut s, "key", json!({"key": "Escape"}));
        call(&mut s, "wait_idle", json!({}));
        assert!(state(&mut s)["tabs"][0]["new_branch"].is_null());
        assert_eq!(locals(&mut s), before);

        // Clicking elsewhere (here: the Tags section in the sidebar): no branch either.
        open(&mut s);
        call(&mut s, "type_text", json!({"text": "feature/clicked-away"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Tags", "role": "Button"}));
        assert!(!err, "{out}");
        assert!(state(&mut s)["tabs"][0]["new_branch"].is_null());
        assert_eq!(locals(&mut s), before);

        // Enter on an empty field cancels.
        open(&mut s);
        call(&mut s, "key", json!({"key": "Enter"}));
        assert!(state(&mut s)["tabs"][0]["new_branch"].is_null());

        // A taken or invalid name: Enter keeps the field open and creates nothing.
        for name in [before[0].as_str(), "a b", "x.."] {
            open(&mut s);
            call(&mut s, "type_text", json!({"text": name}));
            call(&mut s, "key", json!({"key": "Enter"}));
            call(&mut s, "wait_idle", json!({}));
            assert_eq!(
                state(&mut s)["tabs"][0]["new_branch"]["name"],
                json!(name),
                "{name} should stay in the field"
            );
            assert_eq!(locals(&mut s), before);
            call(&mut s, "key", json!({"key": "Escape"}));
            assert!(state(&mut s)["tabs"][0]["new_branch"].is_null());
        }

        // Enter with a new name: created and checked out.
        open(&mut s);
        call(&mut s, "type_text", json!({"text": "feature/inline"}));
        call(&mut s, "key", json!({"key": "Enter"}));
        call(&mut s, "wait_idle", json!({}));
        let st = state(&mut s);
        assert_eq!(
            st["tabs"][0]["head_branch"],
            json!("feature/inline"),
            "{st}"
        );
        assert!(st["tabs"][0]["new_branch"].is_null());
    }

    /// A commit outside the loaded history has no row to put the field on: the dialog opens.
    #[test]
    fn new_branch_on_an_unloaded_commit_uses_the_dialog() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let h = s.session.as_mut().unwrap();
        let tab = &mut h.state_mut().tabs_mut()[0];
        let at = tab.data.as_ref().unwrap().commits[0].sha.clone();
        tab.row_of.remove(&at);
        h.state_mut()
            .apply(0, vec![Action::Input(InputKind::Branch { at })]);
        h.run_steps(2);
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(state["dialog"]["title"], json!("Create branch"), "{state}");
        assert!(state["tabs"][0]["new_branch"].is_null());
        call(&mut s, "type_text", json!({"text": "feature/from-dialog"}));
        call(&mut s, "key", json!({"key": "Enter"}));
        call(&mut s, "wait_idle", json!({}));
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        assert_eq!(
            state["tabs"][0]["head_branch"],
            json!("feature/from-dialog")
        );
    }

    /// In the Pull menu the radio sets the button's default; the name runs the action once.
    #[test]
    fn pull_menu_radio_sets_default_name_runs_once() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let button = |s: &mut Server| {
            let (_, w) = call(s, "widgets", json!({"role": "Button", "label": "Fetch"}));
            w.lines().any(|l| l.contains("Button \"Fetch\""))
        };
        let status = |s: &mut Server| -> Value {
            let state: Value =
                serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
            state["status_line"]["text"].clone()
        };
        let clear = |s: &mut Server| s.session.as_mut().unwrap().state_mut().clear_status();
        // The name runs a fetch, but the button stays "Pull".
        clear(&mut s);
        fetch(&mut s);
        assert_eq!(status(&mut s), "Fetch done", "the name should run a fetch");
        assert!(!button(&mut s), "running once must not change the default");
        // The radio makes Fetch All the default without running anything; the menu stays open.
        clear(&mut s);
        call(&mut s, "click", json!({"label": "Pull options"}));
        let (err, out) = call(&mut s, "click", json!({"label": "Default: Fetch All"}));
        assert!(!err, "{out}");
        let (_, still) = call(
            &mut s,
            "widgets",
            json!({"label": "Default: Pull (rebase)"}),
        );
        assert!(
            still.contains("RadioButton"),
            "menu should stay open: {still}"
        );
        call(&mut s, "wait_idle", json!({}));
        assert_eq!(
            status(&mut s),
            Value::Null,
            "choosing a default must not run it"
        );
        call(&mut s, "key", json!({"key": "Escape"}));
        assert!(button(&mut s), "the button should now read Fetch");
    }

    /// A background fetch (on open, tab switch, timer) must not swallow a click on Pull.
    #[test]
    fn pull_works_while_a_background_fetch_runs() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let h = s.session.as_mut().unwrap();
        h.state_mut().tabs_mut()[0]
            .running
            .push_back("Fetch".into());
        h.run_steps(2);
        call(&mut s, "click", json!({"label": "Pull", "role": "Button"}));
        // The pretend fetch never finishes; the pull's completion pops it instead, so clear the
        // queue of labels and let the real pull report.
        s.session.as_mut().unwrap().state_mut().tabs_mut()[0]
            .running
            .clear();
        call(&mut s, "wait_idle", json!({}));
        let state: Value =
            serde_json::from_str(&call(&mut s, "state", json!({"commits": 0})).1).unwrap();
        let status = state["status_line"]["text"].as_str().unwrap_or_default();
        assert!(status.starts_with("Pull"), "pull did not run: {status}");
    }

    /// Double-clicking a column border fits the column to its contents, like Excel.
    #[test]
    fn double_click_fits_columns() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let cols = |s: &mut Server| -> Value {
            let state: Value =
                serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap();
            state["tabs"][0]["columns"].clone()
        };
        // Wide enough for the author column (hidden in narrow commit lists).
        call(&mut s, "resize", json!({"width": 2000, "height": 900}));
        let before = cols(&mut s);
        assert_eq!(before["author"], 150.0, "{before}");
        // Author names in the demo ("Demo Developer") need less than the default 150.
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Resize author column", "double": true}),
        );
        assert!(!err, "{out}");
        let author = cols(&mut s)["author"].as_f64().unwrap();
        assert!(author < 150.0 && author > 60.0, "author fitted to {author}");
        // The branch column fits the widest chips: wider than the default 200 here
        // ("feature/PROJ-523-ai-search" with icons).
        call(
            &mut s,
            "click",
            json!({"label": "Resize branch column", "double": true}),
        );
        let branch = cols(&mut s)["branch"].as_f64().unwrap();
        assert!(branch != 200.0, "branch column not fitted: {branch}");
        // The message fits the longest subject; the author column takes what is left.
        call(
            &mut s,
            "click",
            json!({"label": "Resize message column", "double": true}),
        );
        let after = cols(&mut s);
        assert!(after["author"].as_f64().unwrap() >= 60.0, "{after}");
        assert_eq!(after["graph"], Value::Null, "graph follows the lanes");
        if std::env::var_os("BB_SAVE_SHOTS").is_some() {
            let save = |s: &mut Server, name: &str| {
                let shot = s.call("screenshot", &json!({}));
                let d = shot["content"][0]["data"].as_str().unwrap();
                let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
                std::fs::write(std::env::temp_dir().join(name), png).unwrap();
            };
            save(&mut s, "bb-fitted.png");
            let h = s.session.as_mut().unwrap();
            h.state_mut().tabs_mut()[0].graph_col_w = Some(56.0);
            h.run_steps(3);
            save(&mut s, "bb-narrow-graph.png");
        }
    }

    /// Busy labels ("Fetching…", "Pulling…", "Pushing…") are wider; the buttons stay put.
    #[test]
    fn fetching_does_not_move_pull_and_push() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let at = |s: &mut Server, label: &str| {
            let (_, w) = call(s, "widgets", json!({"label": label, "role": "Button"}));
            w.lines()
                .next()
                .unwrap()
                .split(" at ")
                .nth(1)
                .unwrap()
                .to_string()
        };
        let (pull, push) = (at(&mut s, "Pull"), at(&mut s, "Push"));
        let h = s.session.as_mut().unwrap();
        h.state_mut().tabs_mut()[0]
            .running
            .push_back("Fetch".into());
        h.run_steps(6);
        assert_eq!(at(&mut s, "Pull"), pull, "Pull moved while fetching");
        assert_eq!(at(&mut s, "Push"), push, "Push moved while fetching");
        let branch = at(&mut s, "Branch");
        let h = s.session.as_mut().unwrap();
        h.state_mut().tabs_mut()[0].running.push_back("Pull".into());
        h.run_steps(6);
        assert_eq!(at(&mut s, "Push"), push, "Push moved while pulling");
        let shot = s.call("screenshot", &json!({}));
        if std::env::var_os("BB_SAVE_SHOTS").is_some()
            && let Some(d) = shot["content"][0]["data"].as_str()
        {
            let png = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
            std::fs::write(std::env::temp_dir().join("bb-pulling.png"), png).unwrap();
        }
        let h = s.session.as_mut().unwrap();
        h.state_mut().tabs_mut()[0].running.push_back("Push".into());
        h.run_steps(6);
        assert_eq!(at(&mut s, "Branch"), branch, "Branch moved while pushing");
    }

    /// While git works in a tab, the top line of the tab animates instead of a spinner that
    /// widened the tab and pushed the others aside.
    #[test]
    fn busy_tab_keeps_its_size() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let close_x = |s: &mut Server| {
            let (_, w) = call(
                s,
                "widgets",
                json!({"label": "Close work", "role": "Button"}),
            );
            w.lines().next().unwrap().to_string()
        };
        let before = close_x(&mut s);
        let h = s.session.as_mut().unwrap();
        h.state_mut().tabs_mut()[0]
            .running
            .push_back("Fetch".into());
        h.run_steps(3);
        assert_eq!(close_x(&mut s), before, "tab close button moved");
        s.session.as_mut().unwrap().state_mut().tabs_mut()[0]
            .running
            .clear();
    }

    /// A confirmation is a bar under the toolbar, not a dialog. Escape cancels it, Enter does not
    /// confirm a destructive one, and its button runs the action.
    #[test]
    fn discard_all_asks_in_the_alert_bar() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let state = |s: &mut Server| -> Value {
            serde_json::from_str(&call(s, "state", json!({"commits": 0})).1).unwrap()
        };
        let changed = |st: &Value| {
            st["tabs"][0]["staged"].as_array().unwrap().len()
                + st["tabs"][0]["unstaged"].as_array().unwrap().len()
        };
        assert!(changed(&state(&mut s)) > 0);
        let (err, out) = call(&mut s, "click", json!({"label": "WIP", "role": "Button"}));
        assert!(!err, "{out}");
        let ask = |s: &mut Server| {
            let (err, out) = call(s, "click", json!({"label": "Discard all changes…"}));
            assert!(!err, "{out}");
            assert!(out.contains("alert: Discard all changes?"), "{out}");
            let st = state(s);
            assert!(st["dialog"].is_null(), "not a dialog: {st}");
            assert_eq!(st["tabs"][0]["alert"]["danger"], json!(true));
        };
        ask(&mut s);
        // Question and buttons sit together in the middle, not at the far ends.
        let (_, w) = call(
            &mut s,
            "widgets",
            json!({"label": "Cancel", "role": "Button"}),
        );
        let x: f64 = w
            .split(" at (")
            .nth(1)
            .and_then(|r| r.split(',').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{w}"));
        assert!(x < 1100.0, "buttons should be centred with the text: {w}");
        // A narrow window cuts the message short; the buttons stay on screen.
        call(&mut s, "resize", json!({"width": 720, "height": 600}));
        let (_, w) = call(
            &mut s,
            "widgets",
            json!({"label": "Discard all changes", "role": "Button"}),
        );
        let right: f64 = w
            .lines()
            .find(|l| l.contains("\"Discard all changes\""))
            .and_then(|l| {
                let (pos, size) = l.split_once(" at (")?.1.split_once(") ")?;
                let x: f64 = pos.split(',').next()?.parse().ok()?;
                let wd: f64 = size.split('x').next()?.parse().ok()?;
                Some(x + wd)
            })
            .unwrap_or_else(|| panic!("{w}"));
        assert!(right <= 720.0, "button off screen: {w}");
        call(&mut s, "resize", json!({"width": 1400, "height": 860}));
        call(&mut s, "key", json!({"key": "Enter"}));
        let st = state(&mut s);
        assert!(
            !st["tabs"][0]["alert"].is_null(),
            "Enter must not confirm: {st}"
        );
        call(&mut s, "key", json!({"key": "Escape"}));
        let st = state(&mut s);
        assert!(st["tabs"][0]["alert"].is_null(), "Escape cancels: {st}");
        assert!(changed(&st) > 0);

        ask(&mut s);
        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Discard all changes", "role": "Button", "confirm_dangerous": true}),
        );
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        let st = state(&mut s);
        assert!(st["tabs"][0]["alert"].is_null());
        assert_eq!(changed(&st), 0, "{st}");
    }

    /// An assistant cannot confirm a destructive action unless the call says so explicitly.
    #[test]
    fn destructive_confirmation_needs_explicit_consent() {
        let mut s = Server::default();
        let (err, out) = call(&mut s, "demo_repo", json!({}));
        assert!(!err, "{out}");
        let dir = s.demo_dirs[0].clone();
        call(
            &mut s,
            "click",
            json!({"label": "develop", "button": "right", "nth": 2}),
        );
        let (_, out) = call(
            &mut s,
            "click",
            json!({"label": "Delete develop on origin"}),
        );
        assert!(out.contains("alert: Delete develop on origin?"), "{out}");

        let (_, out) = call(&mut s, "click", json!({"label": "Delete origin/develop"}));
        assert!(out.contains("confirm_dangerous"), "{out}");
        // A failed call with the flag must not leave it switched on for the next click.
        let (err, _) = call(
            &mut s,
            "click",
            json!({"label": "Delete origin/develop", "confirm_dangerous": true, "modifiers": ["bogus"]}),
        );
        assert!(err);
        let (err, _) = call(
            &mut s,
            "click",
            json!({"label": "Delete origin/develop", "confirm_dangerous": true, "double": true}),
        );
        assert!(err);
        call(
            &mut s,
            "type_text",
            json!({"label": "Delete origin/develop", "text": ""}),
        );
        call(&mut s, "click", json!({"label": "Delete origin/develop"}));
        assert!(remote_branches(&mut s).contains(&"origin/develop".to_string()));

        let (err, out) = call(
            &mut s,
            "click",
            json!({"label": "Delete origin/develop", "confirm_dangerous": true}),
        );
        assert!(!err, "{out}");
        call(&mut s, "wait_idle", json!({}));
        assert!(!remote_branches(&mut s).contains(&"origin/develop".to_string()));

        // The demo repository is removed with the server.
        drop(s);
        assert!(!dir.exists());
    }

    #[test]
    fn view_and_input_tools() {
        let mut s = Server::default();
        call(&mut s, "demo_repo", json!({}));
        let r = s.call("screenshot", &json!({}));
        match r["content"][0]["type"].as_str() {
            Some("image") => {
                let png = base64::engine::general_purpose::STANDARD
                    .decode(r["content"][0]["data"].as_str().unwrap())
                    .unwrap();
                assert!(png.starts_with(b"\x89PNG"));
            }
            // Machines without a graphics adapter get a clear error, not a crash.
            _ => assert!(
                r["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("screenshot")
            ),
        }

        // The sidebar filter has no label, but is listed and can be typed into by position.
        let (_, inputs) = call(&mut s, "widgets", json!({"role": "TextInput"}));
        let first = inputs.lines().next().unwrap();
        let at = first.split(" at (").nth(1).unwrap();
        let (x, rest) = at.split_once(',').unwrap();
        let y = rest.split(')').next().unwrap();
        let (x, y): (f64, f64) = (
            x.parse::<f64>().unwrap() + 20.0,
            y.parse::<f64>().unwrap() + 8.0,
        );
        let (err, out) = call(
            &mut s,
            "type_text",
            json!({"text": "hotfix", "x": x, "y": y}),
        );
        assert!(!err, "{out}");
        // The graph still labels main; the sidebar (left of x = 280) must not list it.
        let (_, found) = call(&mut s, "widgets", json!({"label": "main"}));
        let in_sidebar = found.lines().any(|l| {
            l.contains("Button \"main\"")
                && l.split(" at (")
                    .nth(1)
                    .and_then(|r| r.split(',').next())
                    .and_then(|x| x.parse::<f32>().ok())
                    .is_some_and(|x| x < 280.0)
        });
        assert!(!in_sidebar, "filter should hide main: {found}");

        for (tool, args) in [
            ("set_theme", json!({"theme": "light"})),
            ("resize", json!({"width": 1000, "height": 700})),
            ("scroll", json!({"dy": 200})),
            ("hover", json!({"label": "Pull"})),
        ] {
            let (err, out) = call(&mut s, tool, args);
            assert!(!err, "{tool}: {out}");
        }
        let (err, _) = call(&mut s, "resize", json!({"width": 10000, "height": 700}));
        assert!(err);
        let (err, _) = call(&mut s, "wait_idle", json!({"timeout_ms": "soon"}));
        assert!(err);
    }
}
