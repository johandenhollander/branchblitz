//! File history and blame of one file, in place of the graph: opened from a diff's header.
//! History lists the commits that changed the file (click one for its diff); blame shows who last
//! changed each line (click a line for its commit).

use eframe::egui::{self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::app::{Action, RepoTab, Selection};
use crate::git::{self, BlameLine, DiffTarget, FileCommit};
use crate::theme::Palette;
use crate::{ds, icons, views};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    History,
    Blame,
}

pub enum Content {
    History(Vec<FileCommit>),
    Blame(Vec<BlameLine>),
}

pub struct FileView {
    pub kind: Kind,
    pub path: String,
    /// The commit it is seen from; `None`: HEAD (history) or the working tree (blame).
    pub rev: Option<String>,
    pub content: Option<Result<Content, String>>,
    pub generation: u64,
}

impl FileView {
    /// What the MCP `state` shows.
    pub fn summary(&self) -> serde_json::Value {
        let rows = match &self.content {
            Some(Ok(Content::History(h))) => Some(h.len()),
            Some(Ok(Content::Blame(b))) => Some(b.len()),
            _ => None,
        };
        serde_json::json!({
            "kind": format!("{:?}", self.kind),
            "path": self.path,
            "rev": self.rev,
            "rows": rows,
            "error": match &self.content { Some(Err(e)) => Some(e), _ => None },
        })
    }
}

/// Runs in a background thread.
pub fn load(
    repo: &std::path::Path,
    kind: Kind,
    path: &str,
    rev: Option<&str>,
) -> Result<Content, String> {
    match kind {
        Kind::History => git::file_history(repo, path, rev, 5000).map(Content::History),
        Kind::Blame => git::blame(repo, path, rev).map(Content::Blame),
    }
}

/// The header buttons that open history or blame for the file of a diff.
pub fn diff_buttons(
    ui: &mut Ui,
    target: &DiffTarget,
    status: char,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    let (path, rev) = match target {
        DiffTarget::Commit { sha, path } => (path, Some(sha.clone())),
        DiffTarget::Unstaged {
            path,
            untracked: false,
        }
        | DiffTarget::Staged { path } => (path, None),
        _ => return,
    };
    // A deleted file has no lines left to blame.
    if status != 'D'
        && ds::Button::icon_only(icons::USER_ROUND_PEN, "Blame")
            .quiet()
            .small()
            .show(ui, pal)
            .clicked()
    {
        acts.push(Action::FileView {
            kind: Kind::Blame,
            path: path.clone(),
            rev: rev.clone(),
        });
    }
    if ds::Button::icon_only(icons::HISTORY, "File history")
        .quiet()
        .small()
        .show(ui, pal)
        .clicked()
    {
        acts.push(Action::FileView {
            kind: Kind::History,
            path: path.clone(),
            rev,
        });
    }
}

pub fn show(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some(fv) = &tab.file_view else { return };
    // Header: icon, path, where it is seen from, History/Blame, close.
    egui::Frame::new()
        .fill(pal.app)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_height(36.0);
            ui.set_width(ui.available_width());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let icon = match fv.kind {
                    Kind::History => icons::HISTORY,
                    Kind::Blame => icons::USER_ROUND_PEN,
                };
                ui.label(RichText::new(icon).size(14.0).color(pal.text_muted));
                ui.add(
                    egui::Label::new(
                        RichText::new(&fv.path)
                            .monospace()
                            .size(12.0)
                            .variation("wght", 500.0)
                            .color(pal.text),
                    )
                    .truncate(),
                );
                let at = match (&fv.rev, fv.kind) {
                    (Some(sha), _) => format!("at {}", git::short(sha)),
                    (None, Kind::History) => "from HEAD".into(),
                    (None, Kind::Blame) => "working tree".into(),
                };
                ui.label(RichText::new(at).size(12.5).color(pal.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ds::Button::icon_only(icons::X, "Close (Esc)")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::CloseFileView);
                    }
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.border);
                    for (kind, text) in [(Kind::Blame, "Blame"), (Kind::History, "History")] {
                        let b = ds::Button::new(text).small().label(format!("Show {text}"));
                        let b = if fv.kind == kind {
                            b.primary()
                        } else {
                            b.quiet()
                        };
                        if b.show(ui, pal).clicked() && fv.kind != kind {
                            acts.push(Action::FileView {
                                kind,
                                path: fv.path.clone(),
                                rev: fv.rev.clone(),
                            });
                        }
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(Action::CloseFileView);
    }
    let selected = match &tab.selected {
        Selection::Commit(sha) => Some(sha.as_str()),
        _ => None,
    };
    match &fv.content {
        None => {
            ui.add_space(16.0);
            ui.spinner();
        }
        Some(Err(e)) => {
            ui.add_space(16.0);
            ui.colored_label(pal.danger, e);
        }
        Some(Ok(Content::History(h))) => history(ui, h, selected, pal, acts),
        Some(Ok(Content::Blame(b))) => blame(ui, b, selected, pal, acts),
    }
}

