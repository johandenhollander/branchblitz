//! The colours of the desktop theme, for now Omarchy's
//! (`~/.local/state/omarchy/current/theme/colors.toml`). The file is checked every few seconds
//! and the app follows when the user switches themes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui::{Color32, Theme};

use crate::theme::{Ink, Scheme, ThemeDef, mix, readable_on};

/// Name of the theme in Settings.
pub const NAME: &str = "Omarchy";
const CHECK_EVERY: Duration = Duration::from_secs(2);

fn path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))?;
    Some(state.join("omarchy/current/theme/colors.toml"))
}

/// Reads `colors.toml` again when it changed since the last look.
pub struct Watcher {
    path: Option<PathBuf>,
    stamp: Option<(SystemTime, u64)>,
    def: Option<&'static ThemeDef>,
    checked: Option<Instant>,
    /// The last look found no usable file, but the colours from before were kept: Omarchy
    /// replaces the whole theme directory on a switch, so the file is briefly gone.
    missed: bool,
}

impl Watcher {
    pub fn new(path: Option<PathBuf>) -> Self {
        Watcher {
            path,
            stamp: None,
            def: None,
            checked: None,
            missed: false,
        }
    }

    /// True when the theme changed (appeared, disappeared or got other colours).
    pub fn check(&mut self) -> bool {
        self.checked = Some(Instant::now());
        let Some(path) = &self.path else { return false };
        let stamp = std::fs::metadata(path)
            .ok()
            .and_then(|m| Some((m.modified().ok()?, m.len())));
        if stamp == self.stamp && !self.missed {
            return false;
        }
        self.stamp = stamp;
        let def = stamp
            .and_then(|_| std::fs::read_to_string(path).ok())
            .and_then(|text| parse(&text));
        match def {
            Some(d) => {
                // A switch happens a handful of times a day: leaking the old definition is
                // cheaper than handing out lifetimes to every palette.
                self.def = Some(Box::leak(Box::new(d)));
                self.missed = false;
                true
            }
            // Missing or half written: keep the colours one more look.
            None if self.def.is_some() && !self.missed => {
                self.missed = true;
                false
            }
            None => {
                self.missed = false;
                self.def.take().is_some()
            }
        }
    }

    fn due(&self) -> bool {
        self.checked.is_none_or(|t| t.elapsed() >= CHECK_EVERY)
    }
}

static WATCHER: Mutex<Option<Watcher>> = Mutex::new(None);

/// Points the system theme at `file` instead of Omarchy's.
#[cfg(test)]
pub fn use_file(file: PathBuf) {
    let mut w = Watcher::new(Some(file));
    w.check();
    *WATCHER.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
}

/// Makes the next `poll` look at the file.
#[cfg(test)]
pub fn expire() {
    with(|w| w.checked = None);
}

fn with<R>(f: impl FnOnce(&mut Watcher) -> R) -> R {
    let mut guard = WATCHER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(|| {
        let mut w = Watcher::new(path());
        w.check();
        w
    }))
}

/// The system theme, when its colours can be read.
pub fn current() -> Option<&'static ThemeDef> {
    with(|w| w.def)
}

/// Looks at the file when the last look is a while ago; true when the theme changed.
pub fn poll() -> bool {
    with(|w| w.due() && w.check())
}

/// The colours of an Omarchy `colors.toml` as a theme for its own mode (light or dark).
pub fn parse(text: &str) -> Option<ThemeDef> {
    let get = |key: &str| -> Option<String> {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            // `key = "value"`, possibly followed by a comment.
            let v = v.trim().strip_prefix('"')?;
            (k.trim() == key).then_some(())?;
            Some(v[..v.find('"')?].to_string())
        })
    };
    let color = |key: &str| get(key).and_then(|v| hex_color(&v));
    let bg = color("background")?;
    let fg = color("foreground")?;
    let accent = color("accent").or_else(|| color("blue"))?;
    let dark = match get("mode").as_deref() {
        Some("light") => false,
        Some("dark") => true,
        _ => crate::theme::luminance(bg) < 0.4,
    };
    let toward = |t: f32| mix(bg, fg, t);
    let sc = Scheme {
        sunken: color("darker_background").unwrap_or(toward(0.04)),
        app: color("dark_background").unwrap_or(toward(0.02)),
        canvas: bg,
        raised: color("lighter_background").unwrap_or(toward(0.08)),
        control: toward(0.08),
        hover: toward(0.12),
        selected: color("selection").unwrap_or(mix(bg, accent, 0.25)),
        accent,
        accent_hover: mix(accent, fg, 0.2),
        accent_soft: mix(bg, accent, if dark { 0.2 } else { 0.14 }),
        on_accent: readable_on(accent),
    };
    let or = |key: &str, fallback: Color32| color(key).unwrap_or(fallback);
    let danger = or("red", Color32::from_rgb(0xE0, 0x40, 0x40));
    let warning = or("yellow", Color32::from_rgb(0xD0, 0x90, 0x20));
    Some(ThemeDef {
        name: NAME,
        light: sc,
        dark: sc,
        ink: Some(Ink {
            text: fg,
            success: or("green", Color32::from_rgb(0x30, 0xA0, 0x50)),
            warning,
            danger,
            modified: or("orange", warning),
        }),
        mode: Some(if dark { Theme::Dark } else { Theme::Light }),
    })
}

