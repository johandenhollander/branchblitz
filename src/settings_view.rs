//! The Settings window: a large popup with a section list on the left (like T3 Code).
//! Clicking outside it, Escape or the close button closes it.

use eframe::egui::{
    self, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, vec2,
};

use crate::app::Settings;
use crate::colors::{self, ColorRule};
use crate::theme::{self, DiffColors, Look, Palette, RADIUS_LG, Scheme, ThemeChoice};
use crate::{ds, icons};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    General,
    #[default]
    Appearance,
    Agents,
    About,
}

impl Page {
    const ALL: [Page; 4] = [Page::General, Page::Appearance, Page::Agents, Page::About];

    fn title(self) -> &'static str {
        match self {
            Page::General => "General",
            Page::Appearance => "Appearance",
            Page::Agents => "Agents",
            Page::About => "About",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::General => icons::SETTINGS,
            Page::Appearance => icons::PALETTE,
            Page::Agents => icons::BOT,
            Page::About => icons::INFO,
        }
    }
}

#[derive(Default)]
pub struct Outcome {
    pub close: bool,
    pub colors_changed: bool,
    /// Theme, contrast, sizes or animation speed: re-apply the egui style.
    pub style_changed: bool,
    pub fonts_changed: bool,
    pub open_log: bool,
    pub check_updates: bool,
}

/// The size of the Settings window, also used for the agent shown large.
pub fn window_size(ctx: &egui::Context) -> egui::Vec2 {
    let screen = ctx.content_rect();
    vec2(
        (screen.width() * 0.86)
            .clamp(600.0, 1120.0)
            .min(screen.width() - 16.0),
        (screen.height() * 0.86)
            .clamp(420.0, 780.0)
            .min(screen.height() - 16.0),
    )
}

/// What the settings show but don't set.
pub struct Info<'a> {
    /// The agent CLI that was found (or why there is none).
    pub agent: &'a str,
    /// The GPU and graphics API in use.
    pub renderer: &'a str,
    /// The GPU choice the app started with (a change applies at the next start).
    pub high_performance_at_start: bool,
    /// Version, system, rendering, git and agent, for bug reports.
    pub diagnostics: &'a str,
    pub log: Option<&'a str>,
    /// A newer release than this build, and when the app last looked (Unix seconds, 0 = never).
    pub newer: Option<&'a str>,
    pub checked: i64,
}

pub fn window(
    ctx: &egui::Context,
    settings: &mut Settings,
    page: &mut Page,
    info: &Info,
) -> Outcome {
    let mut out = Outcome::default();
    let pal = settings.palette(ctx.theme());
    let size = window_size(ctx);
    let frame = egui::Frame::window(&ctx.global_style())
        .inner_margin(0)
        .stroke(Stroke::new(1.0, pal.border_strong));
    let resp = egui::Modal::new(egui::Id::unique("bb-settings"))
        .frame(frame)
        .show(ctx, |ui| {
            // Fixed rectangles instead of nested layouts: the left column and the page.
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let (nav_rect, page_rect) = rect.split_left_right_at_x(rect.left() + NAV_W);
            ui.painter().rect_filled(
                nav_rect,
                CornerRadius {
                    nw: theme::RADIUS_XL,
                    sw: theme::RADIUS_XL,
                    ne: 0,
                    se: 0,
                },
                pal.sunken,
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(nav_rect.shrink2(vec2(12.0, 20.0))),
                |ui| nav(ui, page, &pal),
            );
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(page_rect.shrink2(vec2(32.0, 20.0))),
                |ui| {
                    if header(ui, *page, &pal) {
                        out.close = true;
                    }
                    ui.add_space(10.0);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| match page {
                            Page::General => general(ui, settings, &pal, info),
                            // Branch colors are part of the look, below the theme and fonts.
                            Page::Appearance => {
                                (out.style_changed, out.fonts_changed) =
                                    appearance(ui, settings, &pal);
                                out.colors_changed =
                                    color_rules_editor(ui, &mut settings.color_rules, &pal);
                            }
                            Page::Agents => agents(ui, settings, &pal, info.agent),
                            Page::About => {
                                (out.open_log, out.check_updates) = about(ui, settings, &pal, info)
                            }
                        });
                },
            );
        });
    if resp.should_close() {
        out.close = true;
    }
    out
}

const NAV_W: f32 = 210.0;

