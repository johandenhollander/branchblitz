//! Bars under the toolbar: the merge/rebase state banner and confirmation alerts.

use super::*;

// ---------------------------------------------------------------- state banner

/// Merge, rebase, cherry-pick or revert in progress: icon, bold lead word, one sentence, then
/// the way out (quiet) before the way forward (primary).
pub fn state_banner(ui: &mut Ui, tab: &RepoTab, pal: &Palette, acts: &mut Vec<Action>) {
    let Some(data) = &tab.data else { return };
    let conflicts = data.status.conflicted.len();
    let (icon, lead) = match data.state {
        RepoState::Merging => (icons::GIT_MERGE, "Merge in progress"),
        RepoState::Rebasing => (icons::GIT_COMPARE_ARROWS, "Rebase in progress"),
        RepoState::CherryPicking => (icons::CHERRY, "Cherry-pick in progress"),
        RepoState::Reverting => (icons::UNDO_2, "Revert in progress"),
        RepoState::Clean => return,
    };
    let sentence = match conflicts {
        0 => "No conflicts left.".to_string(),
        1 => "1 conflicted file — resolve it in the panel on the right.".to_string(),
        n => format!("{n} conflicted files — resolve them in the panel on the right."),
    };
    ui.set_min_height(28.0);
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(icon).size(16.0).color(pal.warning));
        ui.label(
            RichText::new(lead)
                .variation("wght", 600.0)
                .color(pal.warning),
        );
        ui.label(RichText::new(sentence).color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (out, forward, cont, abort) = match data.state {
                RepoState::Merging => (
                    "Abort merge",
                    "Commit merge",
                    Op::ContinueMerge,
                    Op::AbortMerge,
                ),
                RepoState::Rebasing => (
                    "Abort rebase",
                    "Continue rebase",
                    Op::ContinueRebase,
                    Op::AbortRebase,
                ),
                _ => ("Abort", "Commit", Op::ContinueMerge, Op::AbortCherryPick),
            };
            // Stays primary while conflicts remain, so the next step is visible.
            if ds::Button::new(forward)
                .icon(icons::GIT_COMMIT_HORIZONTAL)
                .primary()
                .enabled(conflicts == 0)
                .show(ui, pal)
                .clicked()
            {
                acts.push(Action::Op(cont));
            }
            if ds::Button::new(out).quiet().show(ui, pal).clicked() {
                acts.push(Action::Op(abort));
            }
        });
    });
}

/// The alert bar under the toolbar: the question, then Cancel and the action at the right.
/// `Some(true)` confirms, `Some(false)` cancels. With `keys`, Escape cancels and Enter
/// confirms an action that is not destructive.
pub fn alert_bar(
    ui: &mut Ui,
    alert: &crate::app::Alert,
    pal: &Palette,
    keys: bool,
) -> Option<bool> {
    let mut answer = None;
    let tone = if alert.danger { pal.danger } else { pal.accent };
    ui.set_min_height(30.0);
    // Question and buttons as one centred group. Its width is known after drawing, so the
    // last frame's width places it; the message is cut short when the bar is too narrow.
    let full = ui.available_width();
    // Per question, so another alert (or tab) never starts from this one's widths.
    let id = ui
        .scope_id()
        .with(("alert-group", &alert.title, &alert.body));
    let stored = ui.ctx().data(|d| d.get_temp::<(f32, f32)>(id));
    let (group_w, body_w) = stored.unwrap_or((full, 0.0));
    let fixed = group_w - body_w;
    ui.horizontal_centered(|ui| {
        if stored.is_none() {
            // Measured this frame, shown from the next one.
            ui.set_invisible();
        }
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.add_space(((full - group_w) / 2.0).max(0.0));
        let start = ui.cursor().min.x;
        ui.label(
            RichText::new(if alert.danger {
                icons::TRIANGLE_ALERT
            } else {
                icons::CIRCLE_ALERT
            })
            .size(16.0)
            .color(tone),
        );
        ui.label(
            RichText::new(alert.title.as_str())
                .variation("wght", 600.0)
                .color(pal.text),
        );
        let body = ui
            .scope(|ui| {
                ui.set_max_width(if stored.is_some() {
                    (full - fixed).max(80.0)
                } else {
                    full
                });
                ui.add(
                    egui::Label::new(RichText::new(alert.body.as_str()).color(pal.text_muted))
                        .truncate(),
                )
            })
            .inner;
        ui.add_space(16.0);
        if ds::Button::new("Cancel").quiet().show(ui, pal).clicked() {
            answer = Some(false);
        }
        // Destructive actions are outlined in red, never primary.
        let kind = if alert.danger {
            ds::Kind::Danger
        } else {
            ds::Kind::Primary
        };
        if ds::Button::new(alert.op.label())
            .kind(kind)
            .show(ui, pal)
            .clicked()
        {
            answer = Some(true);
        }
        let measured = (ui.min_rect().right() - start, body.rect.width());
        if (measured.0 - group_w).abs() > 0.5 || (measured.1 - body_w).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(id, measured));
            ui.ctx().request_repaint();
        }
    });
    if keys && answer.is_none() {
        // Consumed, so the same Escape does not also close the diff view.
        let (esc, enter) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if esc {
            answer = Some(false);
        } else if enter && !alert.danger {
            answer = Some(true);
        }
    }
    answer
}
