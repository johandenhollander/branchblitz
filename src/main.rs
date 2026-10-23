// Windows: a GUI program, without a console window next to it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod agent;
mod app;
mod color_scheme;
mod colors;
mod conflict;
mod ds;
mod fonts;
mod git;
mod graph;
mod icons;
mod live;
mod log;
mod mcp;
mod settings_view;
mod system_theme;
mod theme;
mod views;

/// The window icon. Desktops that go by the app id (Wayland) take the installed
/// `branchblitz` icon from the desktop entry instead.
fn app_icon() -> eframe::egui::IconData {
    let png = include_bytes!("../assets/icons/branchblitz-256.png");
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("bundled icon is a valid PNG")
        .into_rgba8();
    eframe::egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    }
}

/// Where the GPU choice (Settings, General) is kept for the next start: read before the window
/// exists, so before the rest of the settings. Present means "high performance".
pub fn gpu_choice_file() -> Option<std::path::PathBuf> {
    eframe::storage_dir("branchblitz").map(|d| d.join("gpu-high-performance"))
}

fn high_performance_chosen() -> bool {
    gpu_choice_file().is_some_and(|f| f.exists())
}

/// Power saving (the integrated GPU where there are two) unless the user chose high
/// performance: a git client doesn't need the discrete GPU, and keeping it awake costs heat
/// and battery. `WGPU_POWER_PREF` still overrides.
fn chosen_gpu(high: bool) -> eframe::egui_wgpu::WgpuSetupCreateNew {
    use eframe::wgpu::PowerPreference;
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    let chosen = if high {
        PowerPreference::HighPerformance
    } else {
        PowerPreference::LowPower
    };
    setup.power_preference = PowerPreference::from_env().unwrap_or(chosen);
    setup
}

/// Where BranchBlitz lives: releases, issues, the update check.
pub const REPO_URL: &str = "https://github.com/johandenhollander/branchblitz";

const VK_DISABLE: &str = "VK_LOADER_DRIVERS_DISABLE";
/// Set when BranchBlitz hid the NVIDIA Vulkan driver from itself (see `hide_discrete_vulkan`).
static HID_DISCRETE_VULKAN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// With an integrated GPU next to an NVIDIA one, keeps the Vulkan loader from loading the NVIDIA
/// driver. Rendering is on the integrated GPU anyway, but tearing down a Vulkan instance that
/// knows the NVIDIA GPU wakes it if it sleeps: about 2 s when closing the app. Not when the user
/// chose themselves (`WGPU_POWER_PREF`, `VK_LOADER_DRIVERS_*`). Programs the app starts get
/// their environment back (`child_env`).
fn hide_discrete_vulkan() {
    let user_chose = ["WGPU_POWER_PREF", VK_DISABLE, "VK_LOADER_DRIVERS_SELECT"]
        .iter()
        .any(|v| std::env::var_os(v).is_some());
    if user_chose || !cfg!(target_os = "linux") {
        return;
    }
    let drivers: Vec<String> = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"]
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .filter(|n| n.ends_with(".json"))
        .collect();
    let nvidia = drivers.iter().any(|n| n.contains("nvidia"));
    // A hardware driver besides NVIDIA's (lavapipe is software rendering).
    let other = drivers
        .iter()
        .any(|n| !n.contains("nvidia") && !n.contains("lvp"));
    if nvidia && other {
        // SAFETY: called first thing in `main`, before any thread exists.
        unsafe { std::env::set_var(VK_DISABLE, "*nvidia*") };
        HID_DISCRETE_VULKAN.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Prepares a program started by the app: the user's environment (without
/// `hide_discrete_vulkan`), and on Windows no console window flashing up for it.
pub fn child_env(cmd: &mut std::process::Command) -> &mut std::process::Command {
    if HID_DISCRETE_VULKAN.load(std::sync::atomic::Ordering::Relaxed) {
        cmd.env_remove(VK_DISABLE);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn main() -> eframe::Result {
    if std::env::args_os().nth(1).is_some_and(|a| a == "mcp") {
        let args: Vec<String> = std::env::args().collect();
        // `mcp --attach`: the bridge from an agent CLI to a running app's agent tools.
        if args.iter().any(|a| a == "--attach") {
            if let Err(e) = live::attach(&args) {
                eprintln!("branchblitz mcp --attach: {e}");
                std::process::exit(1);
            }
        } else {
            mcp::run();
        }
        return Ok(());
    }
    let crashed = log::init();
    let high = high_performance_chosen();
    if !high {
        hide_discrete_vulkan();
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("BranchBlitz")
            .with_app_id("branchblitz")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(app_icon()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::WgpuConfiguration {
            wgpu_setup: chosen_gpu(high).into(),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "BranchBlitz",
        options,
        Box::new(|cc| {
            let open = std::env::args_os().nth(1).map(std::path::PathBuf::from);
            color_scheme::watch(cc.egui_ctx.clone());
            let mut app = app::BranchBlitz::new(cc, open);
            app.use_installed_agent();
            app.check_for_updates(false);
            if crashed {
                app.report_crash();
            }
            Ok(Box::new(app))
        }),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn app_icon_decodes() {
        let icon = super::app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert_eq!(icon.rgba[3], 0, "rounded corners are transparent");
    }
}
