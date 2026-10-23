//! The bundled design system fonts (Inter, JetBrains Mono, Lucide icons) and optional system
//! fonts for the interface and for code, found with fontconfig (`fc-list`, `fc-match`).
//! Without fontconfig only the bundled fonts are offered.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily, FontTweak};

const INTER: &[u8] = include_bytes!("../assets/fonts/InterVariable-latin.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMonoVariable-latin.ttf");
const LUCIDE: &[u8] = include_bytes!("../assets/fonts/lucide.ttf");

/// Installed font families: (all, monospace). Looked up once, on first use.
pub fn families() -> &'static (Vec<String>, Vec<String>) {
    static LIST: OnceLock<(Vec<String>, Vec<String>)> = OnceLock::new();
    LIST.get_or_init(|| {
        let list = |pattern: &str| -> Vec<String> {
            let mut v: Vec<String> = run(&["fc-list", "--format", "%{family[0]}\n", pattern])
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            v.sort_by_key(|s| s.to_lowercase());
            v.dedup();
            v
        };
        (list(":style=Regular"), list(":spacing=mono:style=Regular"))
    })
}

fn run(args: &[&str]) -> String {
    Command::new(args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// The regular face of `family`: file contents and face index within the file.
fn load(family: &str) -> Option<FontData> {
    let out = run(&[
        "fc-match",
        "--format",
        "%{family[0]}\t%{file}\t%{index}",
        "--",
        &format!("{}:style=Regular", escape(family)),
    ]);
    let mut parts = out.split('\t');
    let (found, file, index) = (parts.next()?, parts.next()?, parts.next()?);
    // fc-match substitutes a similar font for unknown names; only take the one asked for.
    let usable = [".ttf", ".otf", ".ttc"]
        .iter()
        .any(|ext| file.to_lowercase().ends_with(ext));
    if !found.eq_ignore_ascii_case(family) || !usable {
        return None;
    }
    let bytes = std::fs::read(file).ok()?;
    let mut data = FontData::from_owned(bytes);
    // fontconfig keeps the named instance of variable fonts in the upper 16 bits.
    data.index = index.trim().parse::<u32>().unwrap_or(0) & 0xFFFF;
    Some(data)
}

/// Escapes the characters that have a meaning in fontconfig patterns (`-` starts a size).
fn escape(family: &str) -> String {
    family
        .chars()
        .flat_map(|c| {
            let special = matches!(c, '\\' | '-' | ':' | ',');
            special.then_some('\\').into_iter().chain(Some(c))
        })
        .collect()
}

/// Inter for the interface, JetBrains Mono for code and Lucide icons in both; the chosen
/// system fonts go in front. egui's own fonts stay as fallback (arrows, symbols, emoji).
pub fn apply(ctx: &egui::Context, ui_font: Option<&str>, mono_font: Option<&str>) {
    ctx.set_fonts(definitions(ui_font, mono_font));
}

pub fn definitions(ui_font: Option<&str>, mono_font: Option<&str>) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let mut add = |key: &str, data: FontData| {
        defs.font_data.insert(key.into(), data.into());
    };
    add("inter", FontData::from_static(INTER));
    add("jetbrains-mono", FontData::from_static(JETBRAINS_MONO));
    // The icons sit on the baseline in a 1 em box; move them down to the middle of the line.
    add(
        "lucide",
        FontData::from_static(LUCIDE).tweak(FontTweak {
            y_offset_factor: 0.11,
            ..Default::default()
        }),
    );
    for (choice, family, bundled) in [
        (ui_font, FontFamily::Proportional, "inter"),
        (mono_font, FontFamily::Monospace, "jetbrains-mono"),
    ] {
        let list = defs.families.entry(family.clone()).or_default();
        list.insert(0, "lucide".into());
        list.insert(0, bundled.into());
        if let Some(name) = choice
            && let Some(data) = load(name)
        {
            let key = format!("user:{name}");
            defs.font_data.insert(key.clone(), data.into());
            defs.families.entry(family).or_default().insert(0, key);
        }
    }
    defs
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_fontconfig_pattern_characters() {
        assert_eq!(super::escape("DejaVu Sans-Bold"), "DejaVu Sans\\-Bold");
        assert_eq!(super::escape("a:b,c"), "a\\:b\\,c");
    }
}
