use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Theme};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        }
    }
}

/// Colours for added and removed lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DiffColors {
    #[default]
    RedGreen,
    /// Distinguishable with red-green colour blindness.
    BlueOrange,
}

impl DiffColors {
    pub fn label(self) -> &'static str {
        match self {
            DiffColors::RedGreen => "Red & green",
            DiffColors::BlueOrange => "Blue & orange",
        }
    }
}

/// User adjustments on top of a theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// 1.0 = as designed; higher makes borders and branch colours stand out more.
    pub contrast: f32,
    pub animation_ms: u32,
    pub ui_size: f32,
    pub mono_size: f32,
    pub diff: DiffColors,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            contrast: 1.0,
            animation_ms: 80,
            ui_size: 13.0,
            mono_size: 12.0,
            diff: DiffColors::RedGreen,
        }
    }
}

/// One look of the app, with a light and a dark variant.
pub struct ThemeDef {
    pub name: &'static str,
    pub light: Scheme,
    pub dark: Scheme,
    /// Text, border and status colours; `None` uses those of the design system.
    pub ink: Option<Ink>,
    /// A theme made for one mode only, which then also decides light or dark.
    pub mode: Option<Theme>,
}

/// Text and status colours of a theme that brings its own (the system theme).
#[derive(Clone, Copy)]
pub struct Ink {
    pub text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub modified: Color32,
}

/// Surface and accent colours of one variant of a theme (design system tokens `bg-*`, `accent*`).
#[derive(Clone, Copy)]
pub struct Scheme {
    /// Repo tab bar and the settings nav: the lowest chrome layer.
    pub sunken: Color32,
    /// Toolbar, sidebar, detail panel, status bar.
    pub app: Color32,
    /// Commit list and diff.
    pub canvas: Color32,
    /// Menus, dialogs, popovers.
    pub raised: Color32,
    /// Resting fill of default buttons.
    pub control: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    /// Tinted fill for accent badges; `accent` text reads on it.
    pub accent_soft: Color32,
    /// Text and icons on an `accent` fill.
    pub on_accent: Color32,
}

/// Surfaces in back-to-front order, then hover/selected, then the accent group.
#[allow(clippy::too_many_arguments)]
const fn scheme(
    sunken: u32,
    app: u32,
    canvas: u32,
    raised: u32,
    control: u32,
    hover: u32,
    selected: u32,
    accent: u32,
    accent_hover: u32,
    accent_soft: u32,
    on_accent: u32,
) -> Scheme {
    Scheme {
        sunken: hex(sunken),
        app: hex(app),
        canvas: hex(canvas),
        raised: hex(raised),
        control: hex(control),
        hover: hex(hover),
        selected: hex(selected),
        accent: hex(accent),
        accent_hover: hex(accent_hover),
        accent_soft: hex(accent_soft),
        on_accent: hex(on_accent),
    }
}

pub const DEFAULT_THEME: &str = "BranchBlitz";

