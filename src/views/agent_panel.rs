//! The agent panel: conversation, input, model and access pickers.

use super::*;

// ---------------------------------------------------------------- agent panel

/// Who answers in the agent panel and how: backend name, chosen model, access.
#[derive(Clone, Copy)]
pub struct AgentChoice<'a> {
    pub name: &'a str,
    pub model: Option<&'a str>,
    pub access: crate::app::AgentAccess,
    /// Show the tool calls in the conversation.
    pub show_tools: bool,
}

/// Height of the closed agent panel: a bar at the bottom of the right panel.
pub const AGENT_BAR_H: f32 = 34.0;

/// The closed agent panel; click to open it. A dot while an answer is still coming in.
pub fn agent_bar(ui: &mut Ui, pal: &Palette, acts: &mut Vec<Action>, busy: bool) {
    let rect = ui.available_rect_before_wrap();
    let (r, resp) = ui.allocate_exact_size(vec2(rect.width(), AGENT_BAR_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Open agent"));
    let p = ui.painter();
    p.rect_filled(r, 0.0, if resp.hovered() { pal.hover } else { pal.app });
    p.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, pal.border));
    let yc = r.center().y;
    ds::paint_icon(
        p,
        pos2(r.left() + 20.0, yc),
        icons::BOT,
        15.0,
        pal.text_muted,
    );
    let g = p.layout_job(ds::text_job("Agent", 12.5, 550.0, pal.text, false));
    let gw = g.size().x;
    p.galley(pos2(r.left() + 34.0, yc - g.size().y / 2.0), g, pal.text);
    if busy {
        p.circle_filled(pos2(r.left() + 42.0 + gw, yc), 3.5, pal.accent);
    }
    ds::paint_icon(
        p,
        pos2(r.right() - 20.0, yc),
        icons::CHEVRON_UP,
        14.0,
        pal.text_subtle,
    );
    if resp.clicked() {
        acts.push(Action::AgentOpen(true));
    }
}

/// The border between the details and the agent; drag to resize. Returns the vertical drag.
pub fn agent_splitter(ui: &mut Ui, pal: &Palette) -> Option<f32> {
    let rect = ui.available_rect_before_wrap();
    let (r, resp) = ui.allocate_exact_size(vec2(rect.width(), 6.0), Sense::drag());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Splitter, true, "Resize agent panel")
    });
    let active = resp.hovered() || resp.dragged();
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    ui.painter().hline(
        r.x_range(),
        r.top() + 0.5,
        Stroke::new(1.0, if active { pal.accent } else { pal.border }),
    );
    resp.dragged().then(|| resp.drag_delta().y)
}