fn nav(ui: &mut Ui, page: &mut Page, pal: &Palette) {
    ui.label(ds::heading("Settings").color(pal.text));
    ui.add_space(12.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    for p in Page::ALL {
        let selected = *page == p;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, p.title())
        });
        let (fill, fg, icon_c) = if selected {
            (pal.accent_soft, pal.accent, pal.accent)
        } else if resp.hovered() {
            (pal.hover, pal.text, pal.text_subtle)
        } else {
            (Color32::TRANSPARENT, pal.text, pal.text_subtle)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(theme::RADIUS_MD), fill);
        ds::paint_icon(
            painter,
            egui::pos2(rect.left() + 10.0 + 8.0, rect.center().y),
            p.icon(),
            16.0,
            icon_c,
        );
        let g = painter.layout_job(ds::text_job(
            p.title(),
            13.0,
            if selected { 550.0 } else { 400.0 },
            fg,
            false,
        ));
        painter.galley(
            egui::pos2(rect.left() + 36.0, rect.center().y - g.size().y / 2.0),
            g,
            fg,
        );
        if resp.clicked() {
            *page = p;
        }
    }
}

/// "Settings / Page" and a close button. Returns whether close was clicked.
fn header(ui: &mut Ui, page: Page, pal: &Palette) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Settings").size(14.0).color(pal.text_muted));
        ui.label(RichText::new("/").size(14.0).color(pal.text_subtle));
        ui.label(ds::heading(page.title()).color(pal.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ds::Button::icon_only(icons::X, "Close")
                .quiet()
                .show(ui, pal)
                .clicked()
            {
                close = true;
            }
        });
    });
    close
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(16.0);
    ui.label(ds::heading(title));
    ui.add_space(8.0);
}

fn general(ui: &mut Ui, s: &mut Settings, pal: &Palette, info: &Info) {
    section(ui, "General");
    egui::Grid::new("general")
        .num_columns(2)
        .spacing([24.0, 14.0])
        .show(ui, |ui| {
            ui.label("Commits to load");
            ui.add(
                egui::DragValue::new(&mut s.commit_limit)
                    .range(100..=50_000)
                    .speed(50),
            );
            ui.end_row();
            ui.label("Fetch");
            ui.checkbox(&mut s.fetch_on_switch, "When opening or switching to a tab");
            ui.end_row();
            ui.label("Scroll speed");
            ui.add(
                egui::Slider::new(&mut s.scroll_speed, 0.5..=5.0)
                    .step_by(0.25)
                    .suffix("×"),
            );
            ui.end_row();
            ui.label("Auto fetch");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.auto_fetch_minutes).range(0..=120));
                ui.label("minutes (0 = off)");
            });
            ui.end_row();
        });
    section(ui, "Graphics");
    egui::Grid::new("graphics")
        .num_columns(2)
        .spacing([24.0, 14.0])
        .show(ui, |ui| {
            ui.label("GPU");
            ui.vertical(|ui| {
                ui.radio_value(&mut s.gpu_high_performance, false, "Power saving");
                ui.label(
                    RichText::new(
                        "The integrated GPU where there are two: cooler, longer on battery.",
                    )
                    .size(12.0)
                    .color(pal.text_subtle),
                );
                ui.add_space(4.0);
                ui.radio_value(&mut s.gpu_high_performance, true, "High performance");
                ui.label(
                    RichText::new("The discrete GPU, e.g. for a monitor connected to it.")
                        .size(12.0)
                        .color(pal.text_subtle),
                );
                if s.gpu_high_performance != info.high_performance_at_start {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Applies when BranchBlitz starts again.").color(pal.warning),
                    );
                }
            });
            ui.end_row();
            ui.label("Rendering");
            ui.label(RichText::new(info.renderer).color(pal.text_muted));
            ui.end_row();
        });
}

const REPO: &str = crate::REPO_URL;

