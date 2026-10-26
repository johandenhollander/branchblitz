//! Widgets of the BranchBlitz design system: buttons, chips, badges, rows, menu items.
//! All are painted by hand from `Palette` tokens and carry an accessible label, so the MCP
//! server and kittest find them like egui's own widgets.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Galley, Painter, Pos2, Rect, Response,
    RichText, Role, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo,
    epaint::{TextShape, text::VariationCoords},
    pos2,
    text::{LayoutJob, TextFormat},
    vec2,
};

use crate::icons;
use crate::theme::{Palette, RADIUS_MD, RADIUS_SM, RADIUS_XS};

/// `control-md`: toolbar buttons and inputs.
pub const CONTROL_MD: f32 = 30.0;
/// `control-sm`: buttons inside panels.
pub const CONTROL_SM: f32 = 24.0;
/// `row-height`: commit rows and sidebar rows.
pub const ROW_H: f32 = 28.0;

// ---------------------------------------------------------------- text

/// One run of text in the given size, weight (100–900) and colour.
pub fn text_job(text: &str, size: f32, weight: f32, color: Color32, mono: bool) -> LayoutJob {
    let family = if mono {
        FontFamily::Monospace
    } else {
        FontFamily::Proportional
    };
    LayoutJob::single_section(
        text.to_string(),
        TextFormat {
            font_id: FontId::new(size, family),
            color,
            coords: VariationCoords::new([(b"wght", weight)]),
            ..Default::default()
        },
    )
}

/// `text` laid out on one line, cut with `…` at `max_width`.
pub fn galley(
    painter: &Painter,
    text: &str,
    size: f32,
    weight: f32,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = text_job(text, size, weight, color, false);
    job.wrap.max_width = max_width.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    painter.layout_job(job)
}

/// `body-strong`: current branch, button labels, banner lead.
pub fn strong(text: impl Into<String>) -> RichText {
    RichText::new(text).variation("wght", 550.0)
}

/// `caption`: column and section headers, uppercase in `text-subtle`.
pub fn caption(text: &str, pal: &Palette) -> RichText {
    RichText::new(text.to_uppercase())
        .size(11.0)
        .variation("wght", 600.0)
        .extra_letter_spacing(0.66)
        .color(pal.text_subtle)
}

/// `title`: commit subject in the detail panel, dialog titles.
pub fn title(text: impl Into<String>) -> RichText {
    RichText::new(text)
        .size(17.0)
        .variation("wght", 600.0)
        .extra_letter_spacing(-0.17)
}

/// `heading`: section headings in settings and panels.
pub fn heading(text: impl Into<String>) -> RichText {
    RichText::new(text).size(14.0).variation("wght", 600.0)
}

/// An icon glyph centred on `center`.
pub fn paint_icon(painter: &Painter, center: Pos2, icon: &str, size: f32, color: Color32) {
    painter.text(
        center,
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(size),
        color,
    );
}

