use super::*;

#[cfg(test)]
mod render_tests {
    //! Headless renders of the UI against the demo repo (`BB_DEMO=/path/to/repo`).
    //! Run with `BB_DEMO=... cargo test render -- --ignored --nocapture`.
    use super::*;
    use egui_kittest::Harness;

    fn harness(theme: egui::Theme) -> Option<Harness<'static, BranchBlitz>> {
        let repo = PathBuf::from(std::env::var("BB_DEMO").ok()?);
        let mut h = Harness::builder()
            .with_size(egui::vec2(1500.0, 820.0))
            .with_theme(theme)
            .build_eframe(move |cc| {
                let mut app = BranchBlitz::new(cc, Some(repo));
                app.settings.theme = match theme {
                    egui::Theme::Dark => ThemeChoice::Dark,
                    egui::Theme::Light => ThemeChoice::Light,
                };
                app.settings.fetch_on_switch = false;
                app
            });
        for _ in 0..400 {
            h.step();
            let tab = &h.state().tabs[0];
            if tab.data.is_some() && tab.running.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.run_steps(4);
        Some(h)
    }

    fn save(h: &mut Harness<'_, BranchBlitz>, name: &str) {
        let img = h.render().expect("render");
        let path = format!("/tmp/bb-{name}.png");
        img.save(&path).unwrap();
        println!("saved {path}");
    }

    #[test]
    #[ignore]
    fn render_screens() {
        for (theme, name) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
            let Some(mut h) = harness(theme) else { return };
            let sha = h.state().tabs[0].data.as_ref().unwrap().commits[1]
                .sha
                .clone();
            h.state_mut()
                .apply(0, vec![Action::Select(Selection::Commit(sha))]);
            for _ in 0..50 {
                h.step();
                if h.state().tabs[0].details.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            h.run_steps(3);
            save(&mut h, name);
            h.state_mut().modal = Some(Modal::Settings);
            h.run_steps(3);
            save(&mut h, &format!("settings-{name}"));
        }
    }

    /// Mid-drag: a commit held over the local `develop` branch (outlined, the label says what a
    /// drop does), then over nothing. Released outside, so the repo is unchanged.
    #[test]
    #[ignore]
    fn render_drag() {
        use egui_kittest::kittest::Queryable;
        let Some(mut h) = harness(egui::Theme::Dark) else {
            return;
        };
        let from = h.get_by_label("PROJ-610 - Fix date format").rect().center();
        let to = h
            .get_all_by_label("develop")
            .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .expect("develop")
            .rect()
            .center();
        h.hover_at(from);
        h.step();
        h.drag_at(from);
        h.step();
        for k in 1..=8 {
            h.hover_at(from + (to - from) * (k as f32 / 8.0));
            h.step();
        }
        h.run_steps(2);
        save(&mut h, "drag-target");
        // The remote develop (lowest), which local develop is behind: pulls it first.
        let remote = h
            .get_all_by_label("develop")
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .expect("origin/develop")
            .rect()
            .center();
        h.hover_at(remote);
        h.run_steps(3);
        save(&mut h, "drag-remote");
        h.hover_at(from + egui::vec2(0.0, 60.0));
        h.run_steps(2);
        save(&mut h, "drag");
        h.drop_at(from + egui::vec2(0.0, 60.0));
        h.run_steps(2);
        assert!(h.state().tabs[0].alert.is_none());
    }