/// The commits that changed the file; a click selects one and opens its diff of the file.
fn history(
    ui: &mut Ui,
    h: &[FileCommit],
    selected: Option<&str>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    if h.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("No commits changed this file").color(pal.text_muted));
        return;
    }
    let row_h = 28.0;
    let mono = FontId::monospace(12.0);
    let body = FontId::proportional(13.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("file-history")
        .show_rows(ui, row_h, h.len(), |ui, range| {
            for c in &h[range] {
                let label = format!("{} {}", git::short(&c.sha), c.subject);
                let resp = ds::row(ui, pal, row_h, selected == Some(c.sha.as_str()), &label);
                let r = resp.rect.shrink2(vec2(12.0, 0.0));
                let p = ui.painter_at(resp.rect);
                let y = r.center().y;
                p.text(
                    pos2(r.left(), y),
                    Align2::LEFT_CENTER,
                    git::short(&c.sha),
                    mono.clone(),
                    pal.text_subtle,
                );
                // Date and author from the right; the subject gets the rest.
                let date = p.text(
                    pos2(r.right(), y),
                    Align2::RIGHT_CENTER,
                    views::fmt_time(c.time),
                    body.clone(),
                    pal.text_muted,
                );
                let author =
                    ui.painter()
                        .layout_no_wrap(c.author.clone(), body.clone(), pal.text_muted);
                let author_x =
                    (date.left() - 16.0 - author.size().x.min(160.0)).max(r.left() + 200.0);
                let subject_r = Rect::from_x_y_ranges(
                    (r.left() + 72.0)..=(author_x - 16.0),
                    resp.rect.y_range(),
                );
                p.with_clip_rect(subject_r).text(
                    pos2(subject_r.left(), y),
                    Align2::LEFT_CENTER,
                    &c.subject,
                    body.clone(),
                    pal.text,
                );
                let author_r =
                    Rect::from_x_y_ranges(author_x..=(date.left() - 16.0), resp.rect.y_range());
                p.with_clip_rect(author_r).galley(
                    pos2(author_x, y - author.size().y / 2.0),
                    author,
                    pal.text_muted,
                );
                if c.path != h[0].path {
                    resp.clone().on_hover_text(format!("As {}", c.path));
                }
                if resp.clicked() {
                    acts.push(Action::Jump(c.sha.clone()));
                    acts.push(Action::ShowDiff(DiffTarget::Commit {
                        sha: c.sha.clone(),
                        path: c.path.clone(),
                    }));
                }
            }
        });
}

/// Every line with the commit that last changed it; the commit's details on the first line of
/// each run of lines from the same commit. A click on a line selects its commit.
fn blame(
    ui: &mut Ui,
    lines: &[BlameLine],
    selected: Option<&str>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    if lines.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("The file is empty").color(pal.text_muted));
        return;
    }
    let font = ui.style().text_styles[&egui::TextStyle::Monospace].clone();
    let line_h = (font.size * 1.66).round();
    let small = FontId::proportional(12.0);
    let num_font = FontId::monospace(font.size - 1.0);
    let gutter = 360.0;
    let code_x = gutter + 52.0;
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("blame")
        .show_rows(ui, line_h, lines.len(), |ui, range| {
            for k in range {
                let l = &lines[k];
                let first = k == 0 || lines[k - 1].sha != l.sha;
                let uncommitted = l.sha.bytes().all(|b| b == b'0');
                let g = ui.painter().layout_no_wrap(
                    l.text.replace('\t', "    "),
                    font.clone(),
                    pal.text,
                );
                let w = (code_x + g.size().x + 12.0).max(ui.available_width());
                let (rect, resp) = ui.allocate_exact_size(vec2(w, line_h), Sense::click());
                resp.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        true,
                        format!("Blame line {}: {} {}", k + 1, git::short(&l.sha), l.summary),
                    )
                });
                if !uncommitted {
                    let resp = resp.clone().on_hover_text(format!(
                        "{}\n{}, {}",
                        l.summary,
                        l.author,
                        views::fmt_time(l.time)
                    ));
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        acts.push(Action::Jump(l.sha.clone()));
                    }
                }
                let p = ui.painter();
                let gutter_r = Rect::from_min_size(rect.min, vec2(gutter, line_h));
                let picked = selected == Some(l.sha.as_str());
                let bg = if picked {
                    pal.selected
                } else if resp.hovered() {
                    pal.hover
                } else {
                    Color32::TRANSPARENT
                };
                p.rect_filled(rect, 0.0, bg);
                p.rect_filled(gutter_r, 0.0, pal.app.gamma_multiply(0.6));
                p.vline(
                    gutter_r.right(),
                    rect.y_range(),
                    Stroke::new(1.0, pal.border),
                );
                if first {
                    if k > 0 {
                        p.hline(rect.x_range(), rect.top(), Stroke::new(1.0, pal.border));
                    }
                    let y = rect.center().y;
                    let left = rect.left() + 10.0;
                    let date = if uncommitted {
                        String::new()
                    } else {
                        views::fmt_time(l.time)
                    };
                    let d = p.text(
                        pos2(gutter_r.right() - 8.0, y),
                        Align2::RIGHT_CENTER,
                        date,
                        small.clone(),
                        pal.text_subtle,
                    );
                    let text = if uncommitted {
                        l.summary.clone()
                    } else {
                        format!("{}  {} · {}", git::short(&l.sha), l.author, l.summary)
                    };
                    let clip = Rect::from_x_y_ranges(left..=(d.left() - 8.0), rect.y_range());
                    p.with_clip_rect(clip).text(
                        pos2(left, y),
                        Align2::LEFT_CENTER,
                        text,
                        small.clone(),
                        pal.text_muted,
                    );
                }
                p.text(
                    pos2(gutter_r.right() + 40.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    (k + 1).to_string(),
                    num_font.clone(),
                    pal.text_subtle,
                );
                p.galley(
                    pos2(rect.left() + code_x, rect.center().y - g.size().y / 2.0),
                    g,
                    pal.text,
                );
            }
        });
}