/// A closed ring with a brighter quarter arc at `angle`: the busy state of a button.
pub fn paint_spinner(p: &Painter, c: Pos2, r: f32, color: Color32, angle: f32) {
    p.circle_stroke(c, r, Stroke::new(1.75, color.gamma_multiply(0.25)));
    let arc: Vec<Pos2> = (0..=12)
        .map(|k| {
            let a = angle + k as f32 / 12.0 * std::f32::consts::FRAC_PI_2;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(arc, Stroke::new(1.75, color)));
}

/// An icon turned by `angle` radians around its centre (busy spinners).
fn paint_icon_turned(
    painter: &Painter,
    center: Pos2,
    icon: &str,
    size: f32,
    color: Color32,
    angle: f32,
) {
    let g = painter.layout_no_wrap(icon.to_string(), FontId::proportional(size), color);
    let pos = center - g.rect.center().to_vec2();
    painter.add(TextShape::new(pos, g, color).with_angle_and_anchor(angle, Align2::CENTER_CENTER));
}

/// The 2 px focus ring with 2 px offset, on everything focusable.
pub fn focus_ring(ui: &Ui, resp: &Response, pal: &Palette, radius: u8) {
    if resp.has_focus() && !ui.ctx().input(|i| i.pointer.any_down()) {
        ui.painter().rect_stroke(
            resp.rect.expand(2.0),
            CornerRadius::same(radius + 2),
            Stroke::new(2.0, pal.focus),
            StrokeKind::Outside,
        );
    }
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// `bg-control` fill: Pull, Push and other sync actions.
    #[default]
    Default,
    /// Transparent until hovered: Fetch, Branch, Stash, Pop and panel actions.
    Quiet,
    /// The one step the user is in the middle of: Commit, Commit merge.
    Primary,
    /// Outlined in `danger`: Abort merge, Discard.
    Danger,
}

/// A design system button (`bb-btn`). Build it, then `show` it.
#[must_use]
pub struct Button {
    icon: Option<&'static str>,
    text: String,
    kind: Kind,
    small: bool,
    min_width: f32,
    width: Option<f32>,
    height: Option<f32>,
    busy: bool,
    animate: bool,
    /// Busy turns the icon itself (Fetch's arrows) instead of showing a spinner.
    spin_icon: bool,
    enabled: bool,
    count: Option<String>,
    label: Option<String>,
    tooltip: Option<String>,
}

impl Button {
    pub fn new(text: impl Into<String>) -> Self {
        Button {
            icon: None,
            text: text.into(),
            kind: Kind::Default,
            small: false,
            min_width: 0.0,
            width: None,
            height: None,
            busy: false,
            spin_icon: false,
            animate: true,
            enabled: true,
            count: None,
            label: None,
            tooltip: None,
        }
    }

    /// An icon-only button; `label` names it for screen readers, the MCP and the tooltip.
    pub fn icon_only(icon: &'static str, label: impl Into<String>) -> Self {
        let label = label.into();
        Button::new("")
            .icon(icon)
            .label(label.clone())
            .tooltip(label)
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub fn quiet(self) -> Self {
        self.kind(Kind::Quiet)
    }

    pub fn primary(self) -> Self {
        self.kind(Kind::Primary)
    }

    /// `bb-btn--sm`: 24 px high, inside panels.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Keeps the button from changing width when its label or count changes.
    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    /// A fixed size, e.g. a full-width commit button.
    pub fn size(mut self, size: Vec2) -> Self {
        self.width = Some(size.x);
        self.height = Some(size.y);
        self
    }

    /// A small ring spinner in place of the icon, and a muted label, while the operation it
    /// started runs.
    pub fn busy(mut self, busy: bool, animate: bool) -> Self {
        self.busy = busy;
        self.animate = animate;
        self
    }

    /// While busy, turn the icon itself (for icons that already are a circle of arrows).
    pub fn spin_icon(mut self) -> Self {
        self.spin_icon = true;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A `bb-count` pill after the label (ahead/behind); hidden when `None`.
    pub fn count(mut self, count: Option<String>) -> Self {
        self.count = count;
        self
    }

    /// Accessible name when it differs from the visible text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    pub fn show(self, ui: &mut Ui, pal: &Palette) -> Response {
        let h = self
            .height
            .unwrap_or(if self.small { CONTROL_SM } else { CONTROL_MD });
        let (font, icon_size, pad) = if self.small {
            (12.0, 14.0, 8.0)
        } else {
            (13.0, 16.0, 12.0)
        };
        let fg = match self.kind {
            Kind::Primary => pal.on_accent,
            Kind::Danger => pal.danger,
            _ if self.busy => pal.text_muted,
            _ => pal.text,
        };
        let text_g = (!self.text.is_empty()).then(|| {
            ui.painter()
                .layout_job(text_job(&self.text, font, 550.0, fg, false))
        });
        let count_g = self.count.as_ref().map(|c| {
            ui.painter()
                .layout_job(text_job(c, 11.0, 600.0, pal.accent, true))
        });
        let gap = 6.0;
        let mut content = 0.0;
        if self.icon.is_some() {
            content += icon_size;
        }
        if let Some(g) = &text_g {
            content += g.size().x + if self.icon.is_some() { gap } else { 0.0 };
        }
        if let Some(g) = &count_g {
            content += g.size().x + 12.0 + 2.0 + gap;
        }
        let icon_only = text_g.is_none() && count_g.is_none();
        let w = self.width.unwrap_or(if icon_only {
            h
        } else {
            (content + 2.0 * pad).max(self.min_width)
        });
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        let enabled = self.enabled && ui.is_enabled();
        let resp = if enabled {
            resp
        } else {
            // Not clickable, but still listed and described.
            ui.interact(rect, resp.id, Sense::hover())
        };
        let a11y = self.label.clone().unwrap_or_else(|| self.text.clone());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, &a11y));
        if ui.is_rect_visible(rect) {
            let hovered = enabled && resp.hovered();
            let fill = match (self.kind, hovered) {
                (Kind::Default, false) => pal.control,
                (Kind::Default, true) | (Kind::Quiet, true) => pal.hover,
                (Kind::Quiet, false) | (Kind::Danger, false) => Color32::TRANSPARENT,
                (Kind::Primary, false) => pal.accent,
                (Kind::Primary, true) => pal.accent_hover,
                (Kind::Danger, true) => pal.danger_soft,
            };
            let alpha = if enabled { 1.0 } else { 0.45 };
            let p = ui.painter();
            let r = CornerRadius::same(RADIUS_MD);
            p.rect_filled(rect, r, fill.gamma_multiply(alpha));
            if self.kind == Kind::Danger {
                p.rect_stroke(
                    rect,
                    r,
                    Stroke::new(1.0, pal.danger.gamma_multiply(alpha)),
                    StrokeKind::Inside,
                );
            }
            let mut x = if icon_only {
                rect.center().x - icon_size / 2.0
            } else {
                rect.center().x - content / 2.0
            };
            let yc = rect.center().y;
            let fg = fg.gamma_multiply(alpha);
            if let Some(icon) = self.icon {
                let c = pos2(x + icon_size / 2.0, yc);
                let t = ui.input(|i| i.time) as f32;
                if self.busy && self.spin_icon && self.animate {
                    paint_icon_turned(p, c, icon, icon_size, fg, t * std::f32::consts::TAU / 0.9);
                    ui.ctx().request_repaint();
                } else if self.busy && !self.spin_icon {
                    let turn = if self.animate {
                        t * std::f32::consts::TAU / 0.8
                    } else {
                        0.0
                    };
                    paint_spinner(p, c, icon_size * 0.4, fg, turn);
                    if self.animate {
                        ui.ctx().request_repaint();
                    }
                } else {
                    paint_icon(p, c, icon, icon_size, fg);
                }
                x += icon_size + gap;
            }
            if let Some(g) = text_g {
                p.galley(pos2(x, yc - g.size().y / 2.0), g.clone(), fg);
                x += g.size().x + gap;
            }
            if let Some(g) = count_g {
                let pill =
                    Rect::from_min_size(pos2(x + 2.0, yc - 9.0), vec2(g.size().x + 12.0, 18.0));
                let bg = if self.kind == Kind::Primary {
                    pal.on_accent
                } else {
                    pal.accent_soft
                };
                p.rect_filled(pill, CornerRadius::same(9), bg.gamma_multiply(alpha));
                p.galley(
                    pill.center() - g.size() / 2.0,
                    g,
                    pal.accent.gamma_multiply(alpha),
                );
            }
        }
        focus_ring(ui, &resp, pal, RADIUS_MD);
        let resp = if self.busy {
            resp.on_hover_cursor(egui::CursorIcon::Progress)
        } else {
            resp
        };
        match self.tooltip {
            Some(t) => resp.on_hover_text(t),
            None => resp,
        }
    }
}

