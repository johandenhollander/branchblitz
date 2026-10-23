//! The desktop's light/dark preference (`org.freedesktop.appearance color-scheme` from the XDG
//! desktop portal). winit doesn't report the system theme on Linux, so "System" would otherwise
//! never follow the desktop.

use std::sync::atomic::{AtomicU8, Ordering};

use eframe::egui;
use zbus::zvariant::{OwnedValue, Value};

const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

/// 0: unknown or no preference, 1: dark, 2: light (the portal's values).
static SCHEME: AtomicU8 = AtomicU8::new(0);

/// The desktop's preference, when it has one.
pub fn current() -> Option<egui::Theme> {
    from_portal(SCHEME.load(Ordering::Relaxed) as u32)
}

fn from_portal(v: u32) -> Option<egui::Theme> {
    match v {
        1 => Some(egui::Theme::Dark),
        2 => Some(egui::Theme::Light),
        _ => None,
    }
}

/// Reads the preference and keeps following it on a background thread; repaints on a change.
/// Without a portal nothing happens and "System" stays egui's own guess.
pub fn watch(ctx: egui::Context) {
    std::thread::spawn(move || {
        let _ = listen(&ctx);
    });
}

fn listen(ctx: &egui::Context) -> zbus::Result<()> {
    let conn = zbus::blocking::Connection::session()?;
    let proxy = zbus::blocking::Proxy::new(
        &conn,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )?;
    // Subscribe first, so a change between reading and subscribing isn't missed.
    let changes = proxy.receive_signal("SettingChanged")?;
    if let Ok(v) = proxy.call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY)) {
        set(ctx, &v);
    }
    for msg in changes {
        if let Ok((ns, key, v)) = msg.body().deserialize::<(String, String, OwnedValue)>()
            && ns == NAMESPACE
            && key == KEY
        {
            set(ctx, &v);
        }
    }
    Ok(())
}

fn set(ctx: &egui::Context, v: &Value) {
    let n = number(v).unwrap_or(0).min(2) as u8;
    if SCHEME.swap(n, Ordering::Relaxed) != n {
        ctx.request_repaint();
    }
}

/// The number in a portal value, which may come wrapped in a variant.
fn number(v: &Value) -> Option<u32> {
    match v {
        Value::U32(n) => Some(*n),
        Value::Value(inner) => number(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_portal_values() {
        assert_eq!(from_portal(1), Some(egui::Theme::Dark));
        assert_eq!(from_portal(2), Some(egui::Theme::Light));
        assert_eq!(from_portal(0), None, "no preference");
        assert_eq!(number(&Value::U32(2)), Some(2));
        assert_eq!(
            number(&Value::Value(Box::new(Value::U32(1)))),
            Some(1),
            "wrapped in a variant"
        );
        assert_eq!(number(&Value::from("dark")), None);
    }
}