/// Version, updates and diagnostics, with a way to report a bug (the issue form gets the
/// diagnostics). Returns whether to open the log and whether to look for updates now.
fn about(ui: &mut Ui, s: &mut Settings, pal: &Palette, info: &Info) -> (bool, bool) {
    let mut open_log = false;
    let mut check = false;
    section(ui, &format!("BranchBlitz {}", env!("CARGO_PKG_VERSION")));
    ui.label(RichText::new("A fast desktop git client.").color(pal.text_muted));
    ui.add_space(4.0);
    ui.hyperlink_to(REPO.trim_start_matches("https://"), REPO);
    section(ui, "Updates");
    ui.horizontal(|ui| {
        match info.newer {
            Some(v) => {
                ui.label(RichText::new(format!("Version {v} is available.")).color(pal.accent));
                ui.hyperlink_to("Download", format!("{REPO}/releases/tag/v{v}"));
            }
            None if info.checked > 0 => {
                ui.label(RichText::new("This is the newest version.").color(pal.text_muted));
            }
            None => {
                ui.label(RichText::new("Not checked yet.").color(pal.text_muted));
            }
        }
        if ds::Button::new("Check now").small().show(ui, pal).clicked() {
            check = true;
        }
    });
    ui.checkbox(
        &mut s.check_updates,
        "Look for a new version once a day (asks GitHub for its tags)",
    );
    section(ui, "Diagnostics");
    egui::Frame::new()
        .fill(pal.sunken)
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(info.diagnostics).monospace().color(pal.text));
        });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ds::Button::new("Report a bug")
            .icon(icons::BUG)
            .primary()
            .show(ui, pal)
            .clicked()
        {
            let url = format!(
                "{REPO}/issues/new?template=bug.yml&diagnostics={}",
                url_encode(info.diagnostics)
            );
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
        }
        if ds::Button::new("Copy diagnostics")
            .icon(icons::COPY)
            .show(ui, pal)
            .clicked()
        {
            ui.ctx().copy_text(info.diagnostics.to_string());
        }
        if info.log.is_some()
            && ds::Button::new("Open log")
                .icon(icons::EXTERNAL_LINK)
                .show(ui, pal)
                .clicked()
        {
            open_log = true;
        }
    });
    if let Some(log) = info.log {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "The log ({log}) has failed operations and crashes; it can contain repository \
                 names and paths, so check it before sharing."
            ))
            .size(12.0)
            .color(pal.text_subtle),
        );
    }
    (open_log, check)
}

/// Percent-encodes `s` for a URL query value.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The agent in the panel at the bottom right: which CLI, model, access, what to show, and
/// extra instructions.
fn agents(ui: &mut Ui, s: &mut Settings, pal: &Palette, agent: &str) {
    use crate::agent::CLAUDE_MODELS;
    use crate::app::AgentAccess;
    section(ui, "Agent");
    egui::Grid::new("agents")
        .num_columns(2)
        .spacing([24.0, 14.0])
        .show(ui, |ui| {
            ui.label("Agent CLI");
            ui.label(RichText::new(agent).color(pal.text_muted));
            ui.end_row();

            ui.label("Model");
            let name = |m: &Option<String>| match m.as_deref() {
                None => "Claude Code's default".to_string(),
                Some(a) => CLAUDE_MODELS
                    .iter()
                    .find(|(alias, _)| *alias == a)
                    .map_or(a.to_string(), |(_, n)| n.to_string()),
            };
            egui::ComboBox::from_id_salt("agent-model")
                .selected_text(name(&s.agent_model))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.agent_model, None, name(&None));
                    for (alias, n) in CLAUDE_MODELS {
                        ui.selectable_value(&mut s.agent_model, Some(alias.to_string()), *n);
                    }
                });
            ui.end_row();

            ui.label("Access");
            ui.vertical(|ui| {
                for (mode, label, about) in [
                    (
                        AgentAccess::Supervised,
                        "Supervised",
                        "Asks before pushing, rewriting or throwing away work.",
                    ),
                    (
                        AgentAccess::Auto,
                        "Auto",
                        "Pushes, amends and resets on its own; asks before throwing work away.",
                    ),
                    (
                        AgentAccess::Full,
                        "Full access",
                        "Never asks, also not for force push, hard reset or discarding changes.",
                    ),
                ] {
                    ui.radio_value(&mut s.agent_access, mode, label);
                    ui.label(RichText::new(about).size(12.0).color(pal.text_subtle));
                    ui.add_space(4.0);
                }
            });
            ui.end_row();

            ui.label("Tool calls");
            ui.checkbox(
                &mut s.agent_show_tools,
                "Show what the agent does (each tool it calls) in the conversation",
            );
            ui.end_row();
        });
    section(ui, "Instructions");
    ui.label(
        RichText::new(
            "Added to every conversation, e.g. the language to answer in or how to name branches.",
        )
        .color(pal.text_muted),
    );
    ui.add_space(6.0);
    let field = ui.add(
        egui::TextEdit::multiline(&mut s.agent_instructions)
            .desired_rows(6)
            .desired_width(f32::INFINITY),
    );
    field.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::MultilineTextInput, true, "Agent instructions")
    });
}

