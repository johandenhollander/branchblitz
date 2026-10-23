//! The repository window's panels, drawn each frame; they collect `Action`s for the app.

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

mod agent_panel;
mod banner;
mod conflict_view;
mod details;
mod diff;
mod graph;
mod menus;
mod sidebar;

use conflict_view::conflict_view;
use diff::pill;
use graph::{pill_lane, pill_menu, pills_for};
use menus::commit_menu;
use sidebar::checkout_item;

pub use agent_panel::{AGENT_BAR_H, AgentChoice, agent_bar, agent_panel, agent_splitter};
pub use banner::{alert_bar, state_banner};
pub use details::details;
pub use diff::diff_view;
pub use graph::graph_view;
pub use menus::ref_menu;
pub(crate) use sidebar::checkout_action;
pub use sidebar::sidebar;

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
}