    /// Right-click the remote `develop` branch and merge it into the current branch.
    #[test]
    #[ignore]
    fn render_merge_via_context_menu() {
        use egui_kittest::kittest::Queryable;
        let Some(mut h) = harness(egui::Theme::Dark) else {
            return;
        };
        let current = h.state().tabs[0]
            .data
            .as_ref()
            .unwrap()
            .head_branch
            .clone()
            .unwrap();
        let remote_develop = h
            .get_all_by_label("develop")
            .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
            .expect("remote develop leaf");
        remote_develop.click_secondary();
        h.run_steps(3);
        save(&mut h, "menu");
        h.get_by_label(&format!("Merge origin/develop into {current}"))
            .click();
        h.run_steps(2);
        for _ in 0..300 {
            h.step();
            if h.state().tabs[0].running.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        h.run_steps(3);
        save(&mut h, "merged");
        h.state_mut().apply(
            0,
            vec![
                Action::Select(Selection::Wip),
                Action::ShowDiff(DiffTarget::Unstaged {
                    path: "a.txt".into(),
                    untracked: false,
                }),
            ],
        );
        for _ in 0..50 {
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        save(&mut h, "wip-diff");
        let data = h.state().tabs[0].data.as_ref().unwrap();
        let head = data.head_sha.clone().unwrap();
        let commit = data.commits.iter().find(|c| c.sha == head).unwrap();
        assert_eq!(
            commit.parents.len(),
            2,
            "HEAD should be a merge commit: {}",
            commit.subject
        );
        println!("merge commit: {}", commit.subject);
    }
}

#[cfg(test)]
mod glyph_check {
    /// Every non-ASCII character in the UI code must exist in the bundled fonts,
    /// otherwise it renders as a box.
    #[test]
    fn all_ui_glyphs_exist() {
        let ctx = eframe::egui::Context::default();
        ctx.set_fonts(crate::fonts::definitions(None, None));
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        // The UI code: src/app/, src/views/ and the other modules that draw.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files: Vec<_> = ["app", "views"]
            .iter()
            .flat_map(|d| std::fs::read_dir(src.join(d)).unwrap())
            .map(|e| e.unwrap().path())
            .collect();
        files.extend(["settings_view.rs", "file_view.rs", "ds.rs"].map(|f| src.join(f)));
        assert!(files.len() > 15, "{files:?}");
        let sources: Vec<String> = files
            .iter()
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        let mut missing = String::new();
        ctx.fonts_mut(|f| {
            for c in sources
                .iter()
                .flat_map(|s| s.chars())
                .filter(|c| !c.is_ascii())
                .chain(crate::icons::ALL.iter().flat_map(|s| s.chars()))
            {
                if !f.has_glyph(&eframe::egui::FontId::proportional(13.0), c)
                    && !missing.contains(c)
                {
                    missing.push(c);
                }
            }
        });
        assert!(missing.is_empty(), "missing glyphs: {missing}");
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    /// Settings saved by an older version (fewer fields) still load, with defaults for the rest.
    #[test]
    fn old_settings_still_load() {
        let old = r#"{"theme":"Dark","open_repos":["/tmp/x"],"commit_limit":500}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.commit_limit, 500);
        assert_eq!(s.theme_name, theme::DEFAULT_THEME);
        assert_eq!(s.color_rules, colors::default_rules());
        assert_eq!(s.look(), Look::default());
    }

    #[test]
    fn wheel_scrolling_is_scaled_but_ctrl_zoom_is_not() {
        use eframe::App;
        let mut h = egui_kittest::Harness::builder().build_eframe(|cc| BranchBlitz::new(cc, None));
        let ctx = h.ctx.clone();
        let wheel = |command: bool| egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, -1.0),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: egui::Modifiers {
                command,
                ..Default::default()
            },
        };
        let mut raw = egui::RawInput {
            events: vec![wheel(false), wheel(true)],
            ..Default::default()
        };
        h.state_mut().settings.scroll_speed = 2.5;
        h.state_mut().raw_input_hook(&ctx, &mut raw);
        let dy: Vec<f32> = raw
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                _ => None,
            })
            .collect();
        assert_eq!(dy, vec![-2.5, -1.0]);
    }
}

#[cfg(test)]
mod name_tests {
    use super::ref_name_problem;

    #[test]
    fn validates_ref_names() {
        for ok in ["feature/PROJ-123", "fix_1", "release/3.1.5"] {
            assert_eq!(ref_name_problem(ok), None, "{ok}");
        }
        for bad in [
            "-f",
            "a b",
            "a..b",
            "x.lock",
            "x/",
            "/x",
            "a~1",
            "a:b",
            "a@{1}",
            "x/.hidden",
            "ends.",
        ] {
            assert!(ref_name_problem(bad).is_some(), "{bad}");
        }
    }
}