/// Returns whether the style (theme, contrast, sizes, motion) and whether the fonts changed.
fn appearance(ui: &mut Ui, s: &mut Settings, pal: &Palette) -> (bool, bool) {
    let def = theme::theme_def(&s.theme_name);
    // First: under WSL (and on some high-DPI screens) it is the first thing to set.
    section(ui, "Size");
    row(
        ui,
        "Scale",
        "The size of everything, for screens where it looks too small or too large (also Ctrl + \
         and Ctrl −). Under WSL the Windows display scaling doesn't reach the app: set it here.",
        |ui| {
            let now = ui.ctx().zoom_factor();
            let pct = |z: f32| format!("{:.0}%", z * 100.0);
            let combo = egui::ComboBox::from_id_salt("scale")
                .selected_text(pct(now))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for z in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5] {
                        if ui
                            .selectable_label((now - z).abs() < 0.01, pct(z))
                            .clicked()
                        {
                            ui.ctx().set_zoom_factor(z);
                        }
                    }
                });
            combo.response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Interface scale")
            });
            false
        },
    );
    section(ui, "Color scheme");
    let gap = 12.0;
    let w = (ui.available_width() - 14.0 - 2.0 * gap) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (choice, label) in [
            (ThemeChoice::System, "System"),
            (ThemeChoice::Light, "Light"),
            (ThemeChoice::Dark, "Dark"),
        ] {
            let selected = s.theme == choice;
            let (rect, resp) = card(ui, vec2(w, 150.0), label, selected, pal);
            let preview = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(w - 16.0, 104.0));
            let p = ui.painter();
            match choice {
                ThemeChoice::Light => mini_app(p, preview, &def.light, false),
                ThemeChoice::Dark => mini_app(p, preview, &def.dark, true),
                ThemeChoice::System => {
                    let (left, right) = preview.split_left_right_at_fraction(0.5);
                    mini_app(&p.with_clip_rect(left), preview, &def.light, false);
                    mini_app(&p.with_clip_rect(right), preview, &def.dark, true);
                }
            }
            tile_foot(p, rect, label, selected, pal);
            if resp.clicked() {
                s.theme = choice;
            }
        }
    });
    if let Some(mode) = def.mode {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "{} decides this: it is a {} theme.",
                def.name,
                if mode == egui::Theme::Dark {
                    "dark"
                } else {
                    "light"
                }
            ))
            .color(pal.text_muted),
        );
    }

    section(ui, "Accent");
    let mut changed = false;
    let w = (ui.available_width() - 14.0 - 2.0 * gap) / 3.0;
    for row in theme::all_themes().chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for t in row {
                let selected = t.name == def.name;
                let (rect, resp) = card(ui, vec2(w, 96.0), t.name, selected, pal);
                let p = ui.painter();
                // The light and dark value of the accent, on their own surfaces.
                let art = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(w - 16.0, 50.0));
                if t.mode.is_some() {
                    // One mode only: one surface, with the accent in the middle.
                    let sc = &t.dark;
                    p.rect_filled(art, CornerRadius::same(5), sc.app);
                    p.circle_filled(art.center(), 12.0, sc.accent);
                    p.circle_filled(art.center() + vec2(-26.0, 0.0), 6.0, sc.accent_soft);
                    p.circle_filled(art.center() + vec2(26.0, 0.0), 6.0, sc.selected);
                    tile_foot(p, rect, t.name, selected, pal);
                    let resp = resp.on_hover_text(
                        "The colours of your Omarchy theme; follows when you switch themes",
                    );
                    if resp.clicked() && !selected {
                        s.theme_name = t.name.to_string();
                        changed = true;
                    }
                    continue;
                }
                let (l, d) = art.split_left_right_at_fraction(0.5);
                p.rect_filled(
                    l,
                    CornerRadius {
                        nw: 5,
                        sw: 5,
                        ..Default::default()
                    },
                    t.light.app,
                );
                p.rect_filled(
                    d,
                    CornerRadius {
                        ne: 5,
                        se: 5,
                        ..Default::default()
                    },
                    t.dark.app,
                );
                for (half, sc) in [(l, &t.light), (d, &t.dark)] {
                    p.circle_filled(half.center(), 12.0, sc.accent);
                    p.circle_filled(half.center() + vec2(-26.0, 0.0), 6.0, sc.accent_soft);
                    p.circle_filled(half.center() + vec2(26.0, 0.0), 6.0, sc.selected);
                }
                tile_foot(p, rect, t.name, selected, pal);
                if resp.clicked() && !selected {
                    s.theme_name = t.name.to_string();
                    changed = true;
                }
            }
        });
        ui.add_space(gap);
    }
    let style = interface(ui, s) | changed;
    let (style2, fonts) = typography(ui, s, pal);
    (style | style2, fonts)
}