/// `bb-split`: the main button runs its action at once; the slim caret beside it opens the
/// options. Returns the main button's response; the menu shows while the caret's popup is open.
pub fn split(
    ui: &mut Ui,
    pal: &Palette,
    main: Button,
    options_label: &str,
    menu: impl FnOnce(&mut Ui),
) -> Response {
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 2.0;
    let main = main.show(ui, pal);
    ui.spacing_mut().item_spacing.x = spacing;
    let caret = caret(ui, pal, options_label);
    // Items close the menu themselves (`ui.close()`), so a radio can change without closing it.
    egui::Popup::menu(&caret)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_START)
        .gap(4.0)
        .show(|ui| {
            ui.set_min_width(248.0);
            menu(ui);
        });
    main
}

/// The 20 px caret of a split button.
fn caret(ui: &mut Ui, pal: &Palette, label: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(20.0, CONTROL_MD), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), label));
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    let hot = resp.hovered() || open;
    if hot {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(RADIUS_SM), pal.hover);
    }
    paint_icon(
        ui.painter(),
        rect.center(),
        icons::CHEVRON_DOWN,
        14.0,
        if hot { pal.text } else { pal.text_subtle },
    );
    focus_ring(ui, &resp, pal, RADIUS_SM);
    resp.on_hover_text(label)
}