pub const THEMES: &[ThemeDef] = &[
    // The BranchBlitz design system ("Blitz" accent, from the logo).
    ThemeDef {
        name: DEFAULT_THEME,
        light: scheme(
            0xEAEAF1, 0xF4F4F8, 0xFCFCFE, 0xFFFFFF, 0xE8E8EF, 0xE0DFE9, 0xE5DFF7, 0x7A3BF0,
            0x6A2BE0, 0xEFE8FE, 0xFFFFFF,
        ),
        dark: scheme(
            0x16151F, 0x1F1D2B, 0x1A1925, 0x282637, 0x2C2A3C, 0x353348, 0x34295A, 0xA58CFA,
            0xB9A4FF, 0x2F2457, 0x160442,
        ),
        ink: None,
        mode: None,
    },
    ThemeDef {
        name: "Ocean",
        light: scheme(
            0xE1ECF4, 0xEEF5FA, 0xF8FBFD, 0xFFFFFF, 0xDFE6EC, 0xD7DDE4, 0xD6E9F4, 0x0277BD,
            0x0269A6, 0xE2EFF7, 0xFFFFFF,
        ),
        dark: scheme(
            0x16212B, 0x1B2733, 0x141E28, 0x22303D, 0x27323E, 0x333E4A, 0x23495E, 0x4FC3F7,
            0x72CFF9, 0x1F3C4D, 0x0E151C,
        ),
        ink: None,
        mode: None,
    },
    ThemeDef {
        name: "Grove",
        light: scheme(
            0xE7EFE6, 0xF3F7F2, 0xFBFDFA, 0xFFFFFF, 0xE4E8E4, 0xDCDFDC, 0xDEEDE3, 0x2E8B57,
            0x287A4D, 0xE9F3EB, 0xFFFFFF,
        ),
        dark: scheme(
            0x1A201C, 0x1F2621, 0x171D19, 0x263029, 0x2B312D, 0x373D3A, 0x2A4D3B, 0x5FD49A,
            0x7FDDAE, 0x243E30, 0x101412,
        ),
        ink: None,
        mode: None,
    },
    ThemeDef {
        name: "Ember",
        light: scheme(
            0xF3E8E0, 0xFBF5F1, 0xFFFCFA, 0xFFFFFF, 0xECE6E3, 0xE3DDDC, 0xFAE6DD, 0xD9622B,
            0xBF5626, 0xFCEEE7, 0xFFFFFF,
        ),
        dark: scheme(
            0x241C18, 0x2A211D, 0x1F1814, 0x332822, 0x352D2A, 0x413836, 0x593B26, 0xFF9F5A,
            0xFFB27B, 0x473021, 0x16110E,
        ),
        ink: None,
        mode: None,
    },
    ThemeDef {
        name: "Iris",
        light: scheme(
            0xECE8F7, 0xF5F3FB, 0xFDFCFF, 0xFFFFFF, 0xE6E4ED, 0xDDDBE4, 0xEBE4FA, 0x7C4DDB,
            0x6D44C1, 0xF1ECFC, 0xFFFFFF,
        ),
        dark: scheme(
            0x1D1A28, 0x221F2E, 0x1A1724, 0x2A2638, 0x2E2B3A, 0x3A3745, 0x3F355C, 0xA78BFA,
            0xB9A2FB, 0x332C4B, 0x121019,
        ),
        ink: None,
        mode: None,
    },
];

/// The theme with this name, or the default one. The system theme counts while its colours
/// can be read.
pub fn theme_def(name: &str) -> &'static ThemeDef {
    if name == crate::system_theme::NAME
        && let Some(def) = crate::system_theme::current()
    {
        return def;
    }
    THEMES.iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

/// Every theme to choose from: the built-in ones, then the system theme when there is one.
pub fn all_themes() -> Vec<&'static ThemeDef> {
    THEMES
        .iter()
        .chain(crate::system_theme::current())
        .collect()
}

impl ThemeDef {
    pub fn scheme(&self, theme: Theme) -> &Scheme {
        match theme {
            Theme::Dark => &self.dark,
            Theme::Light => &self.light,
        }
    }
}

/// Graph lane colours of the design system (`lane-1` … `lane-8`), light then dark.
const LANES_LIGHT: [Color32; 8] = [
    hex(0x2F80ED),
    hex(0x15803D),
    hex(0xDC2626),
    hex(0xD97706),
    hex(0x9333EA),
    hex(0x0D9488),
    hex(0xDB2777),
    hex(0x65A30D),
];
pub const LANES_DARK: [Color32; 8] = [
    hex(0x3B82F6),
    hex(0x22C55E),
    hex(0xEF4444),
    hex(0xF59E0B),
    hex(0xA78BFA),
    hex(0x2DD4BF),
    hex(0xF472B6),
    hex(0xA3E635),
];