/// The name of a tile and, when chosen, the accent check badge.
fn tile_foot(p: &egui::Painter, rect: Rect, label: &str, selected: bool, pal: &Palette) {
    let y = rect.bottom() - 19.0;
    let g = p.layout_job(ds::text_job(label, 13.0, 550.0, pal.text, false));
    p.galley(
        egui::pos2(rect.left() + 12.0, y - g.size().y / 2.0),
        g,
        pal.text,
    );
    if selected {
        let c = egui::pos2(rect.right() - 12.0 - 9.0, y);
        p.circle_filled(c, 9.0, pal.accent);
        ds::paint_icon(p, c, icons::CHECK, 12.0, pal.on_accent);
    }
}

/// A setting as a row: title and explanation on the left, the control on the right.
fn row<R>(ui: &mut Ui, title: &str, help: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let mut out = None;
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(RADIUS_LG))
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            // Leave room for the scroll bar.
            ui.set_width(ui.available_width() - 14.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    // The explanation wraps before it reaches the control on the right.
                    ui.set_max_width((ui.available_width() - 360.0).max(200.0));
                    ui.label(ds::strong(title));
                    ui.label(RichText::new(help).weak());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    out = Some(control(ui));
                });
            });
        });
    ui.add_space(8.0);
    out.expect("control ran")
}

fn interface(ui: &mut Ui, s: &mut Settings) -> bool {
    let mut changed = false;
    section(ui, "Interface");
    changed |= row(
        ui,
        "Contrast",
        "Borders, secondary text and branch colours. Branch colours always keep at least WCAG 3:1 against the background; higher contrast raises that.",
        |ui| {
            ui.add(
                egui::Slider::new(&mut s.contrast, 0.5..=1.5)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                    .custom_parser(|t| {
                        t.trim_end_matches('%')
                            .parse::<f64>()
                            .ok()
                            .map(|v| v / 100.0)
                    }),
            )
            .changed()
        },
    );
    row(
        ui,
        "Diff colors",
        "Colours for added and removed lines.",
        |ui| {
            let (name, look) = (s.theme_name.clone(), s.look());
            egui::ComboBox::from_id_salt("diff-colors")
                .selected_text(diff_label(ui, &name, look, s.diff_colors))
                .width(190.0)
                .show_ui(ui, |ui| {
                    for d in [DiffColors::RedGreen, DiffColors::BlueOrange] {
                        let t = diff_label(ui, &name, look, d);
                        ui.selectable_value(&mut s.diff_colors, d, t);
                    }
                });
        },
    );
    section(ui, "Motion");
    changed |= row(
        ui,
        "Animations",
        "How fast menus, panels and collapsing sections open and close. 0 turns them off.",
        |ui| {
            ui.add(egui::Slider::new(&mut s.animation_ms, 0..=300).suffix(" ms"))
                .changed()
        },
    );
    changed
}

/// "■ ■ Red & green", with the dots in the actual diff colours of the theme.
fn diff_label(ui: &Ui, theme_name: &str, look: Look, d: DiffColors) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let p = Palette::new(
        theme::theme_def(theme_name),
        ui.ctx().theme(),
        &Look { diff: d, ..look },
    );
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = LayoutJob::default();
    job.append("■ ", 0.0, TextFormat::simple(font.clone(), p.removed));
    job.append("■ ", 0.0, TextFormat::simple(font.clone(), p.added));
    job.append(
        d.label(),
        2.0,
        TextFormat::simple(font, ui.visuals().text_color()),
    );
    job
}