// ---------------------------------------------------------------- small pieces

/// `bb-st`: a file's status letter in a 16 px tinted square.
pub fn status_letter(painter: &Painter, center: Pos2, status: char, pal: &Palette) {
    let c = pal.status_color(status);
    let r = Rect::from_center_size(center, vec2(16.0, 16.0));
    painter.rect_filled(r, CornerRadius::same(RADIUS_XS), c.gamma_multiply(0.14));
    let g = painter.layout_job(text_job(&status.to_string(), 10.0, 700.0, c, true));
    painter.galley(r.center() - g.size() / 2.0, g, c);
}

/// `bb-avatar`: lane-coloured initials.
/// Radius of an author avatar.
pub const AVATAR_R: f32 = 9.0;

pub fn avatar(painter: &Painter, center: Pos2, initials: &str, lane: Color32) {
    painter.circle_filled(center, AVATAR_R, lane);
    let fg = crate::theme::readable_on(lane);
    let g = painter.layout_job(text_job(initials, 8.5, 700.0, fg, false));
    painter.galley(center - g.size() / 2.0, g, fg);
}

/// What a ref chip stands for.
#[derive(Clone, Copy, Default)]
pub struct ChipKind {
    pub head: bool,
    pub local: bool,
    pub remote: bool,
    pub tag: bool,
    pub default: bool,
    /// A tag that is not on any remote yet: drawn with a dashed border.
    pub unpushed: bool,
    /// A local branch checked out in another worktree: a tree instead of the branch icon.
    pub worktree: bool,
}

impl ChipKind {
    fn icons(self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.head {
            v.push(icons::CHECK);
        } else if self.worktree {
            v.push(icons::TREE_PINE);
        } else if self.local {
            v.push(icons::GIT_BRANCH);
        }
        if self.remote {
            v.push(icons::CLOUD);
        }
        if self.tag {
            v.push(icons::TAG);
        }
        if self.default {
            v.push(icons::STAR);
        }
        v
    }
}

const CHIP_H: f32 = 20.0;
const CHIP_ICON: f32 = 12.0;

/// Like [`chip_layout`], with a muted `prefix` (the remote) before the name.
pub fn chip_layout_prefixed(
    painter: &Painter,
    prefix: Option<&str>,
    name: &str,
    kind: ChipKind,
    pal: &Palette,
    max_width: f32,
) -> (Arc<Galley>, f32) {
    let Some(prefix) = prefix else {
        return chip_layout(painter, name, kind, pal, max_width);
    };
    let icons_w = kind.icons().len() as f32 * (CHIP_ICON + 4.0);
    let (fg, muted) = if kind.head {
        (pal.canvas, pal.canvas.gamma_multiply(0.75))
    } else {
        (pal.text, pal.text_muted)
    };
    let weight = if kind.head { 600.0 } else { 500.0 };
    let mut job = text_job(&format!("{prefix} "), 12.0, 450.0, muted, false);
    let name_job = text_job(name, 12.0, weight, fg, false);
    job.append(name, 0.0, name_job.sections[0].format.clone());
    job.wrap.max_width = (max_width - icons_w - 11.0).max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = painter.layout_job(job);
    let w = 5.0 + icons_w + g.size().x + 6.0;
    (g, w)
}