/// The conversation with the agent: header (show large or back, close), the messages, and
/// the input. Enter sends, Shift+Enter starts a new line.
pub fn agent_panel(
    ui: &mut Ui,
    conv: &mut crate::agent::Conversation,
    agent: AgentChoice,
    pal: &Palette,
    acts: &mut Vec<Action>,
    large: bool,
) {
    use crate::agent::Role;
    let rect = ui.available_rect_before_wrap();
    let header_h = 36.0;
    let input_h = if large { 120.0 } else { 96.0 };
    let (header, rest) = rect.split_top_bottom_at_y(rect.top() + header_h);
    let (body, input) = rest.split_top_bottom_at_y(rest.bottom() - input_h);
    ui.painter().rect_filled(rect, 0.0, pal.app);

    // Header.
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(header.shrink2(vec2(12.0, 0.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(RichText::new(icons::BOT).size(15.0).color(pal.text_muted));
                ui.label(
                    RichText::new("Agent")
                        .size(13.0)
                        .variation("wght", 600.0)
                        .color(pal.text),
                );
                // The backend's name only when there is room for it beside the controls.
                if ui.available_width() > 360.0 {
                    ui.label(RichText::new(agent.name).size(11.5).color(pal.text_subtle));
                }
                if agent.name != "Preview" {
                    model_picker(ui, conv.model.as_deref(), agent.model, pal, acts);
                    access_picker(ui, agent.access, pal, acts);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let (icon, label, close) = if large {
                        (icons::X, "Close agent", Action::AgentMaximize(false))
                    } else {
                        (icons::CHEVRON_DOWN, "Close agent", Action::AgentOpen(false))
                    };
                    if ds::Button::icon_only(icon, label)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(close);
                    }
                    let (icon, label) = if large {
                        (icons::MINIMIZE_2, "Show in the panel")
                    } else {
                        (icons::MAXIMIZE_2, "Show large")
                    };
                    if ds::Button::icon_only(icon, label)
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        acts.push(Action::AgentMaximize(!large));
                    }
                    if !conv.messages.is_empty()
                        && ds::Button::icon_only(icons::ROTATE_CCW, "New conversation")
                            .quiet()
                            .small()
                            .enabled(!conv.busy())
                            .show(ui, pal)
                            .clicked()
                    {
                        acts.push(Action::AgentClear);
                    }
                });
            });
        },
    );
    ui.painter().hline(
        rect.x_range(),
        header.bottom() - 0.5,
        Stroke::new(1.0, pal.border),
    );

    // Messages, following the end while an answer streams in.
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .id_salt("agent-messages")
            .show(ui, |ui| {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 8.0;
                        if conv.messages.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(
                                        "Ask about this repository, or tell the agent what to do. \
                                         It works in this repository and you see what it does.",
                                    )
                                    .size(12.5)
                                    .color(pal.text_muted),
                                )
                                .wrap(),
                            );
                        }
                        for m in conv
                            .messages
                            .iter()
                            .filter(|m| agent.show_tools || m.role != Role::Tool)
                        {
                            match m.role {
                                Role::User => {
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Max),
                                        |ui| {
                                            egui::Frame::new()
                                                .fill(pal.accent_soft)
                                                .corner_radius(CornerRadius::same(
                                                    crate::theme::RADIUS_MD,
                                                ))
                                                .inner_margin(egui::Margin::symmetric(10, 6))
                                                .show(ui, |ui| {
                                                    ui.set_max_width(ui.available_width() * 0.85);
                                                    ui.add(
                                                        egui::Label::new(
                                                            RichText::new(&m.text).color(pal.text),
                                                        )
                                                        .wrap()
                                                        .selectable(true),
                                                    );
                                                });
                                        },
                                    );
                                }
                                Role::Agent => markdown(ui, &m.text, pal),
                                Role::Tool => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&m.text)
                                                .monospace()
                                                .size(11.5)
                                                .color(pal.text_subtle),
                                        )
                                        .wrap(),
                                    );
                                }
                                Role::Error => {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&m.text).size(12.5).color(pal.danger),
                                        )
                                        .wrap(),
                                    );
                                }
                            }
                        }
                        // As long as the agent works: what it does now, and for how long.
                        if let Some((what, took)) = conv.status() {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(13.0));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{what} · {} s", took.as_secs()))
                                            .color(pal.text_muted),
                                    )
                                    .truncate(),
                                );
                            });
                        }
                    });
            });
    });

    // Input: the field with Send (or Stop) beside it.
    ui.painter().hline(
        rect.x_range(),
        input.top() + 0.5,
        Stroke::new(1.0, pal.border),
    );
    let busy = conv.busy();
    ui.scope_builder(egui::UiBuilder::new().max_rect(input.shrink(10.0)), |ui| {
        ui.horizontal_top(|ui| {
            let button_w = 32.0;
            let field_w = ui.available_width() - button_w - 8.0;
            let field = ui.add_sized(
                vec2(field_w, ui.available_height()),
                egui::TextEdit::multiline(&mut conv.input)
                    .hint_text(
                        RichText::new("Ask or instruct the agent… (Enter sends)")
                            .color(pal.text_subtle),
                    )
                    .return_key(Some(egui::KeyboardShortcut::new(
                        egui::Modifiers::SHIFT,
                        egui::Key::Enter,
                    )))
                    .frame(egui::Frame::new()),
            );
            // Named, so the MCP (and a screen reader) can find the field.
            field.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::MultilineTextInput,
                    true,
                    "Instruction for the agent",
                )
            });
            let enter = field.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if busy {
                if ds::Button::icon_only(icons::SQUARE, "Stop the agent")
                    .small()
                    .show(ui, pal)
                    .clicked()
                {
                    acts.push(Action::AgentStop);
                }
            } else {
                let empty = conv.input.trim().is_empty();
                if ds::Button::icon_only(icons::SEND_HORIZONTAL, "Send (Enter)")
                    .primary()
                    .small()
                    .enabled(!empty)
                    .show(ui, pal)
                    .clicked()
                    || (enter && !empty)
                {
                    acts.push(Action::AgentSend);
                }
            }
        });
    });
}

/// Agent answers: paragraphs, `#` headings, `-`/`*`/numbered lists, fenced code blocks, and
/// inline `code` and **bold**. Anything else shows as written.
fn markdown(ui: &mut Ui, text: &str, pal: &Palette) {
    let size = ui.style().text_styles[&egui::TextStyle::Body].size;
    let mono = ui.style().text_styles[&egui::TextStyle::Monospace].size;
    let mut lines = text.lines().peekable();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        while let Some(line) = lines.next() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") {
                let mut code = Vec::new();
                for l in lines.by_ref() {
                    if l.trim_start().starts_with("```") {
                        break;
                    }
                    code.push(l);
                }
                egui::Frame::new()
                    .fill(pal.sunken)
                    .corner_radius(CornerRadius::same(crate::theme::RADIUS_SM))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(code.join("\n"))
                                    .monospace()
                                    .size(mono)
                                    .color(pal.text),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    });
                continue;
            }
            if trimmed.is_empty() {
                ui.add_space(2.0);
                continue;
            }
            let hashes = trimmed.chars().take_while(|&c| c == '#').count();
            let (body, weight, bullet) = if hashes > 0 && trimmed[hashes..].starts_with(' ') {
                (&trimmed[hashes + 1..], 650.0, None)
            } else if let Some(rest) = trimmed.strip_prefix("- ").or(trimmed.strip_prefix("* ")) {
                (rest, 400.0, Some("•".to_string()))
            } else if let Some((n, rest)) = trimmed.split_once(". ")
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit())
            {
                (rest, 400.0, Some(format!("{n}.")))
            } else {
                (trimmed, 400.0, None)
            };
            let mut job = LayoutJob::default();
            if let Some(b) = &bullet {
                let indent = (line.len() - trimmed.len()) as f32 * 4.0;
                let f = ds::text_job(b, size, 400.0, pal.text_muted, false).sections[0]
                    .format
                    .clone();
                job.append(&format!("{b} "), indent, f);
            }
            inline_markdown(&mut job, body, size, mono, weight, pal);
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap().selectable(true));
        }
    });
}