/// Returns (style changed, fonts changed).
fn typography(ui: &mut Ui, s: &mut Settings, pal: &Palette) -> (bool, bool) {
    let (mut style, mut fonts) = (false, false);
    section(ui, "Typography");
    let (all, mono) = crate::fonts::families();
    let picker = |ui: &mut Ui,
                  id: &str,
                  value: &mut Option<String>,
                  names: &[String],
                  bundled: &str|
     -> bool {
        let before = value.clone();
        egui::ComboBox::from_id_salt(id)
            .selected_text(value.clone().unwrap_or_else(|| bundled.into()))
            .width(210.0)
            .height(320.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(value, None, bundled);
                for n in names {
                    ui.selectable_value(value, Some(n.clone()), n);
                }
            });
        *value != before
    };
    row(
        ui,
        "Interface font",
        "Everything except diffs and commit hashes.",
        |ui| {
            style |= ui
                .add(
                    egui::DragValue::new(&mut s.ui_font_size)
                        .range(11.0..=18.0)
                        .speed(0.1)
                        .suffix(" px"),
                )
                .changed();
            fonts |= picker(ui, "ui-font", &mut s.ui_font, all, "Inter (built-in)");
        },
    );
    preview_box(ui, |ui| {
        ui.label("Merge origin/develop into feature/PROJ-523 — 3 files changed, ready to push.");
    });
    row(
        ui,
        "Monospace font",
        "Diffs, file names in diffs and commit hashes.",
        |ui| {
            style |= ui
                .add(
                    egui::DragValue::new(&mut s.mono_font_size)
                        .range(10.0..=18.0)
                        .speed(0.1)
                        .suffix(" px"),
                )
                .changed();
            fonts |= picker(
                ui,
                "mono-font",
                &mut s.mono_font,
                if mono.is_empty() { all } else { mono },
                "JetBrains Mono (built-in)",
            );
        },
    );
    preview_box(ui, |ui| {
        let font = ui.style().text_styles[&egui::TextStyle::Monospace].clone();
        for (line, bg) in [
            ("@@ -1,3 +1,3 @@", Color32::TRANSPARENT),
            (
                " export function formatUser(user: User) {",
                Color32::TRANSPARENT,
            ),
            ("-  return user.name.toUpperCase();", pal.removed_bg),
            (
                "+  return `${user.name} <${user.email}>`; // 0O 1lI",
                pal.added_bg,
            ),
            (" }", Color32::TRANSPARENT),
        ] {
            let fg = if line.starts_with("@@") {
                pal.hunk
            } else {
                ui.visuals().text_color()
            };
            let g = ui
                .painter()
                .layout_no_wrap(line.to_string(), font.clone(), fg);
            let (r, _) = ui
                .allocate_exact_size(vec2(ui.available_width(), g.size().y + 4.0), Sense::hover());
            ui.painter().rect_filled(r, 0.0, bg);
            ui.painter().galley(r.left_top() + vec2(8.0, 2.0), g, fg);
        }
    });
    (style, fonts)
}

fn preview_box(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width() - 14.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    ui.add_space(12.0);
}

/// `bb-tile`: a clickable card, ringed in the accent when chosen; `label` is its accessible name.
fn card(
    ui: &mut Ui,
    size: egui::Vec2,
    label: &str,
    selected: bool,
    pal: &Palette,
) -> (Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    let stroke = if selected {
        Stroke::new(2.0, pal.accent)
    } else if resp.hovered() {
        Stroke::new(1.0, pal.border_strong)
    } else {
        Stroke::new(1.0, pal.border)
    };
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS_LG), pal.raised);
    p.rect_stroke(
        rect,
        CornerRadius::same(RADIUS_LG),
        stroke,
        StrokeKind::Inside,
    );
    (rect, resp)
}