/// Width of a ref chip for `name`, cut to fit `max_width`.
pub fn chip_layout(
    painter: &Painter,
    name: &str,
    kind: ChipKind,
    pal: &Palette,
    max_width: f32,
) -> (Arc<Galley>, f32) {
    let icons_w = kind.icons().len() as f32 * (CHIP_ICON + 4.0);
    let fg = if kind.head { pal.canvas } else { pal.text };
    let weight = if kind.head { 600.0 } else { 500.0 };
    let g = galley(painter, name, 12.0, weight, fg, max_width - icons_w - 11.0);
    let w = 5.0 + icons_w + g.size().x + 6.0;
    (g, w)
}

/// `bb-ref`: a branch, remote or tag label tinted with its lane; the text stays `text`.
pub fn paint_chip(
    painter: &Painter,
    left_center: Pos2,
    g: Arc<Galley>,
    width: f32,
    kind: ChipKind,
    lane: Color32,
    pal: &Palette,
) -> Rect {
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - CHIP_H / 2.0),
        vec2(width, CHIP_H),
    );
    let r = CornerRadius::same(RADIUS_SM);
    let (fill, border, icon_c, fg) = if kind.head {
        (lane, lane, pal.canvas, pal.canvas)
    } else {
        (
            crate::theme::mix(pal.canvas, lane, 0.12),
            crate::theme::mix(pal.canvas, lane, 0.45),
            lane,
            pal.text,
        )
    };
    painter.rect_filled(rect, r, fill);
    if kind.unpushed {
        dashed_rect(painter, rect, border);
    } else {
        painter.rect_stroke(rect, r, Stroke::new(1.0, border), StrokeKind::Inside);
    }
    let mut x = rect.left() + 5.0;
    for icon in kind.icons() {
        paint_icon(
            painter,
            pos2(x + CHIP_ICON / 2.0, left_center.y),
            icon,
            CHIP_ICON,
            icon_c,
        );
        x += CHIP_ICON + 4.0;
    }
    painter.galley(pos2(x, left_center.y - g.size().y / 2.0), g, fg);
    rect
}

fn dashed_rect(painter: &Painter, r: Rect, color: Color32) {
    let r = r.shrink(0.5);
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &pts,
        Stroke::new(1.0, color),
        3.0,
        2.0,
    ));
}

/// `bb-ref-more`: "+N" after the chips that fit.
pub fn paint_more(painter: &Painter, left_center: Pos2, n: usize, pal: &Palette) -> Rect {
    let g = painter.layout_job(text_job(
        &format!("+{n}"),
        11.0,
        600.0,
        pal.text_muted,
        true,
    ));
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - 9.0),
        vec2(g.size().x + 10.0, 18.0),
    );
    painter.rect_filled(rect, CornerRadius::same(RADIUS_SM), pal.control);
    painter.galley(rect.center() - g.size() / 2.0, g, pal.text_muted);
    rect
}

// ---------------------------------------------------------------- rows

/// A full-width clickable row of `row-height` with hover and selected fills; returns the
/// response and the rect to paint into. `label` is its accessible name.
pub fn row(ui: &mut Ui, pal: &Palette, height: f32, selected: bool, label: &str) -> Response {
    row_sense(ui, pal, height, selected, label, Sense::click())
}

/// `row`, sensing `sense` (e.g. drags as well).
pub fn row_sense(
    ui: &mut Ui,
    pal: &Palette,
    height: f32,
    selected: bool,
    label: &str,
    sense: Sense,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), sense);
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, label));
    let fill = if selected {
        pal.selected
    } else if resp.hovered() {
        pal.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS_SM), fill);
    resp
}