fn hex_color(v: &str) -> Option<Color32> {
    let h = v.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

#[cfg(test)]
pub const SAMPLE_DARK: &str = r##"mode = "dark"
accent = "#89b4fa"
selection = "#45475a"
background = "#1e1e2e"
dark_background = "#161622"
darker_background = "#101019"
lighter_background = "#313244"
foreground = "#cdd6f4"
red = "#f38ba8"
yellow = "#f9e2af"
orange = "#f6b6ab"
green = "#a6e3a1"
"##;

#[cfg(test)]
pub const SAMPLE_LIGHT: &str = r##"mode = "light"
accent = "#1e66f5"
selection = "#ccd0da"
background = "#eff1f5"
dark_background = "#e3e4e8"
darker_background = "#d7d8dc"
lighter_background = "#dce0e8"
foreground = "#4c4f69"
red = "#d20f39"
yellow = "#df8e1d"
orange = "#d84e2b"
green = "#40a02b"
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_mode_and_colours() {
        let d = parse(SAMPLE_DARK).unwrap();
        assert_eq!(d.mode, Some(Theme::Dark));
        assert_eq!(d.dark.canvas, Color32::from_rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(d.dark.accent, Color32::from_rgb(0x89, 0xb4, 0xfa));
        assert_eq!(parse(SAMPLE_LIGHT).unwrap().mode, Some(Theme::Light));
        assert!(
            parse("mode = \"dark\"\n").is_none(),
            "needs background and foreground"
        );
        // A comment after the value; no accent, so blue; an unknown mode goes by the background.
        let t = "mode = \"dusk\"\nbackground = \"#101010\" # night\nforeground = \"#eeeeee\"\nblue = \"#3366ff\"\n";
        let d = parse(t).unwrap();
        assert_eq!(d.dark.canvas, Color32::from_rgb(0x10, 0x10, 0x10));
        assert_eq!(d.dark.accent, Color32::from_rgb(0x33, 0x66, 0xff));
        assert_eq!(d.mode, Some(Theme::Dark));
        assert!(hex_color("#+12345").is_none());
        assert!(hex_color("#12345").is_none());
        assert!(
            parse(&t.replace("#eeeeee", "#eeeexx")).is_none(),
            "bad foreground"
        );
    }

    #[test]
    fn follows_changes_to_the_file() {
        let dir = std::env::temp_dir().join(format!("bb-system-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("colors.toml");
        let mut w = Watcher::new(Some(file.clone()));
        assert!(!w.check(), "no file, nothing changed");
        assert!(w.def.is_none());

        std::fs::write(&file, SAMPLE_DARK).unwrap();
        assert!(w.check());
        assert_eq!(w.def.unwrap().mode, Some(Theme::Dark));
        assert!(!w.check(), "unchanged");

        // Another theme: other size, so seen even within the same mtime second.
        std::fs::write(&file, format!("{SAMPLE_LIGHT}\n# switched\n")).unwrap();
        assert!(w.check());
        assert_eq!(w.def.unwrap().mode, Some(Theme::Light));

        // Half written during a switch: the colours stay, and the next good file is taken.
        std::fs::write(&file, "mode = \"dark\"\n").unwrap();
        assert!(!w.check());
        assert_eq!(w.def.unwrap().mode, Some(Theme::Light));
        std::fs::write(&file, SAMPLE_DARK).unwrap();
        assert!(w.check());
        assert_eq!(w.def.unwrap().mode, Some(Theme::Dark));

        // Gone for good: kept for one look, then dropped.
        std::fs::remove_file(&file).unwrap();
        assert!(!w.check());
        assert!(w.def.is_some(), "kept for one look");
        assert!(w.check());
        assert!(w.def.is_none(), "gone with the file");
        assert!(!w.check());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Every theme Omarchy ships, where installed, gives readable text.
    #[test]
    fn installed_omarchy_themes_stay_readable() {
        let Ok(dirs) = std::fs::read_dir("/usr/share/omarchy/themes") else {
            return;
        };
        for d in dirs.flatten() {
            let Ok(text) = std::fs::read_to_string(d.path().join("colors.toml")) else {
                continue;
            };
            let def = parse(&text).unwrap_or_else(|| panic!("{:?}", d.path()));
            let mode = def.mode.unwrap();
            let pal = crate::theme::Palette::new(&def, mode, &crate::theme::Look::default());
            for (what, c, min) in [("text", pal.text, 4.5), ("muted", pal.text_muted, 4.5)] {
                let r = crate::theme::contrast_ratio(c, pal.canvas);
                assert!(r >= min - 0.01, "{:?} {what}: {r:.2}", d.path());
            }
        }
    }
}
