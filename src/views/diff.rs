//! The diff of one file, with staging of hunks and lines.

use super::*;

// ---------------------------------------------------------------- diff view

/// One line of a unified diff with its old and new line numbers.
struct DiffLine<'a> {
    old: Option<usize>,
    new: Option<usize>,
    kind: char,
    text: &'a str,
}

/// The hunks of a unified diff; the file header lines before the first hunk are dropped.
fn parse_diff(text: &str) -> Vec<DiffLine<'_>> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0usize, 0usize);
    let mut in_hunk = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@") {
            in_hunk = true;
            // "@@ -12,7 +12,8 @@ fn x"
            let mut parts = rest.split_whitespace();
            let num = |s: Option<&str>| {
                s.and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            old = num(parts.next());
            new = num(parts.next());
            out.push(DiffLine {
                old: None,
                new: None,
                kind: '@',
                text: line,
            });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let kind = line
            .chars()
            .next()
            .filter(|c| matches!(c, '+' | '-' | '\\'))
            .unwrap_or(' ');
        let text = match kind {
            '\\' => line,
            _ => line.get(1..).unwrap_or(""),
        };
        let (o, n) = match kind {
            '+' => (None, Some(new)),
            '-' => (Some(old), None),
            '\\' => (None, None),
            _ => (Some(old), Some(new)),
        };
        match kind {
            '+' => new += 1,
            '-' => old += 1,
            '\\' => {}
            _ => {
                old += 1;
                new += 1;
            }
        }
        out.push(DiffLine {
            old: o,
            new: n,
            kind,
            text,
        });
    }
    out
}