/// `bb-sec`: a collapsible section header (chevron, caption, count badge). Returns whether
/// it is open. `force_open` overrides the remembered state (e.g. while filtering).
pub fn section_header(
    ui: &mut Ui,
    pal: &Palette,
    id: egui::Id,
    title: &str,
    count: Option<usize>,
    default_open: bool,
    force_open: bool,
) -> bool {
    let mut open = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(id))
        .unwrap_or(default_open);
    let resp = row(ui, pal, ROW_H, false, title);
    let rect = resp.rect;
    if resp.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    let open = open || force_open;
    let p = ui.painter();
    let yc = rect.center().y;
    paint_icon(
        p,
        pos2(rect.left() + 6.0 + 7.0, yc),
        if open {
            icons::CHEVRON_DOWN
        } else {
            icons::CHEVRON_RIGHT
        },
        14.0,
        pal.text_subtle,
    );
    let cap = caption(title, pal);
    let g = egui::WidgetText::from(cap).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width(),
        egui::TextStyle::Body,
    );
    ui.painter().galley(
        pos2(rect.left() + 26.0, yc - g.size().y / 2.0),
        g,
        pal.text_subtle,
    );
    if let Some(n) = count {
        let g =
            ui.painter()
                .layout_job(text_job(&n.to_string(), 11.0, 600.0, pal.text_muted, true));
        let pill = Rect::from_min_size(
            pos2(rect.right() - 6.0 - g.size().x - 12.0, yc - 9.0),
            vec2(g.size().x + 12.0, 18.0),
        );
        ui.painter()
            .rect_filled(pill, CornerRadius::same(9), pal.control);
        ui.painter()
            .galley(pill.center() - g.size() / 2.0, g, pal.text_muted);
    }
    open
}

/// A changed file: status letter, directory in `text-muted`, file name in 550. Long paths
/// lose the start of the directory so the file name stays visible.
pub fn paint_path(painter: &Painter, rect: Rect, left: f32, path: &str, pal: &Palette, size: f32) {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let yc = rect.center().y;
    let avail = rect.right() - left;
    let name_g = galley(painter, name, size, 550.0, pal.text, avail);
    let dir_w = (avail - name_g.size().x).max(0.0);
    let mut x = left;
    if !dir.is_empty() && dir_w > 12.0 {
        let full = painter.layout_job(text_job(dir, size, 400.0, pal.text_muted, false));
        let dir_g = if full.size().x <= dir_w {
            full
        } else {
            // Keep the end of the directory: "…/coa_locations/".
            let chars: Vec<char> = dir.chars().collect();
            let mut out = full;
            for start in 1..chars.len() {
                let s: String = std::iter::once('…')
                    .chain(chars[start..].iter().copied())
                    .collect();
                out = painter.layout_job(text_job(&s, size, 400.0, pal.text_muted, false));
                if out.size().x <= dir_w {
                    break;
                }
            }
            out
        };
        let w = dir_g.size().x;
        painter.galley(pos2(x, yc - dir_g.size().y / 2.0), dir_g, pal.text_muted);
        x += w;
    }
    painter.galley(pos2(x, yc - name_g.size().y / 2.0), name_g, pal.text);
}

// ---------------------------------------------------------------- menus

/// `bb-mi`: a menu item with a 16 px icon, a short label and an optional key hint.
/// `a11y` is the full description (also the tooltip) when the visible label is short.
pub fn menu_item(
    ui: &mut Ui,
    pal: &Palette,
    icon: &'static str,
    text: &str,
    a11y: Option<&str>,
    danger: bool,
) -> Response {
    menu_entry(ui, pal, Some(icon), text, a11y, danger, false, true)
}

/// One of a set of choices: a check mark before the chosen one.
pub fn menu_choice(ui: &mut Ui, pal: &Palette, chosen: bool, text: &str) -> Response {
    // A blank icon keeps the names in line with the chosen one.
    let icon = if chosen { icons::CHECK } else { " " };
    let resp = menu_entry(ui, pal, Some(icon), text, None, false, false, true);
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, chosen, text));
    resp
}

/// A menu item that is shown but can't be used (e.g. "Already in develop").
pub fn menu_item_disabled(ui: &mut Ui, pal: &Palette, icon: &'static str, text: &str) {
    menu_entry(ui, pal, Some(icon), text, None, false, false, false);
}

/// A menu item that opens a submenu (chevron at the right).
pub fn submenu(
    ui: &mut Ui,
    pal: &Palette,
    icon: &'static str,
    text: &str,
    add: impl FnOnce(&mut Ui),
) {
    let resp = menu_entry(ui, pal, Some(icon), text, None, false, true, true);
    egui::containers::menu::SubMenu::new().show(ui, &resp, add);
}