/// A tiny picture of the app in a colour scheme: sidebar, toolbar, graph lines, an accent pill.
fn mini_app(p: &egui::Painter, r: Rect, sc: &Scheme, dark: bool) {
    let line = if dark {
        Color32::from_white_alpha(40)
    } else {
        Color32::from_black_alpha(35)
    };
    p.rect_filled(r, CornerRadius::same(5), sc.canvas);
    let side = Rect::from_min_max(r.min, egui::pos2(r.left() + r.width() * 0.24, r.bottom()));
    p.rect_filled(
        side,
        CornerRadius {
            nw: 6,
            sw: 6,
            ne: 0,
            se: 0,
        },
        sc.app,
    );
    let top = Rect::from_min_max(
        egui::pos2(side.right(), r.top()),
        egui::pos2(r.right(), r.top() + 18.0),
    );
    p.rect_filled(
        top,
        CornerRadius {
            ne: 6,
            ..Default::default()
        },
        sc.sunken,
    );
    for k in 0..3 {
        let y = side.top() + 14.0 + k as f32 * 12.0;
        p.rect_filled(
            Rect::from_min_size(
                egui::pos2(side.left() + 8.0, y),
                vec2(side.width() - 16.0, 6.0),
            ),
            CornerRadius::same(3),
            line,
        );
    }
    let gx = side.right() + 18.0;
    for k in 0..5 {
        let y = top.bottom() + 14.0 + k as f32 * 16.0;
        if y > r.bottom() - 8.0 {
            break;
        }
        p.circle_filled(
            egui::pos2(gx, y),
            3.5,
            if k == 0 { sc.accent } else { line },
        );
        p.rect_filled(
            Rect::from_min_size(
                egui::pos2(gx + 12.0, y - 3.0),
                vec2((r.right() - gx - 24.0) * (0.9 - k as f32 * 0.12), 6.0),
            ),
            CornerRadius::same(3),
            line,
        );
    }
    p.line_segment(
        [
            egui::pos2(gx, top.bottom() + 14.0),
            egui::pos2(gx, r.bottom() - 8.0),
        ],
        Stroke::new(1.5, sc.accent.gamma_multiply(0.7)),
    );
    let pill = Rect::from_min_size(
        egui::pos2(r.right() - 58.0, top.top() + 4.0),
        vec2(48.0, 10.0),
    );
    p.rect_filled(pill, CornerRadius::same(5), sc.accent);
}

/// Editor for the branch colour rules. Returns whether anything changed.
pub fn color_rules_editor(ui: &mut Ui, rules: &mut Vec<ColorRule>, pal: &Palette) -> bool {
    let mut changed = false;
    section(ui, "Branch colors");
    ui.label(
        RichText::new(
            "Comma-separated names, * matches anything. The first matching rule wins, and \
             higher rules own shared history. git-flow names (git flow init) count as their \
             role. Tags take the colour of their branch; other branches get an automatic colour. \
             In the graph, colours are lightened or darkened where needed to keep WCAG contrast \
             with the theme.",
        )
        .weak(),
    );
    ui.add_space(10.0);
    let (mut remove, mut raise) = (None, None);
    egui::Grid::new("color_rules")
        .num_columns(4)
        .spacing([10.0, 8.0])
        .show(ui, |ui| {
            for (n, rule) in rules.iter_mut().enumerate() {
                let [r, g, b] = rule.color;
                let mut c = Color32::from_rgb(r, g, b);
                if egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut c,
                    egui::color_picker::Alpha::Opaque,
                )
                .changed()
                {
                    rule.color = [c.r(), c.g(), c.b()];
                    changed = true;
                }
                // Grid cells shrink to their content; give the field a fixed size.
                changed |= ui
                    .add_sized(
                        [340.0, 24.0],
                        egui::TextEdit::singleline(&mut rule.patterns).hint_text("e.g. release/*"),
                    )
                    .changed();
                if n > 0 {
                    if ds::Button::icon_only(icons::ARROW_UP, "Higher priority")
                        .quiet()
                        .small()
                        .show(ui, pal)
                        .clicked()
                    {
                        raise = Some(n);
                    }
                } else {
                    ui.label("");
                }
                if ds::Button::icon_only(icons::X, "Remove rule")
                    .quiet()
                    .small()
                    .show(ui, pal)
                    .clicked()
                {
                    remove = Some(n);
                }
                ui.end_row();
            }
        });
    if let Some(n) = raise {
        rules.swap(n - 1, n);
        changed = true;
    }
    if let Some(n) = remove {
        rules.remove(n);
        changed = true;
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ds::Button::new("Add rule")
            .icon(icons::PLUS)
            .show(ui, pal)
            .clicked()
        {
            rules.push(ColorRule {
                patterns: String::new(),
                color: [148, 163, 184],
            });
            changed = true;
        }
        if ds::Button::new("Reset to defaults")
            .quiet()
            .show(ui, pal)
            .clicked()
        {
            *rules = colors::default_rules();
            changed = true;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    #[test]
    fn url_encode_keeps_safe_characters() {
        assert_eq!(
            super::url_encode("Git: 2.47 a/b\né"),
            "Git%3A%202.47%20a%2Fb%0A%C3%A9"
        );
    }
}