/// Every colour the UI paints with, for the active theme and light/dark mode.
/// Names follow the design system tokens.
#[derive(Clone)]
pub struct Palette {
    pub sunken: Color32,
    pub app: Color32,
    pub canvas: Color32,
    pub raised: Color32,
    pub control: Color32,
    pub hover: Color32,
    pub selected: Color32,
    /// Hairlines between panels, rows and menu groups.
    pub border: Color32,
    pub border_strong: Color32,
    /// Input and checkbox outlines (3:1).
    pub border_control: Color32,
    pub text: Color32,
    /// Author, date, secondary labels.
    pub text_muted: Color32,
    /// Column headers, captions, placeholders, decorative icons.
    pub text_subtle: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub focus: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub warning_soft: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    /// Added and removed lines and files; blue/orange when chosen in Settings.
    pub added: Color32,
    pub removed: Color32,
    pub modified: Color32,
    pub added_bg: Color32,
    pub removed_bg: Color32,
    pub hunk: Color32,
    pub lanes: [Color32; 8],
    /// Minimum contrast ratio for graph lines and labels (WCAG 1.4.11 asks 3:1).
    pub min_contrast: f32,
    pub dark: bool,
}

impl Palette {
    pub fn new(def: &ThemeDef, theme: Theme, look: &Look) -> Self {
        let sc = def.scheme(theme);
        let mut pal = Self::for_theme(theme);
        pal.sunken = sc.sunken;
        pal.app = sc.app;
        pal.canvas = sc.canvas;
        pal.raised = sc.raised;
        pal.control = sc.control;
        pal.hover = sc.hover;
        pal.selected = sc.selected;
        pal.accent = sc.accent;
        pal.accent_hover = sc.accent_hover;
        pal.accent_soft = sc.accent_soft;
        pal.on_accent = sc.on_accent;
        pal.focus = if pal.dark { sc.accent_hover } else { sc.accent };
        if let Some(ink) = def.ink {
            pal.apply_ink(&ink);
        }
        pal.min_contrast = (3.0 * look.contrast).clamp(3.0, 7.0);
        if look.diff == DiffColors::BlueOrange {
            let (add, del) = if pal.dark {
                (hex(0x5AA9F7), hex(0xF5A04A))
            } else {
                (hex(0x1C6FD1), hex(0xC4610F))
            };
            pal.added = add;
            pal.removed = del;
            pal.added_bg = mix(pal.canvas, add, if pal.dark { 0.2 } else { 0.14 });
            pal.removed_bg = mix(pal.canvas, del, if pal.dark { 0.2 } else { 0.14 });
        }
        // Contrast: borders and secondary text move towards the text colour, or away from it.
        let (fg, bg) = (pal.text, sc.app);
        let tune = |c: Color32| {
            if look.contrast >= 1.0 {
                mix(c, fg, ((look.contrast - 1.0) * 0.8).min(1.0))
            } else {
                mix(c, bg, ((1.0 - look.contrast) * 0.8).min(1.0))
            }
        };
        pal.border = tune(pal.border);
        pal.border_strong = tune(pal.border_strong);
        pal.border_control = tune(pal.border_control);
        // Secondary text never drops below its design contrast.
        if look.contrast > 1.0 {
            pal.text_muted = tune(pal.text_muted);
            pal.text_subtle = tune(pal.text_subtle);
        }
        pal
    }

    /// Text, borders and status colours from `ink`, derived for the surfaces already set and
    /// kept readable on them.
    fn apply_ink(&mut self, ink: &Ink) {
        let (bg, fg) = (self.canvas, ink.text);
        // Readable on the commit list and panels, and at least 4.5:1 on menus and selected rows.
        let surfaces = [self.canvas, self.app, self.raised, self.selected];
        let readable = |c: Color32, min: f32| {
            let mut c = c;
            for _ in 0..2 {
                for (k, &on) in surfaces.iter().enumerate() {
                    c = ensure_contrast(c, on, if k < 2 { min } else { min.min(4.5) });
                }
            }
            c
        };
        self.text = readable(fg, 7.0);
        self.text_muted = readable(mix(fg, bg, 0.3), 4.5);
        self.text_subtle = readable(mix(fg, bg, 0.42), 4.5);
        self.border = mix(bg, fg, 0.12);
        self.border_strong = mix(bg, fg, 0.24);
        self.border_control = readable(mix(bg, fg, 0.4), 3.0);
        self.success = readable(ink.success, 4.5);
        self.warning = readable(ink.warning, 4.5);
        self.danger = readable(ink.danger, 4.5);
        self.modified = readable(ink.modified, 4.5);
        let soft = if self.dark { 0.2 } else { 0.14 };
        self.warning_soft = mix(bg, self.warning, soft);
        self.danger_soft = mix(bg, self.danger, soft);
        self.added = self.success;
        self.removed = self.danger;
        self.added_bg = mix(bg, self.added, soft);
        self.removed_bg = mix(bg, self.removed, soft);
        self.hunk = readable(self.accent, 4.5);
    }