#[allow(clippy::too_many_arguments)]
fn menu_entry(
    ui: &mut Ui,
    pal: &Palette,
    icon: Option<&'static str>,
    text: &str,
    a11y: Option<&str>,
    danger: bool,
    sub: bool,
    enabled: bool,
) -> Response {
    let w = ui.available_width().max(220.0);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(w, 30.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let label = a11y.unwrap_or(text);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let hot = enabled && resp.hovered();
    if hot {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(RADIUS_SM),
            if danger { pal.danger_soft } else { pal.hover },
        );
    }
    let alpha = if enabled { 1.0 } else { 0.45 };
    let fg = if danger { pal.danger } else { pal.text }.gamma_multiply(alpha);
    let icon_c = if danger { pal.danger } else { pal.text_subtle }.gamma_multiply(alpha);
    let yc = rect.center().y;
    let mut x = rect.left() + 8.0;
    if let Some(icon) = icon {
        paint_icon(ui.painter(), pos2(x + 8.0, yc), icon, 16.0, icon_c);
        x += 16.0 + 8.0;
    }
    let right = if sub {
        rect.right() - 28.0
    } else {
        rect.right() - 8.0
    };
    let g = galley(ui.painter(), text, 13.0, 400.0, fg, right - x);
    ui.painter().galley(pos2(x, yc - g.size().y / 2.0), g, fg);
    if sub {
        paint_icon(
            ui.painter(),
            pos2(rect.right() - 15.0, yc),
            icons::CHEVRON_RIGHT,
            14.0,
            icon_c,
        );
    }
    match a11y {
        Some(full) if full != text => resp.on_hover_text(full),
        _ => resp,
    }
}

