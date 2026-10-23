//! Resolving a conflicted file block by block.

use super::*;

// ---------------------------------------------------------------- conflict view

/// One conflicted file: per conflict block mine and incoming side by side with a choice (mine,
/// incoming, both), the plain text around them, and save once every block has a choice. During
/// a rebase git's "ours" is the upstream, so "mine" is git's "theirs" there.
pub(super) fn conflict_view(
    ui: &mut Ui,
    tab: &mut RepoTab,
    path: &str,
    content: Option<Result<String, String>>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::conflict::{self, Choice, Part};
    let Some(data) = tab.data.as_ref() else {
        return;
    };
    let mine_is_ours = data.state != RepoState::Rebasing;
    let head = data.head_branch.clone().unwrap_or_else(|| "HEAD".into());

    // Header: status, path, "Conflict", close.
    egui::Frame::new()
        .fill(pal.app)
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_height(36.0);
            ui.set_width(ui.available_width());
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                ds::status_letter(ui.painter(), r.center(), 'U', pal);
                ui.add(
                    egui::Label::new(
                        RichText::new(path)
                            .monospace()
                            .size(12.0)
                            .variation("wght", 500.0)
                            .color(pal.text),
                    )
                    .truncate(),
                );
                pill(ui, pal, "Conflict");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ds::Button::icon_only(icons::X, "Close (Esc)")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::CloseDiff);
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(Action::CloseDiff);
    }
    // Unreadable as text (e.g. not UTF-8): only the whole-file actions.
    let (text, error) = match content {
        None => {
            ui.add_space(16.0);
            ui.spinner();
            return;
        }
        Some(Err(e)) => (String::new(), Some(e)),
        Some(Ok(t)) => (t, None),
    };
    let parts = if error.is_some() {
        None
    } else {
        conflict::parse(&text)
    };
    let n = parts.as_deref().map_or(0, conflict::blocks);
    // Choices belong to this text: a file changed on disk starts over.
    let stamp = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    };
    if tab.conflict_for != stamp {
        tab.conflict_for = stamp;
        tab.conflict_choices.clear();
    }
    let choices = &mut tab.conflict_choices;
    choices.resize(n, None);
    let chosen = choices.iter().filter(|c| c.is_some()).count();

    // What the UI calls mine/incoming in git's terms, and back.
    let to_git = |mine: bool, incoming: bool| match (mine, incoming, mine_is_ours) {
        (true, false, true) | (false, true, false) => Choice::Ours,
        (true, false, false) | (false, true, true) => Choice::Theirs,
        (_, _, true) => Choice::OursThenTheirs,
        (_, _, false) => Choice::TheirsThenOurs,
    };
    let sides = |c: Choice| -> (bool, bool) {
        let (ours, theirs) = match c {
            Choice::Ours => (true, false),
            Choice::Theirs => (false, true),
            _ => (true, true),
        };
        if mine_is_ours {
            (ours, theirs)
        } else {
            (theirs, ours)
        }
    };

    // Toolbar: progress, then the actions for the whole file.
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let summary = match (&parts, &error) {
                    (_, Some(e)) => e.clone(),
                    (None, None) => "No conflict markers left in this file".to_string(),
                    (Some(_), None) => format!(
                        "{chosen} of {n} conflict{} chosen",
                        if n == 1 { "" } else { "s" }
                    ),
                };
                ui.label(RichText::new(summary).size(12.5).color(pal.text_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match &parts {
                        Some(parts) => {
                            let resolved = conflict::resolve(parts, choices);
                            if ds::Button::new("Save & mark resolved")
                                .icon(icons::CIRCLE_CHECK)
                                .primary()
                                .small()
                                .enabled(resolved.is_some())
                                .show(ui, pal)
                                .clicked()
                                && let Some(content) = resolved
                            {
                                acts.push(Action::Op(Op::ResolveFile {
                                    path: path.to_string(),
                                    content,
                                    expected: text.clone(),
                                }));
                            }
                        }
                        None => {
                            if ds::Button::new("Mark as resolved")
                                .icon(icons::CIRCLE_CHECK)
                                .primary()
                                .small()
                                .show(ui, pal)
                                .clicked()
                            {
                                acts.push(Action::Op(Op::MarkResolved(path.to_string())));
                            }
                        }
                    }
                    if ds::Button::new("Open in editor")
                        .icon(icons::PENCIL)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::OpenFile(path.to_string()));
                    }
                    if ds::Button::new("Take all incoming")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::TakeSide {
                            path: path.to_string(),
                            ours: !mine_is_ours,
                        }));
                    }
                    if ds::Button::new("Take all mine")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::Op(Op::TakeSide {
                            path: path.to_string(),
                            ours: mine_is_ours,
                        }));
                    }
                });
            });
        });
    let line = ui.available_rect_before_wrap();
    ui.painter()
        .hline(line.x_range(), line.top(), Stroke::new(1.0, pal.border));

    let Some(parts) = parts else {
        if error.is_some() {
            return;
        }
        ui.add_space(12.0);
        ui.label(
            RichText::new("Edited outside the app? Check the file, then mark it as resolved.")
                .color(pal.text_muted),
        );
        return;
    };
    let mono = ui.style().text_styles[&egui::TextStyle::Monospace].size;
    let code = |ui: &mut Ui, text: &str, color: Color32| {
        if text.is_empty() {
            ui.label(
                RichText::new("(nothing)")
                    .italics()
                    .size(mono)
                    .color(pal.text_subtle),
            );
        }
        for l in text.lines() {
            ui.label(RichText::new(l).monospace().size(mono).color(color));
        }
    };
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("conflict")
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut k = 0;
                    for part in &parts {
                        match part {
                            // Context: long stretches keep their first and last lines.
                            Part::Text(t) => {
                                let lines: Vec<&str> = t.lines().collect();
                                let show = |ui: &mut Ui, ls: &[&str]| {
                                    for l in ls {
                                        ui.label(
                                            RichText::new(*l)
                                                .monospace()
                                                .size(mono)
                                                .color(pal.text_muted),
                                        );
                                    }
                                };
                                if lines.len() > 8 {
                                    show(ui, &lines[..3]);
                                    ui.label(
                                        RichText::new(format!("… {} lines", lines.len() - 6))
                                            .size(11.5)
                                            .color(pal.text_subtle),
                                    );
                                    show(ui, &lines[lines.len() - 3..]);
                                } else {
                                    show(ui, &lines);
                                }
                            }
                            Part::Block(b) => {
                                k += 1;
                                let choice = choices[k - 1];
                                let (mine_on, inc_on) = choice.map_or((true, true), sides);
                                let (mine_text, inc_text, mine_label, inc_label) = if mine_is_ours {
                                    (&b.ours, &b.theirs, &b.ours_label, &b.theirs_label)
                                } else {
                                    (&b.theirs, &b.ours, &b.theirs_label, &b.ours_label)
                                };
                                // Git labels the HEAD side "HEAD": the current branch in a merge, the upstream in a rebase.
                                let name = |l: &String| -> String {
                                    match l.as_str() {
                                        "HEAD" if mine_is_ours => head.clone(),
                                        "HEAD" => "upstream".into(),
                                        _ => l.clone(),
                                    }
                                };
                                let (mine_label, inc_label) = (name(mine_label), name(inc_label));
                                ui.add_space(6.0);
                                egui::Frame::new()
                                    .stroke(Stroke::new(
                                        1.0,
                                        if choice.is_some() {
                                            pal.border
                                        } else {
                                            pal.danger
                                        },
                                    ))
                                    .corner_radius(CornerRadius::same(crate::theme::RADIUS_MD))
                                    .inner_margin(8.0)
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!("Conflict {k} of {n}"))
                                                    .size(12.5)
                                                    .variation("wght", 600.0)
                                                    .color(pal.text),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    for (text, mine, inc) in [
                                                        ("Both", true, true),
                                                        ("Incoming", false, true),
                                                        ("Mine", true, false),
                                                    ] {
                                                        let c = to_git(mine, inc);
                                                        let on = choice == Some(c);
                                                        let mut btn = ds::Button::new(text)
                                                            .small()
                                                            .label(format!(
                                                                "Conflict {k}: take {}",
                                                                text.to_lowercase()
                                                            ));
                                                        btn = if on { btn.primary() } else { btn };
                                                        if btn.show(ui, pal).clicked() {
                                                            choices[k - 1] =
                                                                if on { None } else { Some(c) };
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(4.0);
                                        ui.columns(2, |cols| {
                                            let sides = [
                                                (
                                                    "Mine",
                                                    mine_label,
                                                    mine_text,
                                                    mine_on,
                                                    pal.accent_soft,
                                                ),
                                                (
                                                    "Incoming",
                                                    inc_label,
                                                    inc_text,
                                                    inc_on,
                                                    pal.warning_soft,
                                                ),
                                            ];
                                            for (ui, (title, label, text, on, tint)) in
                                                cols.iter_mut().zip(sides)
                                            {
                                                ui.label(
                                                    RichText::new(format!("{title} · {label}"))
                                                        .size(11.5)
                                                        .color(pal.text_muted),
                                                );
                                                egui::Frame::new()
                                                    .fill(if on {
                                                        tint
                                                    } else {
                                                        Color32::TRANSPARENT
                                                    })
                                                    .corner_radius(CornerRadius::same(
                                                        crate::theme::RADIUS_SM,
                                                    ))
                                                    .inner_margin(6.0)
                                                    .show(ui, |ui| {
                                                        ui.set_width(ui.available_width());
                                                        ui.spacing_mut().item_spacing.y = 2.0;
                                                        let color = if on {
                                                            pal.text
                                                        } else {
                                                            pal.text_subtle
                                                        };
                                                        code(ui, text, color);
                                                    });
                                            }
                                        });
                                    });
                                ui.add_space(6.0);
                            }
                        }
                    }
                });
        });
}