    /// The design system tokens of the BranchBlitz theme.
    pub fn for_theme(theme: Theme) -> Self {
        let def = &THEMES[0];
        let sc = def.scheme(theme);
        let base = |dark: bool| Palette {
            sunken: sc.sunken,
            app: sc.app,
            canvas: sc.canvas,
            raised: sc.raised,
            control: sc.control,
            hover: sc.hover,
            selected: sc.selected,
            border: Color32::PLACEHOLDER,
            border_strong: Color32::PLACEHOLDER,
            border_control: Color32::PLACEHOLDER,
            text: Color32::PLACEHOLDER,
            text_muted: Color32::PLACEHOLDER,
            text_subtle: Color32::PLACEHOLDER,
            accent: sc.accent,
            accent_hover: sc.accent_hover,
            accent_soft: sc.accent_soft,
            on_accent: sc.on_accent,
            focus: if dark { sc.accent_hover } else { sc.accent },
            success: Color32::PLACEHOLDER,
            warning: Color32::PLACEHOLDER,
            warning_soft: Color32::PLACEHOLDER,
            danger: Color32::PLACEHOLDER,
            danger_soft: Color32::PLACEHOLDER,
            added: Color32::PLACEHOLDER,
            removed: Color32::PLACEHOLDER,
            modified: Color32::PLACEHOLDER,
            added_bg: Color32::PLACEHOLDER,
            removed_bg: Color32::PLACEHOLDER,
            hunk: Color32::PLACEHOLDER,
            lanes: if dark { LANES_DARK } else { LANES_LIGHT },
            min_contrast: 3.0,
            dark,
        };
        match theme {
            Theme::Dark => Palette {
                border: hex(0x2F2D40),
                border_strong: hex(0x45425C),
                border_control: hex(0x6A6782),
                text: hex(0xE6E4F0),
                text_muted: hex(0xA9A6BC),
                text_subtle: hex(0x8D8AA3),
                success: hex(0x4AD18A),
                warning: hex(0xF3C9A4),
                warning_soft: hex(0x3A2A22),
                danger: hex(0xFF7A80),
                danger_soft: hex(0x3D1C24),
                added: hex(0x4AD18A),
                removed: hex(0xFF7A80),
                modified: hex(0xFF9A5C),
                added_bg: hex(0x183626),
                removed_bg: hex(0x3D1C24),
                hunk: hex(0x8EA2FF),
                ..base(true)
            },
            Theme::Light => Palette {
                border: hex(0xDCDCE5),
                border_strong: hex(0xBBBCC6),
                border_control: hex(0x8E8CA0),
                text: hex(0x1E1B2E),
                text_muted: hex(0x5C5A70),
                text_subtle: hex(0x6E6B82),
                success: hex(0x0F7040),
                warning: hex(0x7A3D0C),
                warning_soft: hex(0xFBEEE4),
                danger: hex(0xC7303A),
                danger_soft: hex(0xFBE6E8),
                added: hex(0x0F7040),
                removed: hex(0xC7303A),
                modified: hex(0xC2510F),
                added_bg: hex(0xE3F4EA),
                removed_bg: hex(0xFBE6E8),
                hunk: hex(0x3B57D6),
                ..base(false)
            },
        }
    }

    /// `c` adjusted (lighter on dark, darker on light backgrounds) until it has at least the
    /// required contrast with the commit list background.
    pub fn graphic(&self, c: Color32) -> Color32 {
        ensure_contrast(c, self.canvas, self.min_contrast)
    }

    pub fn lane(&self, i: usize) -> Color32 {
        self.lanes[i % self.lanes.len()]
    }

    /// A branch rule colour; the design system lanes (saved as their dark value) switch to
    /// their light value in light mode.
    pub fn rule_color(&self, c: Color32) -> Color32 {
        LANES_DARK
            .iter()
            .position(|&l| l == c)
            .map_or(c, |k| self.lanes[k])
    }