/// A menu row with two targets: the radio at the left makes the entry the default, the name
/// runs it once (`runnable` false: shown muted, not clickable). Returns (radio, name) clicked.
pub fn menu_radio_item(
    ui: &mut Ui,
    pal: &Palette,
    selected: bool,
    text: &str,
    runnable: bool,
) -> (bool, bool) {
    let w = ui.available_width().max(220.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let (radio_rect, name_rect) = rect.split_left_right_at_x(rect.left() + 32.0);
    let radio = ui
        .interact(
            radio_rect,
            ui.scope_id().with(("radio", text)),
            Sense::click(),
        )
        .on_hover_text("Use as the button's default");
    radio.widget_info(|| {
        WidgetInfo::selected(
            Role::RadioButton,
            true,
            selected,
            format!("Default: {text}"),
        )
    });
    let sense = if runnable {
        Sense::click()
    } else {
        Sense::hover()
    };
    let name = ui.interact(name_rect, ui.scope_id().with(("run", text)), sense);
    name.widget_info(|| WidgetInfo::labeled(Role::Button, runnable, text));
    let p = ui.painter();
    let r = CornerRadius::same(RADIUS_SM);
    if radio.hovered() {
        p.rect_filled(radio_rect.shrink2(vec2(2.0, 1.0)), r, pal.hover);
    } else if name.hovered() && runnable {
        p.rect_filled(name_rect, r, pal.hover);
    }
    // Focus rings for keyboard use.
    for zone in [&radio, &name] {
        if zone.has_focus() {
            p.rect_stroke(
                zone.rect.shrink(1.0),
                r,
                Stroke::new(1.5, pal.accent),
                StrokeKind::Inside,
            );
        }
    }
    let yc = rect.center().y;
    let icon = if selected {
        icons::CIRCLE_DOT
    } else {
        icons::CIRCLE
    };
    let icon_c = if radio.hovered() || selected {
        pal.text
    } else {
        pal.text_subtle
    };
    paint_icon(p, pos2(rect.left() + 16.0, yc), icon, 16.0, icon_c);
    let x = name_rect.left();
    let fg = if runnable { pal.text } else { pal.text_muted };
    let g = galley(p, text, 13.0, 400.0, fg, name_rect.right() - 8.0 - x);
    p.galley(pos2(x, yc - g.size().y / 2.0), g, fg);
    (radio.clicked(), runnable && name.clicked())
}

/// `bb-mdiv`: a hairline between menu groups.
pub fn menu_divider(ui: &mut Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(
        rect.left() + 8.0..=rect.right() - 8.0,
        rect.center().y,
        Stroke::new(1.0, pal.border),
    );
}

/// `bb-menu-title`: what the menu acts on, as a ref chip (or plain text for a commit).
pub fn menu_title(ui: &mut Ui, pal: &Palette, text: &str, chip: Option<(ChipKind, Color32)>) {
    let w = ui.available_width().max(220.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let p = ui.painter();
    let left = pos2(rect.left() + 6.0, rect.center().y);
    match chip {
        Some((kind, lane)) => {
            let (g, cw) = chip_layout(p, text, kind, pal, rect.width() - 12.0);
            paint_chip(p, left, g, cw, kind, lane, pal);
        }
        None => {
            let g = galley(p, text, 12.0, 550.0, pal.text_muted, rect.width() - 12.0);
            p.galley(
                pos2(left.x + 2.0, left.y - g.size().y / 2.0),
                g,
                pal.text_muted,
            );
        }
    }
    menu_divider(ui, pal);
}

// ---------------------------------------------------------------- inputs

/// `bb-input`: a 30 px field with a leading icon and an optional key hint at the right.
pub fn search_field(
    ui: &mut Ui,
    pal: &Palette,
    text: &mut String,
    hint: &str,
    kbd: Option<&str>,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), CONTROL_MD), Sense::hover());
    let id = ui.scope_id().with(("search-field", hint));
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let p = ui.painter();
    let r = CornerRadius::same(RADIUS_MD);
    p.rect_filled(rect, r, pal.canvas);
    p.rect_stroke(
        rect,
        r,
        Stroke::new(1.0, pal.border_control),
        StrokeKind::Inside,
    );
    if focused {
        p.rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(RADIUS_MD + 1),
            Stroke::new(2.0, pal.focus),
            StrokeKind::Outside,
        );
    }
    paint_icon(
        p,
        pos2(rect.left() + 8.0 + 7.0, rect.center().y),
        icons::SEARCH,
        14.0,
        pal.text_subtle,
    );
    let mut right = rect.right() - 8.0;
    if let Some(k) = kbd.filter(|_| text.is_empty()) {
        let g = p.layout_job(text_job(k, 11.0, 400.0, pal.text_subtle, true));
        right -= g.size().x;
        p.galley(
            pos2(right, rect.center().y - g.size().y / 2.0),
            g,
            pal.text_subtle,
        );
        right -= 6.0;
    }
    let field = Rect::from_x_y_ranges(rect.left() + 28.0..=right, rect.y_range());
    ui.scope_builder(egui::UiBuilder::new().max_rect(field), |ui| {
        ui.centered_and_justified(|ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .hint_text(RichText::new(hint).color(pal.text_subtle))
                    .desired_width(field.width())
                    .align(egui::Align2::LEFT_CENTER),
            )
        })
        .inner
    })
    .inner
}

// ---------------------------------------------------------------- busy line

/// The 2 px `accent` line along the top of a tab, sweeping over a faint accent track while
/// its repository works. Without animations it stands still.
pub fn busy_line(ui: &Ui, rect: Rect, pal: &Palette, animate: bool) {
    let y = rect.top() + 1.0;
    let (x0, x1) = (rect.left() + 8.0, rect.right() - 8.0);
    let p = ui.painter();
    p.hline(
        x0..=x1,
        y,
        Stroke::new(2.0, pal.accent.gamma_multiply(0.18)),
    );
    if !animate {
        p.hline(x0..=x1, y, Stroke::new(2.0, pal.accent));
        return;
    }
    let seg = ((x1 - x0) * 0.45).max(12.0);
    let t = ui.input(|i| i.time) as f32;
    // One sweep per 1.1 s, easing in and out (the CSS ease-in-out of the design system).
    let phase = (t / 1.1).fract();
    let eased = phase * phase * (3.0 - 2.0 * phase);
    let start = x0 - seg * 0.7 + eased * (x1 - x0 + seg * 1.4 - seg);
    let (a, b) = (start.max(x0), (start + seg).min(x1));
    if b > a {
        p.hline(a..=b, y, Stroke::new(2.0, pal.accent));
    }
    ui.ctx().request_repaint();
}