pub fn diff_view(ui: &mut Ui, tab: &mut RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some((target, content)) = &tab.diff else {
        return;
    };
    if let DiffTarget::Conflict { path } = target {
        let (path, content) = (path.clone(), content.clone());
        return conflict_view(ui, tab, &path, content, pal, acts);
    }
    let lines = match content {
        Some(Ok(t)) => parse_diff(t),
        _ => Vec::new(),
    };
    // Staging part of a file: in the working-copy and the staged diff of a tracked file.
    let raw = match content {
        Some(Ok(t)) => t.as_str(),
        _ => "",
    };
    let partial = matches!(
        target,
        DiffTarget::Unstaged {
            untracked: false,
            ..
        } | DiffTarget::Staged { .. }
    );
    let unstage = matches!(target, DiffTarget::Staged { .. });
    let verb = if unstage { "Unstage" } else { "Stage" };
    // Per parsed line: its hunk and its index in that hunk's body (as `git::partial_patch` counts).
    let mut hunk_pos = Vec::with_capacity(lines.len());
    let (mut h, mut i) = (usize::MAX, 0);
    for l in &lines {
        if l.kind == '@' {
            h = h.wrapping_add(1);
            i = 0;
            hunk_pos.push((h, usize::MAX));
        } else {
            hunk_pos.push((h, i));
            i += 1;
        }
    }
    let selection = &mut tab.diff_selection;
    let anchor = &mut tab.diff_anchor;
    let (adds, dels) = lines.iter().fold((0, 0), |(a, d), l| match l.kind {
        '+' => (a + 1, d),
        '-' => (a, d + 1),
        _ => (a, d),
    });
    // Status letter of the file, from the panel it was opened from.
    let status = match (target, &tab.details, &tab.data) {
        (DiffTarget::Commit { path, .. }, Some(d), _) => {
            d.files.iter().find(|f| &f.path == path).map(|f| f.status)
        }
        (
            DiffTarget::Unstaged {
                untracked: true, ..
            },
            _,
            _,
        ) => Some('A'),
        (DiffTarget::Unstaged { path, .. }, _, Some(data)) => data
            .status
            .unstaged
            .iter()
            .find(|f| &f.path == path)
            .map(|f| f.status),
        (DiffTarget::Staged { path }, _, Some(data)) => data
            .status
            .staged
            .iter()
            .find(|f| &f.path == path)
            .map(|f| f.status),
        _ => None,
    }
    .unwrap_or('M');

    // Header: status, path, where it comes from, +/− stat, close at the far right.
    egui::Frame::new()
        .fill(pal.app)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_height(36.0);
            ui.set_width(ui.available_width());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ds::status_letter(ui.painter(), r.center(), status, pal);
                ui.add(
                    egui::Label::new(
                        RichText::new(target.path())
                            .monospace()
                            .size(12.0)
                            .variation("wght", 500.0)
                            .color(pal.text),
                    )
                    .truncate(),
                );
                match target {
                    DiffTarget::Commit { sha, .. } => {
                        ui.label(
                            RichText::new(format!("in {}", git::short(sha)))
                                .size(12.5)
                                .color(pal.text_muted),
                        );
                    }
                    DiffTarget::Unstaged { .. } => pill(ui, pal, "Unstaged"),
                    DiffTarget::Staged { .. } => pill(ui, pal, "Staged"),
                    DiffTarget::Conflict { .. } => pill(ui, pal, "Conflict"),
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ds::Button::icon_only(icons::X, "Close (Esc)")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::CloseDiff);
                    }
                    crate::file_view::diff_buttons(ui, target, status, pal, acts);
                    if partial && !selection.is_empty() {
                        let n = selection.len();
                        let label = format!("{verb} {n} line{}", if n == 1 { "" } else { "s" });
                        if ds::Button::new(&label)
                            .primary()
                            .small()
                            .tooltip("Shift-click extends the selection, Esc clears it")
                            .show(ui, pal)
                            .clicked()
                        {
                            let chosen: std::collections::HashSet<(usize, usize)> =
                                selection.iter().map(|&k| hunk_pos[k]).collect();
                            if let Some(patch) =
                                git::partial_patch(raw, |h, i| chosen.contains(&(h, i)), unstage)
                            {
                                acts.push(Action::Op(Op::ApplyPatch { patch, unstage }));
                            }
                        }
                    }
                    let (sep, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(sep, 0.0, pal.border);
                    ui.label(
                        RichText::new(format!("−{dels}"))
                            .monospace()
                            .size(11.5)
                            .color(pal.removed),
                    );
                    ui.label(
                        RichText::new(format!("+{adds}"))
                            .monospace()
                            .size(11.5)
                            .color(pal.added),
                    );
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if selection.is_empty() {
            acts.push(Action::CloseDiff);
        } else {
            selection.clear();
        }
    }
    match content {
        None => {
            ui.add_space(16.0);
            ui.spinner();
            return;
        }
        Some(Err(e)) => {
            ui.add_space(16.0);
            ui.colored_label(pal.danger, e);
            return;
        }
        Some(Ok(_)) => {}
    }
    if lines.is_empty() {
        ui.add_space(16.0);
        ui.label(RichText::new("No textual changes").color(pal.text_muted));
        return;
    }
    let font = ui.style().text_styles[&egui::TextStyle::Monospace].clone();
    let line_h = (font.size * 1.66).round();
    let num_font = FontId::monospace(font.size - 1.0);
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("diff")
        .show_rows(ui, line_h, lines.len(), |ui, range| {
            for (k, l) in lines.iter().enumerate().skip(range.start).take(range.len()) {
                let change = matches!(l.kind, '+' | '-');
                let picked = selection.contains(&k);
                let (bg, sign_c, fg) = match l.kind {
                    '+' => (pal.added_bg, pal.added, pal.text),
                    '-' => (pal.removed_bg, pal.removed, pal.text),
                    '@' => (
                        crate::theme::mix(pal.canvas, pal.hunk, 0.07),
                        pal.hunk,
                        pal.hunk,
                    ),
                    '\\' => (Color32::TRANSPARENT, pal.text_subtle, pal.text_subtle),
                    _ => (Color32::TRANSPARENT, pal.text_subtle, pal.text),
                };
                let g = ui
                    .painter()
                    .layout_no_wrap(l.text.replace('\t', "    "), font.clone(), fg);
                // Old and new line numbers (40 px each), the sign column, then the code.
                let code_x = if l.kind == '@' { 12.0 } else { 96.0 };
                let w = (code_x + g.size().x + 12.0).max(ui.available_width());
                let sense = if partial && change {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let (rect, resp) = ui.allocate_exact_size(vec2(w, line_h), sense);
                // A changed line: click to choose it, Shift-click for the lines up to it.
                if partial && change {
                    resp.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            true,
                            picked,
                            format!("Line {}: {}", l.new.or(l.old).unwrap_or(0), l.text.trim()),
                        )
                    });
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        if ui.input(|i| i.modifiers.shift)
                            && let Some(a) = *anchor
                        {
                            let (lo, hi) = (a.min(k), a.max(k));
                            selection
                                .extend((lo..=hi).filter(|&j| matches!(lines[j].kind, '+' | '-')));
                        } else if !selection.remove(&k) {
                            selection.insert(k);
                        }
                        *anchor = Some(k);
                    }
                }
                let p = ui.painter();
                p.rect_filled(rect, 0.0, bg);
                if picked {
                    p.rect_filled(rect, 0.0, pal.accent.gamma_multiply(0.18));
                    p.rect_filled(
                        Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                        0.0,
                        pal.accent,
                    );
                }
                let yc = rect.center().y;
                if l.kind != '@' {
                    for (n, x) in [(l.old, 40.0), (l.new, 80.0)] {
                        if let Some(n) = n {
                            p.text(
                                pos2(rect.left() + x - 8.0, yc),
                                Align2::RIGHT_CENTER,
                                n.to_string(),
                                num_font.clone(),
                                pal.text_subtle,
                            );
                        }
                    }
                    if matches!(l.kind, '+' | '-') {
                        p.text(
                            pos2(rect.left() + 84.0, yc),
                            Align2::LEFT_CENTER,
                            l.kind.to_string(),
                            font.clone(),
                            sign_c,
                        );
                    }
                }
                p.galley(pos2(rect.left() + code_x, yc - g.size().y / 2.0), g, fg);
                // The hunk header: stage (or unstage) the whole hunk, at the visible right edge.
                if partial && l.kind == '@' {
                    let hunk = hunk_pos[k].0;
                    let right = ui.clip_rect().right() - 8.0;
                    let r = Rect::from_min_max(
                        pos2(right - 104.0, rect.top() + 2.0),
                        pos2(right, rect.bottom() - 2.0),
                    );
                    let b = ui.put(r, |ui: &mut Ui| {
                        ds::Button::new(format!("{verb} hunk"))
                            .small()
                            .quiet()
                            .label(format!("{verb} hunk {}", hunk + 1))
                            .show(ui, pal)
                    });
                    if b.clicked()
                        && let Some(patch) = git::partial_patch(raw, |h, _| h == hunk, unstage)
                    {
                        acts.push(Action::Op(Op::ApplyPatch { patch, unstage }));
                    }
                }
            }
        });
}

/// `bb-pill`: "Unstaged" / "Staged" next to a working-copy file.
pub(super) fn pill(ui: &mut Ui, pal: &Palette, text: &str) {
    let g = ui
        .painter()
        .layout_job(ds::text_job(text, 11.0, 500.0, pal.text_muted, false));
    let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 18.0), Sense::hover());
    ui.painter()
        .rect_filled(r, CornerRadius::same(9), pal.control);
    ui.painter()
        .galley(r.center() - g.size() / 2.0, g, pal.text_muted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_line_numbers() {
        let d = "diff --git a/x b/x\nindex 1..2\n--- a/x\n+++ b/x\n@@ -3,3 +3,4 @@ fn x\n a\n-b\n+c\n+d\n e\n";
        let l = parse_diff(d);
        assert_eq!(l[0].kind, '@');
        assert_eq!((l[1].old, l[1].new), (Some(3), Some(3)));
        assert_eq!((l[2].old, l[2].new, l[2].kind), (Some(4), None, '-'));
        assert_eq!((l[3].old, l[3].new, l[3].kind), (None, Some(4), '+'));
        assert_eq!((l[4].new, l[4].text), (Some(5), "d"));
        assert_eq!((l[5].old, l[5].new), (Some(5), Some(6)));
    }
}
