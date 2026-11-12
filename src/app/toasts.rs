//! Notification cards in the bottom-left corner: the outcome of an action (pull, push, merge …)
//! or an error that has no dialog of its own. Successes go after a few seconds (not while the
//! pointer is on them); warnings and errors stay until closed.

use super::*;

/// How long a success card stays, in seconds.
const SUCCESS_SECS: f64 = 5.0;
/// At most this many cards at once; the oldest goes first.
const MAX: usize = 4;
const WIDTH: f32 = 340.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Success,
    Warning,
    Error,
}

pub struct Toast {
    id: u64,
    pub level: Level,
    pub title: String,
    pub body: String,
    /// UI time when it goes (success only); set when first drawn.
    until: Option<f64>,
}

impl BranchBlitz {
    /// Tells the user about an outcome: a card, and the status bar.
    pub(super) fn notify(&mut self, level: Level, title: String, body: String) {
        self.status_line = Some((
            level != Level::Error,
            if body.is_empty() {
                title.clone()
            } else {
                format!("{title}: {body}")
            },
        ));
        // The same message again (e.g. a retried push) replaces the old card.
        self.toasts
            .retain(|t| !(t.level == level && t.title == title && t.body == body));
        self.next_id += 1;
        self.toasts.push(Toast {
            id: self.next_id,
            level,
            title,
            body,
            until: None,
        });
        if self.toasts.len() > MAX {
            self.toasts.remove(0);
        }
        // It may come from outside the frame loop (a dialog, a background result).
        self.ctx.request_repaint();
    }

    pub(super) fn show_toasts(&mut self, ctx: &egui::Context, pal: &Palette) {
        if self.toasts.is_empty() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let mut close = None;
        egui::Area::new(egui::Id::unique("bb-toasts"))
            .order(egui::Order::Foreground)
            .anchor(
                egui::Align2::LEFT_BOTTOM,
                egui::vec2(12.0, -(STATUS_H + 12.0)),
            )
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing.y = 8.0;
                for t in &mut self.toasts {
                    let (icon, color) = match t.level {
                        Level::Success => (icons::CIRCLE_CHECK, pal.success),
                        Level::Warning => (icons::TRIANGLE_ALERT, pal.warning),
                        Level::Error => (icons::CIRCLE_ALERT, pal.danger),
                    };
                    let resp = egui::Frame::new()
                        .fill(pal.raised)
                        .stroke(egui::Stroke::new(
                            1.0,
                            if t.level == Level::Error {
                                pal.danger
                            } else {
                                pal.border_strong
                            },
                        ))
                        .corner_radius(egui::CornerRadius::same(theme::RADIUS_LG))
                        .shadow(ui.style().visuals.popup_shadow)
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 6,
                            top: 10,
                            bottom: 10,
                        })
                        .show(ui, |ui| {
                            ui.set_width(WIDTH - 18.0);
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(RichText::new(icon).size(16.0).color(color));
                                ui.vertical(|ui| {
                                    ui.set_width(WIDTH - 18.0 - 24.0 - 32.0);
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(ds::strong(&t.title).size(13.0).color(pal.text));
                                    if !t.body.is_empty() {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&t.body)
                                                    .size(12.0)
                                                    .color(pal.text_muted),
                                            )
                                            .wrap(),
                                        );
                                    }
                                });
                                let label = format!("Close notification: {}", t.title);
                                if ds::Button::icon_only(icons::X, label)
                                    .quiet()
                                    .small()
                                    .show(ui, pal)
                                    .clicked()
                                {
                                    close = Some(t.id);
                                }
                            });
                        })
                        .response;
                    let text = if t.body.is_empty() {
                        t.title.clone()
                    } else {
                        format!("{}: {}", t.title, t.body)
                    };
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Alert, true, &text));
                    if t.level == Level::Success {
                        let until = *t.until.get_or_insert(now + SUCCESS_SECS);
                        // The pointer on a card keeps it.
                        let hover = ui.interact(
                            resp.rect,
                            ui.scope_id().with(("bb-toast", t.id)),
                            egui::Sense::hover(),
                        );
                        if hover.hovered() {
                            t.until = Some(until.max(now + 1.5));
                        }
                    }
                }
            });
        self.toasts
            .retain(|t| Some(t.id) != close && t.until.is_none_or(|until| now < until));
        if let Some(next) = self
            .toasts
            .iter()
            .filter_map(|t| t.until)
            .min_by(f64::total_cmp)
        {
            ctx.request_repaint_after(Duration::from_secs_f64((next - now).max(0.05)));
        }
    }
}