    pub fn status_color(&self, status: char) -> Color32 {
        match status {
            'A' | '?' => self.added,
            'D' => self.removed,
            // Unmerged: a conflict to resolve.
            'U' => self.danger,
            _ => self.modified,
        }
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    let ch = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
}

/// WCAG contrast ratio, 1..21.
pub fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `c`, moved towards white (dark background) or black (light background) just enough to
/// reach `min` contrast with `bg`.
pub fn ensure_contrast(c: Color32, bg: Color32, min: f32) -> Color32 {
    use std::cell::RefCell;
    use std::collections::HashMap;
    // Called for every graph line every frame; the same few colours come back each time.
    type Key = ([u8; 3], [u8; 3], u32);
    thread_local! {
        static CACHE: RefCell<HashMap<Key, Color32>> = RefCell::new(HashMap::new());
    }
    let key = (
        [c.r(), c.g(), c.b()],
        [bg.r(), bg.g(), bg.b()],
        (min * 100.0) as u32,
    );
    if let Some(hit) = CACHE.with(|m| m.borrow().get(&key).copied()) {
        return hit;
    }
    let out = adjust_contrast(c, bg, min);
    CACHE.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 4096 {
            m.clear();
        }
        m.insert(key, out);
    });
    out
}

fn adjust_contrast(c: Color32, bg: Color32, min: f32) -> Color32 {
    let c = Color32::from_rgb(c.r(), c.g(), c.b());
    // On mid-grey backgrounds not even black or white reach high ratios; aim for what can be.
    let best = contrast_ratio(Color32::WHITE, bg).max(contrast_ratio(Color32::BLACK, bg));
    let min = min.min(best * 0.98);
    if contrast_ratio(c, bg) >= min {
        return c;
    }
    let target = if contrast_ratio(Color32::WHITE, bg) >= contrast_ratio(Color32::BLACK, bg) {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    (1..=20)
        .map(|k| mix(c, target, k as f32 / 20.0))
        .find(|m| contrast_ratio(*m, bg) >= min)
        .unwrap_or(target)
}

/// Near-black or white, whichever reads better on `fill`.
pub fn readable_on(fill: Color32) -> Color32 {
    let (dark, light) = (hex(0x111318), Color32::WHITE);
    if contrast_ratio(dark, fill) >= contrast_ratio(light, fill) {
        dark
    } else {
        light
    }
}

/// `a` moved towards `b` by `t` (0..1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

pub const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Fonts, spacing and the colours of `def` for both light and dark mode.
pub fn setup_style(ctx: &egui::Context, def: &ThemeDef, look: &Look) {
    for theme in [Theme::Dark, Theme::Light] {
        let pal = Palette::new(def, theme, look);
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (TextStyle::Heading, FontId::proportional(look.ui_size + 4.0)),
                (TextStyle::Body, FontId::proportional(look.ui_size)),
                (TextStyle::Button, FontId::proportional(look.ui_size)),
                (
                    TextStyle::Small,
                    FontId::proportional((look.ui_size - 1.0).max(8.0)),
                ),
                (TextStyle::Monospace, FontId::monospace(look.mono_size)),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(8.0, 4.0);
            style.spacing.button_padding = egui::vec2(10.0, 4.0);
            style.spacing.menu_margin = egui::Margin::same(4);
            style.spacing.interact_size.y = (look.ui_size + 11.0).max(24.0);
            style.animation_time = look.animation_ms as f32 / 1000.0;
            let v = &mut style.visuals;
            let shadow = |y: i8, blur: u8, alpha: u8| egui::Shadow {
                offset: [0, y],
                blur,
                spread: 0,
                color: if pal.dark {
                    Color32::from_black_alpha(alpha.saturating_mul(3))
                } else {
                    Color32::from_rgba_unmultiplied(30, 27, 46, alpha)
                },
            };
            v.window_shadow = shadow(24, 64, 56);
            v.popup_shadow = shadow(8, 24, 36);
            v.window_corner_radius = CornerRadius::same(RADIUS_XL);
            v.menu_corner_radius = CornerRadius::same(RADIUS_LG);
            v.window_stroke = Stroke::new(1.0, pal.border);
            v.panel_fill = pal.app;
            v.window_fill = pal.raised;
            v.faint_bg_color = pal.app;
            v.extreme_bg_color = pal.canvas;
            v.text_edit_bg_color = Some(pal.canvas);
            v.code_bg_color = pal.control;
            v.hyperlink_color = pal.accent;
            v.warn_fg_color = pal.warning;
            v.error_fg_color = pal.danger;
            v.weak_text_color = Some(pal.text_muted);
            v.selection.bg_fill = pal.selected;
            v.selection.stroke = Stroke::new(1.5, pal.focus);
            v.text_cursor.stroke = Stroke::new(2.0, pal.accent);
            let w = &mut v.widgets;
            for (wv, fill, stroke) in [
                (&mut w.noninteractive, pal.app, pal.border),
                (&mut w.inactive, pal.control, pal.border_control),
                (&mut w.hovered, pal.hover, pal.border_control),
                (&mut w.active, pal.hover, pal.border_control),
                (&mut w.open, pal.hover, pal.border_control),
            ] {
                wv.bg_fill = fill;
                wv.weak_bg_fill = fill;
                wv.bg_stroke = Stroke::new(1.0, stroke);
                wv.fg_stroke = Stroke::new(1.0, pal.text);
                wv.corner_radius = CornerRadius::same(RADIUS_MD);
                wv.expansion = 0.0;
            }
        });
    }
}

