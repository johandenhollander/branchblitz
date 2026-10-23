//! Performance baseline (`scripts/perf.sh`): times for loading, laying out and drawing a large
//! repository, measured the same way at every release. Ignored test; needs `BB_PERF=<repo>`
//! (from `scripts/perf-repo.sh`) and writes JSON to `BB_PERF_OUT`. Run in release mode.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use eframe::egui::{self, vec2};
    use egui_kittest::Harness;
    use serde_json::{Value, json};

    use crate::app::BranchBlitz;
    use crate::{colors, git, graph};

    /// Median wall time of `runs` calls, in milliseconds.
    fn median_ms(runs: usize, mut f: impl FnMut()) -> f64 {
        let mut t: Vec<f64> = (0..runs)
            .map(|_| {
                let start = Instant::now();
                f();
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        t.sort_by(f64::total_cmp);
        t[t.len() / 2]
    }

    fn percentile(sorted: &[f64], p: f64) -> f64 {
        sorted[((sorted.len() - 1) as f64 * p).round() as usize]
    }

    fn rss_peak_mb() -> f64 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find_map(|l| l.strip_prefix("VmHWM:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())
            })
            .map_or(0.0, |kb| kb / 1024.0)
    }

    /// The app on `repo` without a window, until it shows the graph; the time that took.
    fn open(repo: &Path) -> (Harness<'static, BranchBlitz>, f64) {
        let start = Instant::now();
        let repo = repo.to_path_buf();
        let mut h = Harness::builder()
            .with_size(vec2(1400.0, 860.0))
            .with_step_dt(1.0 / 60.0)
            .build_eframe(move |cc| {
                let mut app = BranchBlitz::new(cc, None);
                app.set_headless();
                app.open_for_automation(&repo).unwrap();
                app
            });
        let deadline = Instant::now() + Duration::from_secs(60);
        while h
            .state()
            .tabs_ref()
            .first()
            .is_none_or(|t| t.data.is_none())
        {
            assert!(Instant::now() < deadline, "repository did not load");
            h.step();
            std::thread::sleep(Duration::from_millis(1));
        }
        h.step();
        let took = start.elapsed().as_secs_f64() * 1000.0;
        // Let the background fetch finish, so it doesn't run during the frame timings.
        while h.state().is_busy() {
            assert!(Instant::now() < deadline, "repository stayed busy");
            h.step();
            std::thread::sleep(Duration::from_millis(5));
        }
        (h, took)
    }

    #[test]
    #[ignore]
    fn perf_baseline() {
        let Some(repo) = std::env::var_os("BB_PERF").map(PathBuf::from) else {
            panic!("set BB_PERF to the repository from scripts/perf-repo.sh")
        };
        let mut m = serde_json::Map::new();
        let mut put = |k: &str, v: f64| {
            m.insert(k.into(), json!((v * 10.0).round() / 10.0));
        };

        // Loading: refs, the commit list, status (what every reload does).
        put(
            "load_2000_ms",
            median_ms(5, || drop(git::load(&repo, 2000).unwrap())),
        );
        put(
            "load_10000_ms",
            median_ms(3, || drop(git::load(&repo, 10000).unwrap())),
        );
        let data = git::load(&repo, 10000).unwrap();
        let rules = colors::default_rules();
        put(
            "colors_10000_ms",
            median_ms(5, || drop(colors::assign(&data, &rules))),
        );
        let coloring = colors::assign(&data, &rules);
        put(
            "graph_10000_ms",
            median_ms(5, || drop(graph::build(&data.commits, &coloring.commit))),
        );
        let merge = data.commits.iter().find(|c| c.parents.len() > 1).unwrap();
        let target = git::DiffTarget::Commit {
            sha: merge.sha.clone(),
            path: "features".into(),
        };
        put("diff_ms", median_ms(5, || drop(git::diff(&repo, &target))));

        // The app: opening the repository, then drawing while scrolling through the graph.
        let (mut h, open_ms) = open(&repo);
        put("open_to_graph_ms", open_ms);
        let mut frames = Vec::new();
        for i in 0..240 {
            let dy = if (i / 60) % 2 == 0 { -120.0 } else { 120.0 };
            h.event(egui::Event::PointerMoved(egui::pos2(700.0, 400.0)));
            h.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, dy),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            });
            let start = Instant::now();
            h.step();
            frames.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        frames.sort_by(f64::total_cmp);
        put("frame_scroll_p50_ms", percentile(&frames, 0.5));
        put("frame_scroll_p95_ms", percentile(&frames, 0.95));
        let idle: Vec<f64> = (0..60)
            .map(|_| {
                let start = Instant::now();
                h.step();
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        let mut idle = idle;
        idle.sort_by(f64::total_cmp);
        put("frame_idle_p50_ms", percentile(&idle, 0.5));
        put("rss_peak_mb", rss_peak_mb());

        let v = Value::Object(m);
        let text = serde_json::to_string_pretty(&v).unwrap();
        println!("{text}");
        if let Some(out) = std::env::var_os("BB_PERF_OUT") {
            std::fs::write(out, text + "\n").unwrap();
        }
    }
}