/// `code` in the mono font on a tint, **bold** heavier, the rest as is.
fn inline_markdown(
    job: &mut LayoutJob,
    text: &str,
    size: f32,
    mono: f32,
    weight: f32,
    pal: &Palette,
) {
    let fmt = |w: f32| {
        ds::text_job("x", size, w, pal.text, false).sections[0]
            .format
            .clone()
    };
    for (i, part) in text.split('`').enumerate() {
        if i % 2 == 1 {
            let mut f = ds::text_job("x", mono, 400.0, pal.text, true).sections[0]
                .format
                .clone();
            f.background = pal.sunken;
            job.append(part, 0.0, f);
            continue;
        }
        for (j, piece) in part.split("**").enumerate() {
            let w = if j % 2 == 1 { 650.0 } else { weight };
            job.append(piece, 0.0, fmt(w));
        }
    }
}

/// The model in the agent panel's header; click to choose another for the next questions.
/// Shows the model that answered last when it is the chosen one, else the choice.
fn model_picker(
    ui: &mut Ui,
    answered: Option<&str>,
    chosen: Option<&str>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::agent::{CLAUDE_MODELS, model_name};
    let shown = answered
        .filter(|m| chosen.is_none_or(|c| m.contains(c)))
        .map(model_name)
        .or_else(|| {
            chosen.map(|c| {
                CLAUDE_MODELS
                    .iter()
                    .find(|(a, _)| *a == c)
                    .map_or(c.to_string(), |(_, n)| n.to_string())
            })
        })
        .unwrap_or_else(|| "Default model".into());
    let resp = ds::Button::new(&shown)
        .icon(icons::CHEVRON_DOWN)
        .quiet()
        .small()
        .label(format!("Model: {shown}"))
        .tooltip("The model for the next questions")
        .show(ui, pal);
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(240.0);
            if ds::menu_choice(ui, pal, chosen.is_none(), "Claude Code's default").clicked() {
                acts.push(Action::AgentModel(None));
                ui.close();
            }
            for (alias, name) in CLAUDE_MODELS {
                if ds::menu_choice(ui, pal, chosen == Some(*alias), name).clicked() {
                    acts.push(Action::AgentModel(Some(alias.to_string())));
                    ui.close();
                }
            }
        });
}

/// How much the agent may do without asking; Full access shows as a red open lock.
fn access_picker(
    ui: &mut Ui,
    access: crate::app::AgentAccess,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    use crate::app::AgentAccess;
    let modes = [
        (
            AgentAccess::Supervised,
            icons::LOCK,
            "Supervised",
            "Asks before pushing, rewriting or throwing away work.",
        ),
        (
            AgentAccess::Auto,
            icons::SPARKLES,
            "Auto",
            "Pushes, amends and resets on its own; asks before throwing work away.",
        ),
        (
            AgentAccess::Full,
            icons::LOCK_OPEN,
            "Full access",
            "Never asks, also not for force push, hard reset or discarding changes.",
        ),
    ];
    let (_, icon, name, _) = modes
        .iter()
        .find(|m| m.0 == access)
        .copied()
        .unwrap_or(modes[0]);
    let mut button = ds::Button::icon_only(icon, format!("Access: {name}"))
        .quiet()
        .small()
        .tooltip(format!(
            "Access: {name}. What the agent may do without asking you"
        ));
    if access == AgentAccess::Full {
        button = button.kind(ds::Kind::Danger);
    }
    let resp = button.show(ui, pal);
    egui::Popup::menu(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(300.0);
            for (mode, icon, name, about) in modes {
                let chosen = mode == access;
                let r = ds::menu_item(
                    ui,
                    pal,
                    icon,
                    name,
                    Some(&format!("{name}: {about}")),
                    false,
                );
                if chosen {
                    ui.painter().text(
                        pos2(r.rect.right() - 14.0, r.rect.center().y),
                        egui::Align2::CENTER_CENTER,
                        icons::CHECK,
                        FontId::proportional(14.0),
                        pal.accent,
                    );
                }
                // The explanation, under the name.
                ui.horizontal(|ui| {
                    ui.add_space(36.0);
                    ui.add(
                        egui::Label::new(RichText::new(about).size(11.5).color(pal.text_subtle))
                            .wrap(),
                    );
                });
                ui.add_space(4.0);
                if r.clicked() {
                    acts.push(Action::AgentAccess(mode));
                    ui.close();
                }
            }
        });
}