/// Radii of the design system (`radius-xs` … `radius-xl`).
pub const RADIUS_XS: u8 = 3;
pub const RADIUS_SM: u8 = 5;
pub const RADIUS_MD: u8 = 7;
pub const RADIUS_LG: u8 = 10;
pub const RADIUS_XL: u8 = 14;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_colours_keep_wcag_contrast_in_every_theme() {
        let rules = crate::colors::default_rules();
        for def in THEMES {
            for theme in [Theme::Dark, Theme::Light] {
                let pal = Palette::new(def, theme, &Look::default());
                for r in &rules {
                    let c = pal.graphic(
                        pal.rule_color(Color32::from_rgb(r.color[0], r.color[1], r.color[2])),
                    );
                    let ratio = contrast_ratio(c, pal.canvas);
                    assert!(
                        ratio >= 3.0,
                        "{} {theme:?} {}: {ratio:.2}",
                        def.name,
                        r.patterns
                    );
                    assert!(
                        contrast_ratio(readable_on(c), c) >= 4.5,
                        "{} text",
                        r.patterns
                    );
                }
            }
        }
    }

    #[test]
    fn system_theme_text_stays_readable() {
        for text in [
            crate::system_theme::SAMPLE_DARK,
            crate::system_theme::SAMPLE_LIGHT,
        ] {
            let def = crate::system_theme::parse(text).unwrap();
            let mode = def.mode.unwrap();
            let pal = Palette::new(&def, mode, &Look::default());
            assert_eq!(pal.dark, mode == Theme::Dark);
            for (what, c, min) in [
                ("text", pal.text, 7.0),
                ("muted", pal.text_muted, 4.5),
                ("subtle", pal.text_subtle, 4.5),
                ("danger", pal.danger, 4.5),
                ("success", pal.success, 4.5),
                ("warning", pal.warning, 4.5),
                ("modified", pal.modified, 4.5),
                ("added", pal.added, 4.5),
            ] {
                for bg in [pal.canvas, pal.app] {
                    let r = contrast_ratio(c, bg);
                    assert!(r >= min - 0.01, "{mode:?} {what}: {r:.2}");
                }
                for bg in [pal.raised, pal.selected] {
                    let r = contrast_ratio(c, bg);
                    assert!(r >= 4.49, "{mode:?} {what} on menus/selection: {r:.2}");
                }
            }
        }
    }

    #[test]
    fn contrast_maths() {
        assert!((contrast_ratio(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        let yellow = hex(0xEAB308);
        let on_white = ensure_contrast(yellow, Color32::WHITE, 3.0);
        assert!(contrast_ratio(on_white, Color32::WHITE) >= 3.0);
        assert_eq!(
            ensure_contrast(yellow, hex(0x181A1F), 3.0),
            yellow,
            "already fine on dark"
        );
    }
}
