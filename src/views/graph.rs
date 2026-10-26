//! The commit graph: columns, lanes, ref pills, find bar, inline branch name field.

use super::*;

// ---------------------------------------------------------------- graph

struct Cols {
    /// Distance between lanes: wider when the dots carry the author's initials.
    lane_w: f32,
    refs: (f32, f32),
    graph: (f32, f32),
    msg: (f32, f32),
    author: (f32, f32),
    date: (f32, f32),
}

/// User-set widths of the resizable columns; the message takes the rest.
#[derive(Clone, Copy)]
struct ColWidths {
    refs: f32,
    /// `None`: as wide as the lanes need.
    graph: Option<f32>,
    author: f32,
    date: f32,
    /// The author column is shown (when the window is wide enough).
    show_author: bool,
    lane_w: f32,
}

/// What the commit list shows of the author (from Settings).
#[derive(Clone, Copy)]
pub struct GraphOpts {
    pub author_column: bool,
    /// The commit dots carry the author's initials.
    pub author_on_node: bool,
}

/// Lane distance and dot radius of a commit with the author's initials on it.
const AVATAR_LANE_W: f32 = 22.0;
const AVATAR_R: f32 = ds::AVATAR_R;

/// Width the lanes of `lanes` need.
fn graph_fit(lanes: usize, lane_w: f32) -> f32 {
    (lanes.max(1) as f32 * lane_w + GRAPH_PAD * 2.0).clamp(60.0, 360.0)
}

/// `bb-commits` grid: refs · graph · message · author · date, 12 px padding and gaps.
fn columns(left: f32, width: f32, w: ColWidths, lanes: usize) -> Cols {
    let (pad, gap) = (12.0, 12.0);
    let graph_w = w
        .graph
        .unwrap_or_else(|| graph_fit(lanes, w.lane_w))
        .clamp(40.0, 600.0);
    let r0 = left + pad;
    let g0 = r0 + w.refs + gap;
    let m0 = g0 + graph_w + gap;
    let d1 = left + width - pad;
    let d0 = (d1 - w.date).max(m0 + 100.0);
    // The message keeps at least 100 px; the author column gives way first.
    let author_w = if w.show_author && width > 820.0 {
        w.author.min(d0 - gap - (m0 + 100.0 + gap)).max(0.0)
    } else {
        0.0
    };
    let a0 = d0 - gap - author_w;
    Cols {
        lane_w: w.lane_w,
        refs: (r0, r0 + w.refs),
        graph: (g0, g0 + graph_w),
        msg: (m0, if author_w > 0.0 { a0 - gap } else { d0 - gap }),
        author: (a0, a0 + author_w),
        date: (d0, d1),
    }
}

/// Labels for one commit, merging a local branch with its same-named remote branches.
pub(super) struct Pill {
    /// Branch name without remote prefix, or the tag name.
    pub(super) text: String,
    /// The remotes this chip stands for, when the branch is elsewhere on another remote.
    pub(super) prefix: Option<String>,
    refs: Vec<usize>,
    pub(super) kind: ChipKind,
    remotes: Vec<String>,
}

impl Pill {
    /// Full ref names, for tooltips and the accessible name.
    pub(super) fn full_names(&self, data: &RepoData) -> String {
        self.refs
            .iter()
            .map(|&i| data.refs[i].name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(super) fn pills_for(idxs: &[usize], data: &RepoData) -> Vec<Pill> {
    let mut pills: Vec<Pill> = Vec::new();
    let mut order: Vec<usize> = idxs.to_vec();
    let rank = |i: &usize| {
        let r = &data.refs[*i];
        let cur = r.kind == RefKind::Local && data.head_branch.as_deref() == Some(r.name.as_str());
        (
            if cur { 0 } else { 1 },
            match r.kind {
                RefKind::Local => 0,
                RefKind::Remote => 1,
                RefKind::Tag => 2,
            },
        )
    };
    order.sort_by_key(rank);
    for i in order {
        let r = &data.refs[i];
        // A branch and its copies on the remotes at this commit: one chip, named without the
        // remote prefix; the cloud icon says it is on a remote.
        if r.kind != RefKind::Tag
            && let Some(p) = pills
                .iter_mut()
                .find(|p| !p.kind.tag && p.text == r.branch_name())
        {
            match r.kind {
                RefKind::Local => {
                    p.kind.local = true;
                    p.kind.worktree |= data.worktree_of(&r.name).is_some();
                }
                _ => {
                    p.kind.remote = true;
                    p.remotes.extend(r.remote.clone());
                }
            }
            p.kind.default |= data.is_default_branch(r);
            p.refs.push(i);
            continue;
        }
        pills.push(Pill {
            text: match r.kind {
                RefKind::Tag => r.name.clone(),
                _ => r.branch_name().to_string(),
            },
            prefix: None,
            refs: vec![i],
            kind: chip_kind(r, data),
            remotes: r.remote.clone().into_iter().collect(),
        });
    }
    // Name the remote only when the same branch sits elsewhere on another remote
    // (origin/develop here, azure/develop further down); otherwise the cloud icon says enough.
    for p in &mut pills {
        let elsewhere = !p.remotes.is_empty()
            && data.refs.iter().enumerate().any(|(i, r)| {
                r.kind == RefKind::Remote && r.branch_name() == p.text && !p.refs.contains(&i)
            });
        if elsewhere {
            p.prefix = Some(p.remotes.join(" · "));
        }
    }
    pills
}

/// The colour a commit row shows: canvas, with the selection or hover tint on top.
/// A find query (lower case) matches the subject anywhere or the hash from its start.
fn commit_matches(c: &crate::git::Commit, query: &str) -> bool {
    c.sha.starts_with(query) || c.subject.to_lowercase().contains(query)
}

/// Seconds of no typing before find searches the whole history (a git process per search).
const SEARCH_DELAY: f64 = 0.3;

/// The find bar, floating at the top right of the graph: field, "n of m", previous/next and
/// close. Enter goes to the next match, Shift+Enter to the previous; typing jumps to the first
/// match from the selection on. True when it should close.
#[allow(clippy::too_many_arguments)]
fn find_bar(
    ui: &mut Ui,
    full: Rect,
    find: &mut Option<Find>,
    hits: &[usize],
    sel_row: Option<usize>,
    data: &RepoData,
    row_of: &std::collections::HashMap<String, usize>,
    pal: &Palette,
    acts: &mut Vec<Action>,
) -> bool {
    let Some(f) = find.as_mut() else { return false };
    let mut close = false;
    // From the selected row: the next match below it, or the previous one above it (wrapping).
    let step = |forward: bool| -> Option<usize> {
        let (first, last) = (*hits.first()?, *hits.last()?);
        Some(if forward {
            hits.iter()
                .copied()
                .find(|&h| sel_row.is_none_or(|r| h > r))
                .unwrap_or(first)
        } else {
            hits.iter()
                .rev()
                .copied()
                .find(|&h| sel_row.is_some_and(|r| h < r))
                .unwrap_or(last)
        })
    };
    let go = |row: Option<usize>, acts: &mut Vec<Action>| {
        if let Some(r) = row {
            acts.push(Action::Jump(data.commits[r].sha.clone()));
        }
    };
    if let Some(at) = f.search_at {
        let now = ui.input(|i| i.time);
        if now >= at {
            f.search_at = None;
            acts.push(Action::FindHistory(f.query.trim().to_string()));
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(at - now));
        }
    }
    if std::mem::take(&mut f.jump) && !hits.is_empty() {
        let from = hits
            .iter()
            .copied()
            .find(|&h| sel_row.is_none_or(|r| h >= r))
            .unwrap_or(hits[0]);
        go(Some(from), acts);
    }
    let width = 380.0;
    egui::Area::new(ui.scope_id().with("find-bar"))
        .order(egui::Order::Middle)
        .fixed_pos(pos2(full.right() - width - 12.0, full.top() + 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(pal.raised)
                .stroke(Stroke::new(1.0, pal.border_strong))
                .corner_radius(CornerRadius::same(crate::theme::RADIUS_MD))
                .inner_margin(4.0)
                .shadow(ui.style().visuals.popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(width - 8.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let field = ui
                            .scope(|ui| {
                                ui.set_width(200.0);
                                ds::search_field(
                                    ui,
                                    pal,
                                    &mut f.query,
                                    "Find message or hash",
                                    None,
                                )
                            })
                            .inner;
                        if std::mem::take(&mut f.focus) {
                            field.request_focus();
                        }
                        let shift = ui.input(|i| i.modifiers.shift);
                        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            go(step(!shift), acts);
                            field.request_focus();
                        } else if field.changed() {
                            f.jump = true;
                            let q = f.query.trim();
                            f.search_at = (q.chars().count() >= 2)
                                .then(|| ui.input(|i| i.time) + SEARCH_DELAY);
                            ui.ctx().request_repaint();
                        }
                        if (field.has_focus() || field.lost_focus())
                            && ui.input(|i| i.key_pressed(egui::Key::Escape))
                        {
                            close = true;
                        }
                        let at = sel_row.and_then(|r| hits.iter().position(|&h| h == r));
                        let count = match (at, hits.len()) {
                            _ if f.query.trim().is_empty() => String::new(),
                            (_, 0) => "No results".into(),
                            (Some(i), n) => format!("{} of {n}", i + 1),
                            (None, n) => format!("{n} found"),
                        };
                        let g = ui.painter().layout_job(ds::text_job(
                            &count,
                            12.0,
                            400.0,
                            pal.text_muted,
                            false,
                        ));
                        let (r, resp) = ui.allocate_exact_size(vec2(76.0, 24.0), Sense::hover());
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::Label, true, count.clone())
                        });
                        ui.painter().galley(
                            pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0),
                            g,
                            pal.text_muted,
                        );
                        let any = !hits.is_empty();
                        let btn = |icon, label: &str| {
                            ds::Button::icon_only(icon, label)
                                .quiet()
                                .small()
                                .enabled(any)
                        };
                        if btn(icons::CHEVRON_UP, "Previous match (Shift+Enter)")
                            .show(ui, pal)
                            .clicked()
                        {
                            go(step(false), acts);
                        }
                        if btn(icons::CHEVRON_DOWN, "Next match (Enter)")
                            .show(ui, pal)
                            .clicked()
                        {
                            go(step(true), acts);
                        }
                        if ds::Button::icon_only(icons::X, "Close find (Esc)")
                            .quiet()
                            .small()
                            .show(ui, pal)
                            .clicked()
                        {
                            close = true;
                        }
                    });
                    // Matches in commits that are not loaded yet: load until the first one.
                    let current = f
                        .older
                        .as_ref()
                        .filter(|(q, _)| q.to_lowercase() == f.query.trim().to_lowercase());
                    // Git stopped at the maximum: there may be more than it found.
                    let capped =
                        current.is_some_and(|(_, shas)| shas.len() >= crate::app::FIND_HISTORY_MAX);
                    let older: Vec<&String> = current
                        .map(|(_, shas)| shas.iter().filter(|s| !row_of.contains_key(*s)).collect())
                        .unwrap_or_default();
                    if let Some(first) = older.first() {
                        ui.horizontal(|ui| {
                            let n = older.len();
                            let text = format!(
                                "{n}{} more in older history",
                                if capped { "+" } else { "" }
                            );
                            let g = ui.painter().layout_job(ds::text_job(
                                &text,
                                12.0,
                                400.0,
                                pal.text_muted,
                                false,
                            ));
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(width - 120.0, 24.0), Sense::hover());
                            resp.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::Label, true, text.clone())
                            });
                            ui.painter().galley(
                                pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0),
                                g,
                                pal.text_muted,
                            );
                            if ds::Button::new("Show older match")
                                .small()
                                .show(ui, pal)
                                .clicked()
                            {
                                acts.push(Action::LoadUntil((*first).clone()));
                            }
                        });
                    }
                });
        });
    close
}

fn row_background(pal: &Palette, selected: bool, hovered: bool) -> Color32 {
    let tint = |c: Color32| crate::theme::mix(pal.canvas, c, c.a() as f32 / 255.0);
    if selected {
        tint(pal.selected)
    } else if hovered {
        tint(pal.hover)
    } else {
        pal.canvas
    }
}

/// A 20 px gradient from transparent to `bg` ending at `x`: lanes that continue out of view.
fn fade_right(painter: &Painter, x: f32, y: egui::Rangef, bg: Color32) {
    let r = Rect::from_x_y_ranges((x - 20.0)..=x, y);
    let clear = Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 0);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), clear);
    mesh.colored_vertex(r.right_top(), bg);
    mesh.colored_vertex(r.right_bottom(), bg);
    mesh.colored_vertex(r.left_bottom(), clear);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Width the branch column needs for the chips of every commit, drawn as in the graph.
fn fit_refs(
    painter: &Painter,
    data: &RepoData,
    refs_at: &std::collections::HashMap<String, Vec<usize>>,
    pal: &Palette,
) -> f32 {
    refs_at
        .values()
        .map(|idxs| {
            let pills = pills_for(idxs, data);
            let shown = if pills.len() > 2 { 1 } else { pills.len() };
            let mut w: f32 = pills[..shown]
                .iter()
                .map(|p| {
                    ds::chip_layout_prefixed(
                        painter,
                        p.prefix.as_deref(),
                        &p.text,
                        p.kind,
                        pal,
                        1e6,
                    )
                    .1 + 4.0
                })
                .sum();
            if pills.len() > shown {
                let g = painter.layout_job(ds::text_job(
                    &format!("+{}", pills.len() - shown),
                    11.0,
                    600.0,
                    pal.text_muted,
                    true,
                ));
                w += g.size().x + 14.0;
            }
            w + 4.0
        })
        .fold(80.0, f32::max)
}

/// Centre of a lane in the graph column.
fn lane_x(cols: &Cols, lane: usize) -> f32 {
    cols.graph.0 + GRAPH_PAD + cols.lane_w * lane as f32 + cols.lane_w / 2.0
}

fn draw_seg(painter: &Painter, from: Pos2, to: Pos2, color: Color32, top_half: bool) {
    let stroke = Stroke::new(2.0, color);
    if (from.x - to.x).abs() < 0.5 {
        painter.line_segment([from, to], stroke);
        return;
    }
    // Top half: leave the lane vertically, enter the node horizontally.
    // Bottom half: leave the node horizontally, enter the lane vertically.
    let (c1, c2) = if top_half {
        (pos2(from.x, to.y), pos2(from.x, to.y))
    } else {
        (pos2(to.x, from.y), pos2(to.x, from.y))
    };
    painter.add(CubicBezierShape::from_points_stroke(
        [from, c1, c2, to],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn dashed_v(painter: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    let mut y = y0;
    while y < y1 {
        painter.line_segment(
            [pos2(x, y), pos2(x, (y + 3.0).min(y1))],
            Stroke::new(2.0, color),
        );
        y += 6.0;
    }
}

pub fn graph_view(
    ui: &mut Ui,
    tab: &mut RepoTab,
    pal: &Palette,
    opts: GraphOpts,
    acts: &mut Vec<Action>,
) {
    let landing = dnd::take_landing(ui.ctx());
    let has_wip = tab.has_wip_row();
    let RepoTab {
        data,
        graph,
        coloring,
        refs_at,
        selected,
        scroll_to,
        ref_col_w,
        graph_col_w,
        author_col_w,
        date_col_w,
        row_of,
        new_branch,
        find,
        limit,
        loading_more,
        ..
    } = tab;
    let Some(data) = data.as_ref() else {
        ui.centered_and_justified(|ui| ui.spinner());
        return;
    };
    let offset = has_wip as usize;
    let total = data.commits.len() + offset;
    // Follow the interface font size from Settings (13 by default).
    let ui_size = ui.style().text_styles[&egui::TextStyle::Body].size;

    // Keyboard navigation.
    if ui.ctx().memory(|m| m.focused().is_none()) && total > 0 {
        let (up, down) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
            )
        });
        if up || down {
            let cur = match selected {
                Selection::Wip => Some(0),
                Selection::Commit(s) => row_of.get(s.as_str()).map(|r| r + offset),
                Selection::None => None,
            };
            let next = match cur {
                Some(c) if up => c.saturating_sub(1),
                Some(c) => (c + 1).min(total - 1),
                None => 0,
            };
            *scroll_to = Some(next);
            acts.push(Action::Select(if has_wip && next == 0 {
                Selection::Wip
            } else {
                Selection::Commit(data.commits[next - offset].sha.clone())
            }));
        }
    }

    // Find (Ctrl F): rows whose subject or hash match, top to bottom.
    if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::F)) {
        find.get_or_insert_with(Find::default).focus = true;
    }
    let query = find
        .as_ref()
        .map(|f| f.query.trim().to_lowercase())
        .unwrap_or_default();
    let hits: Vec<usize> = if query.is_empty() {
        Vec::new()
    } else {
        (0..data.commits.len())
            .filter(|&i| commit_matches(&data.commits[i], &query))
            .collect()
    };

    // Header: captions over a hairline.
    let full = ui.available_rect_before_wrap();
    if find.is_some() {
        let sel_row = match &*selected {
            Selection::Commit(s) => row_of.get(s.as_str()).copied(),
            _ => None,
        };
        if find_bar(ui, full, find, &hits, sel_row, data, row_of, pal, acts) {
            *find = None;
        }
    }
    let widths = ColWidths {
        refs: *ref_col_w,
        graph: *graph_col_w,
        author: *author_col_w,
        date: *date_col_w,
        show_author: opts.author_column,
        lane_w: if opts.author_on_node {
            AVATAR_LANE_W
        } else {
            LANE_W
        },
    };
    let cols = columns(full.left(), full.width(), widths, graph.lanes);
    let (header, _) = ui.allocate_exact_size(vec2(full.width(), 30.0), Sense::hover());
    {
        let p = ui.painter();
        p.hline(
            header.x_range(),
            header.bottom() - 0.5,
            Stroke::new(1.0, pal.border),
        );
        let y = header.center().y;
        let cap = |p: &Painter, x: f32, t: &str, right: bool| {
            let g = egui::WidgetText::from(ds::caption(t, pal)).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            );
            let x = if right { x - g.size().x } else { x };
            p.galley(pos2(x, y - g.size().y / 2.0), g, pal.text_subtle);
        };
        cap(p, cols.refs.0, "Branch / tag", false);
        cap(p, cols.graph.0, "Graph", false);
        cap(p, cols.msg.0, "Commit message", false);
        if cols.author.1 > cols.author.0 {
            cap(p, cols.author.0, "Author", false);
        }
        cap(p, cols.date.1, "Date", true);
    }
    // Handles between columns: drag to resize; double-click fits the column on the left to its
    // contents, like Excel (the message is the exception: it takes what the author leaves).
    let handle = |x: f32, id: &str| -> (f32, bool) {
        let rect = Rect::from_center_size(pos2(x, header.center().y), vec2(8.0, header.height()));
        let r = ui
            .interact(rect, ui.scope_id().with(id), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to resize, double-click to fit");
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, id));
        if r.hovered() || r.dragged() {
            ui.painter().vline(
                rect.center().x,
                header.y_range().shrink(6.0),
                Stroke::new(1.0, pal.border_strong),
            );
        }
        (
            if r.dragged() { r.drag_delta().x } else { 0.0 },
            r.double_clicked(),
        )
    };
    let gap = 12.0;
    let (dx, fit) = handle(cols.refs.1 + 6.0, "Resize branch column");
    *ref_col_w = if fit {
        fit_refs(ui.painter(), data, refs_at, pal)
    } else {
        *ref_col_w + dx
    }
    .clamp(80.0, 500.0);
    let (dx, fit) = handle(cols.graph.1 + 6.0, "Resize graph column");
    if fit {
        *graph_col_w = None;
    } else if dx != 0.0 {
        *graph_col_w = Some((cols.graph.1 - cols.graph.0 + dx).clamp(40.0, 600.0));
    }
    if cols.author.1 > cols.author.0 {
        // Dragging the message's right edge to the right makes the author column narrower.
        let (dx, fit) = handle(cols.author.0 - 6.0, "Resize message column");
        if fit {
            let longest = data
                .commits
                .iter()
                .map(|c| {
                    // As drawn: the ticket key a little bolder than the rest.
                    let key = ticket_key_len(&c.subject);
                    let mut job = ds::text_job(&c.subject[..key], ui_size, 550.0, pal.text, false);
                    let rest = ds::text_job(&c.subject[key..], ui_size, 400.0, pal.text, false);
                    job.append(&c.subject[key..], 0.0, rest.sections[0].format.clone());
                    ui.painter().layout_job(job).size().x
                })
                .fold(0.0, f32::max);
            *author_col_w = (cols.author.1 - (cols.msg.0 + longest + 8.0 + gap)).clamp(60.0, 400.0);
        } else if dx != 0.0 {
            *author_col_w = (cols.author.1 - cols.author.0 - dx).clamp(60.0, 400.0);
        }
        // Between author and date only those two change, so the message stays put.
        let (dx, fit) = handle(cols.date.0 - 6.0, "Resize author column");
        if fit {
            let longest = data
                .commits
                .iter()
                .map(|c| {
                    let g = ds::galley(ui.painter(), &c.author, ui_size, 400.0, pal.text, 1e6);
                    g.size().x
                })
                .fold(0.0, f32::max);
            *author_col_w = (longest + 24.0 + 6.0).clamp(60.0, 400.0);
        } else if dx != 0.0 {
            // Start from the drawn widths, which may differ from the stored ones on narrow tabs.
            let (author_now, date_now) = (cols.author.1 - cols.author.0, cols.date.1 - cols.date.0);
            let date = (date_now - dx).clamp(60.0, 240.0);
            *author_col_w = (author_now + (date_now - date)).clamp(60.0, 400.0);
            *date_col_w = date;
        }
    } else {
        // Narrow lists have no author column: this border sits between message and date.
        let (dx, fit) = handle(cols.date.0 - 6.0, "Resize date column");
        *date_col_w = if fit {
            data.commits
                .iter()
                .take(500)
                .map(|c| {
                    ds::galley(
                        ui.painter(),
                        &fmt_time(c.time),
                        ui_size,
                        400.0,
                        pal.text,
                        1e6,
                    )
                    .size()
                    .x
                })
                .fold(60.0, f32::max)
                + 4.0
        } else {
            *date_col_w - dx
        }
        .clamp(60.0, 240.0);
    }

    if total == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("No commits yet").color(pal.text_muted))
        });
        return;
    }

    // The commit went away (reset, rebase): so does the name field.
    if new_branch
        .as_ref()
        .is_some_and(|b| !row_of.contains_key(&b.at))
    {
        *new_branch = None;
    }
    let head_row = data
        .head_sha
        .as_ref()
        .and_then(|s| row_of.get(s.as_str()).copied());
    let mut area = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("graph");
    if let Some(target) = scroll_to.take() {
        let view_h = ui.available_height();
        let y = target as f32 * ROW_H;
        let current = ui
            .ctx()
            .data(|d| d.get_temp::<f32>(ui.scope_id().with("graph-offset")))
            .unwrap_or(0.0);
        if y < current || y + ROW_H > current + view_h {
            area = area.vertical_scroll_offset((y - view_h / 3.0).max(0.0));
        }
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    // Near the end of what is loaded, while there may be more: load the next portion.
    let more = data.commits.len() >= limit.load(std::sync::atomic::Ordering::Relaxed);
    let out = area.show_rows(ui, ROW_H, total, |ui, range| {
        if more && !*loading_more && range.end + 20 >= total {
            *loading_more = true;
            acts.push(Action::LoadMore);
        }
        for row in range {
            let is_wip = has_wip && row == 0;
            // Commits drag onto a branch to cherry-pick them there.
            let sense = if is_wip {
                Sense::click()
            } else {
                Sense::click_and_drag()
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
            let painter = ui.painter_at(rect);
            let yc = rect.center().y;
            let is_sel = if is_wip {
                *selected == Selection::Wip
            } else {
                matches!(selected, Selection::Commit(s) if *s == data.commits[row - offset].sha)
            };
            if is_sel {
                painter.rect_filled(rect, 0.0, pal.selected);
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, pal.hover);
            }
            // Find matches: a faint tint and an accent bar at the left edge.
            if !is_wip && hits.binary_search(&(row - offset)).is_ok() {
                if !is_sel && !resp.hovered() {
                    painter.rect_filled(rect, 0.0, pal.accent_soft.gamma_multiply(0.5));
                }
                painter.rect_filled(
                    Rect::from_x_y_ranges(rect.left()..=rect.left() + 3.0, rect.y_range()),
                    0.0,
                    pal.accent,
                );
            }

            if is_wip {
                // Clipped to the graph column like the commit rows.
                let gpainter = painter.with_clip_rect(Rect::from_x_y_ranges(
                    cols.graph.0..=cols.graph.1,
                    rect.y_range(),
                ));
                let col = head_row.map(|r| graph.rows[r].col).unwrap_or(0);
                let x = lane_x(&cols, col);
                let color = head_row
                    .map(|r| coloring.color(graph.rows[r].color, pal))
                    .unwrap_or(pal.text_subtle);
                if head_row.is_some() {
                    dashed_v(&gpainter, x, yc + NODE_R, rect.bottom(), color);
                }
                gpainter.circle_filled(pos2(x, yc), NODE_R, pal.canvas);
                gpainter.add(egui::Shape::dashed_line(
                    &(0..=24)
                        .map(|k| {
                            let a = k as f32 / 24.0 * std::f32::consts::TAU;
                            pos2(x + NODE_R * a.cos(), yc + NODE_R * a.sin())
                        })
                        .collect::<Vec<_>>(),
                    Stroke::new(2.0, color),
                    3.0,
                    2.0,
                ));
                if graph_fit(graph.lanes, cols.lane_w) > cols.graph.1 - cols.graph.0 + 0.5 {
                    fade_right(
                        &painter,
                        cols.graph.1,
                        rect.y_range(),
                        row_background(pal, is_sel, resp.hovered()),
                    );
                }
                let st = &data.status;
                let files = |n: usize| {
                    if n == 1 {
                        "1 changed file".to_string()
                    } else {
                        format!("{n} changed files")
                    }
                };
                let label = match st.conflicted.len() {
                    0 => format!("// WIP · {}", files(st.count())),
                    c => format!(
                        "// WIP · {c} conflict{}, {}",
                        if c == 1 { "" } else { "s" },
                        files(st.count() - c)
                    ),
                };
                let mut job = ds::text_job(&label, ui_size, 400.0, pal.text_muted, false);
                job.sections[0].format.italics = true;
                let g = painter.layout_job(job);
                painter.galley(pos2(cols.msg.0, yc - g.size().y / 2.0), g, pal.text_muted);
                resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, "WIP")
                });
                if resp.clicked() {
                    acts.push(Action::Select(Selection::Wip));
                }
                continue;
            }

            let idx = row - offset;
            let c = &data.commits[idx];
            let g = &graph.rows[idx];
            let color = coloring.color(g.color, pal);

            // Graph lines.
            let gpainter = painter.with_clip_rect(Rect::from_x_y_ranges(
                cols.graph.0..=cols.graph.1,
                rect.y_range(),
            ));
            for s in &g.top {
                draw_seg(
                    &gpainter,
                    pos2(lane_x(&cols, s.from), rect.top()),
                    pos2(lane_x(&cols, s.to), yc),
                    coloring.color(s.color, pal),
                    true,
                );
            }
            for s in &g.bottom {
                draw_seg(
                    &gpainter,
                    pos2(lane_x(&cols, s.from), yc),
                    pos2(lane_x(&cols, s.to), rect.bottom()),
                    coloring.color(s.color, pal),
                    false,
                );
            }
            // Dashed line from the WIP row down to the checked out commit.
            if let (true, Some(hr)) = (has_wip, head_row)
                && idx <= hr
            {
                let hx = lane_x(&cols, graph.rows[hr].col);
                let hc = coloring.color(graph.rows[hr].color, pal);
                dashed_v(
                    &gpainter,
                    hx,
                    rect.top(),
                    if idx == hr {
                        let r = if opts.author_on_node && c.parents.len() < 2 {
                            AVATAR_R
                        } else {
                            NODE_R
                        };
                        yc - r - 4.0
                    } else {
                        rect.bottom()
                    },
                    hc,
                );
            }

            // Ref chips, left-aligned in their column: up to two, else one and "+N". A new
            // branch's name field takes their place.
            let node = pos2(lane_x(&cols, g.col), yc);
            let merge = c.parents.len() > 1;
            let avatar = opts.author_on_node && !merge;
            let node_r = if avatar { AVATAR_R } else { NODE_R };
            let naming = new_branch.as_ref().is_some_and(|b| b.at == c.sha);
            if naming {
                let field =
                    Rect::from_x_y_ranges(cols.refs.0 + 4.0..=cols.refs.1, yc - 13.0..=yc + 13.0);
                if branch_name_field(ui, field, new_branch, data, pal, acts) {
                    *new_branch = None;
                }
                painter.line_segment(
                    [
                        pos2(cols.refs.1 + 2.0, yc),
                        pos2((node.x - node_r - 3.0).min(cols.graph.1), yc),
                    ],
                    Stroke::new(1.0, color.gamma_multiply(0.6)),
                );
            } else if let Some(idxs) = refs_at.get(&c.sha) {
                let mut pills = pills_for(idxs, data);
                // Two chips: the checked out branch on the right, next to its dot, where it
                // keeps its full name.
                if pills.len() == 2 && pills[0].kind.head {
                    pills.swap(0, 1);
                }
                let shown = if pills.len() > 2 { 1 } else { pills.len() };
                let more = pills.len() - shown;
                // Width of "+N" and its gap, kept free at the right.
                let more_w = if more > 0 {
                    painter
                        .layout_job(ds::text_job(
                            &format!("+{more}"),
                            11.0,
                            600.0,
                            pal.text_muted,
                            true,
                        ))
                        .size()
                        .x
                        + 14.0
                } else {
                    0.0
                };
                // Sized from the right so the last chip gets the room, then painted from the
                // column's left edge.
                let mut budget = cols.refs.1 - more_w;
                let mut chips: Vec<_> = pills[..shown]
                    .iter()
                    .rev()
                    .map(|pill| {
                        let avail = (budget - cols.refs.0).max(40.0);
                        let (gal, w) = ds::chip_layout_prefixed(
                            &painter,
                            pill.prefix.as_deref(),
                            &pill.text,
                            pill.kind,
                            pal,
                            avail,
                        );
                        budget -= w + 4.0;
                        (pill, gal, w)
                    })
                    .collect();
                chips.reverse();
                let mut x = cols.refs.0;
                for (pill, gal, w) in chips {
                    let lane = pill_lane(pill, data, coloring, pal, color);
                    let pr = ds::paint_chip(&painter, pos2(x, yc), gal, w, pill.kind, lane, pal);
                    x += w + 4.0;
                    let presp = ui.interact(
                        pr,
                        ui.scope_id()
                            .with(("pill", &c.sha, &pill.text, &pill.prefix)),
                        Sense::click_and_drag(),
                    );
                    presp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, pill.full_names(data))
                    });
                    let tip: Vec<String> = pill
                        .refs
                        .iter()
                        .map(|&i| data.refs[i].name.clone())
                        .collect();
                    let note = if pill.kind.default {
                        "\ndefault branch of the remote"
                    } else {
                        ""
                    };
                    let presp = presp.on_hover_text(format!(
                        "{}{note}\nRight-click for actions",
                        tip.join("\n")
                    ));
                    if presp.double_clicked() {
                        acts.push(checkout_action(&data.refs[pill.refs[0]], data));
                    } else if presp.clicked() {
                        acts.push(Action::Select(Selection::Commit(c.sha.clone())));
                    }
                    presp.context_menu(|ui| pill_menu(ui, pal, pill, data, lane, acts));
                    let first = &data.refs[pill.refs[0]];
                    if first.kind != RefKind::Tag {
                        dnd::source(&presp, || dnd::Drag::Branch {
                            name: first.name.clone(),
                            spec: data.spec(first),
                            target: first.target.clone(),
                        });
                    }
                    dnd::target(ui, &presp, first, data, pal, acts);
                }
                if more > 0 {
                    let r = ds::paint_more(&painter, pos2(x, yc), more, pal);
                    x = r.right() + 4.0;
                    let names: Vec<String> =
                        pills[shown..].iter().map(|p| p.full_names(data)).collect();
                    let mresp = ui
                        .interact(r, ui.scope_id().with(("more", &c.sha)), Sense::click())
                        .on_hover_text(names.join("\n"));
                    mresp.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, true, format!("+{more}"))
                    });
                    mresp.context_menu(|ui| {
                        ui.set_width(300.0);
                        for p in &pills[shown..] {
                            let lane = pill_lane(p, data, coloring, pal, color);
                            ds::submenu(ui, pal, icons::GIT_BRANCH, &p.text, |ui| {
                                pill_menu(ui, pal, p, data, lane, acts)
                            });
                        }
                    });
                }
                // A thin connector from the chips to the dot, in the lane colour.
                painter.line_segment(
                    [
                        pos2(x - 2.0, yc),
                        pos2((node.x - node_r - 3.0).min(cols.graph.1), yc),
                    ],
                    Stroke::new(1.0, color.gamma_multiply(0.6)),
                );
            }

            // Node: filled dot (or the author's initials), hollow for merges, an extra ring for
            // HEAD. Clipped to the graph column, which the user can make narrower than the lanes.
            let is_head = data.head_sha.as_deref() == Some(c.sha.as_str());
            if merge {
                gpainter.circle_filled(node, NODE_R - 0.5, pal.canvas);
                gpainter.circle_stroke(node, NODE_R - 0.5, Stroke::new(2.0, color));
            } else if avatar {
                ds::avatar(&gpainter, node, &initials(&c.author), color);
            } else {
                gpainter.circle_filled(node, NODE_R, color);
            }
            if is_head {
                gpainter.circle_stroke(node, node_r + 3.5, Stroke::new(1.5, pal.text));
            }
            // Dragging over a branch: where the new commit would land, on top of this one.
            if landing.as_deref() == Some(c.sha.as_str()) {
                let lp = ui.painter();
                let spot = pos2(node.x, rect.top());
                let accent = Stroke::new(2.0, pal.accent);
                lp.line_segment([spot, pos2(cols.msg.1, spot.y)], accent);
                lp.line_segment([spot, node - vec2(0.0, node_r)], accent);
                lp.circle_filled(spot, NODE_R, pal.canvas);
                lp.circle_stroke(spot, NODE_R, accent);
            }
            // More lanes than fit: fade them out at the column's right edge.
            if graph_fit(graph.lanes, cols.lane_w) > cols.graph.1 - cols.graph.0 + 0.5 {
                fade_right(
                    &painter,
                    cols.graph.1,
                    rect.y_range(),
                    row_background(pal, is_sel, resp.hovered()),
                );
            }

            // Message: ticket key in 550; merges in `text-muted`; the body after it, subtle.
            let subject_color = if merge { pal.text_muted } else { pal.text };
            let key = if merge { 0 } else { ticket_key_len(&c.subject) };
            let mut job = LayoutJob::default();
            for (part, weight) in [(&c.subject[..key], 550.0), (&c.subject[key..], 400.0)] {
                if !part.is_empty() {
                    let s = ds::text_job(part, ui_size, weight, subject_color, false);
                    job.append(part, 0.0, s.sections[0].format.clone());
                }
            }
            if !c.body.is_empty() {
                let s = ds::text_job(&c.body, ui_size, 400.0, pal.text_subtle, false);
                job.append(&c.body, 10.0, s.sections[0].format.clone());
            }
            job.wrap.max_width = (cols.msg.1 - cols.msg.0).max(1.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let msg = painter.layout_job(job);
            painter.galley(
                pos2(cols.msg.0, yc - msg.size().y / 2.0),
                msg,
                subject_color,
            );

            if cols.author.1 > cols.author.0 {
                ds::avatar(
                    &painter,
                    pos2(cols.author.0 + 9.0, yc),
                    &initials(&c.author),
                    color,
                );
                let a = ds::galley(
                    &painter,
                    &c.author,
                    ui_size,
                    400.0,
                    pal.text_muted,
                    cols.author.1 - cols.author.0 - 24.0,
                );
                painter.galley(
                    pos2(cols.author.0 + 24.0, yc - a.size().y / 2.0),
                    a,
                    pal.text_muted,
                );
            }
            // Elided to the column, which the user can make narrow.
            let date = ds::galley(
                &painter,
                &fmt_time(c.time),
                ui_size,
                400.0,
                pal.text_muted,
                cols.date.1 - cols.date.0,
            );
            painter.galley(
                pos2(cols.date.1 - date.size().x, yc - date.size().y / 2.0),
                date,
                pal.text_muted,
            );

            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::Role::Button, true, is_sel, &c.subject)
            });
            if resp.clicked() {
                acts.push(Action::Select(Selection::Commit(c.sha.clone())));
            }
            resp.context_menu(|ui| commit_menu(ui, pal, &c.sha, &c.subject, data, acts));
            dnd::source(&resp, || dnd::Drag::Commit {
                sha: c.sha.clone(),
                subject: c.subject.clone(),
                merge: c.parents.len() > 1,
            });
        }
    });
    let off = out.state.clamped_offset().y;
    ui.ctx()
        .data_mut(|d| d.insert_temp(ui.scope_id().with("graph-offset"), off));
}

/// The name field for a new branch. Enter with a usable name creates the branch and checks it
/// out; Escape, Enter on an empty field or clicking elsewhere drops it. Returns true when the
/// field is done.
fn branch_name_field(
    ui: &mut Ui,
    rect: Rect,
    state: &mut Option<crate::app::NewBranch>,
    data: &RepoData,
    pal: &Palette,
    acts: &mut Vec<Action>,
) -> bool {
    let Some(b) = state.as_mut() else { return true };
    let name = b.name.trim().to_string();
    let problem = crate::app::new_ref_problem(Some(data), &name);
    let shown = problem.filter(|_| !name.is_empty());
    let resp = ui
        .scope(|ui| {
            if shown.is_some() {
                let w = &mut ui.visuals_mut().widgets;
                for s in [&mut w.inactive, &mut w.hovered, &mut w.active] {
                    s.bg_stroke = Stroke::new(1.0, pal.danger);
                }
                ui.visuals_mut().selection.stroke = Stroke::new(1.0, pal.danger);
            }
            ui.put(
                rect,
                egui::TextEdit::singleline(&mut b.name)
                    // A fixed id, so scrolling (which renumbers the rows) keeps the focus.
                    .id(egui::Id::unique(("new-branch", &b.at)))
                    .hint_text("enter branch name")
                    .margin(egui::Margin::symmetric(8, 4))
                    .align(egui::Align2::LEFT_CENTER),
            )
        })
        .inner;
    let resp = match shown {
        Some(p) => resp.on_hover_text(p),
        None => resp.on_hover_text("Enter creates the branch and checks it out, Escape cancels"),
    };
    if !b.focused {
        resp.request_focus();
        b.focused = true;
        return false;
    }
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    if resp.lost_focus() && enter {
        if problem.is_some() {
            // Keep the field open until the name works (an empty name stays allowed to cancel).
            if name.is_empty() {
                return true;
            }
            resp.request_focus();
            return false;
        }
        acts.push(Action::Op(Op::CreateBranch {
            name,
            at: b.at.clone(),
            checkout: true,
        }));
        return true;
    }
    !resp.has_focus()
}

/// A branch has its own colour; a tag follows the branch its commit is on.
pub(super) fn pill_lane(
    pill: &Pill,
    data: &RepoData,
    coloring: &crate::colors::Coloring,
    pal: &Palette,
    commit_color: Color32,
) -> Color32 {
    let first = &data.refs[pill.refs[0]];
    match first.kind {
        RefKind::Tag => commit_color,
        _ => coloring
            .branch(first.branch_name(), pal)
            .unwrap_or(commit_color),
    }
}

pub(super) fn pill_menu(
    ui: &mut Ui,
    pal: &Palette,
    pill: &Pill,
    data: &RepoData,
    lane: Color32,
    acts: &mut Vec<Action>,
) {
    let first = &data.refs[pill.refs[0]];
    ref_menu(ui, pal, first, data, lane, acts);
    for &i in &pill.refs[1..] {
        let r = &data.refs[i];
        ds::menu_divider(ui, pal);
        ds::submenu(ui, pal, icons::CLOUD, &r.name, |ui| {
            ref_menu(ui, pal, r, data, lane, acts)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_and_date_widths_are_used_but_the_message_keeps_room() {
        let w = ColWidths {
            refs: 200.0,
            graph: None,
            author: 220.0,
            date: 120.0,
            show_author: true,
            lane_w: LANE_W,
        };
        let c = columns(0.0, 1400.0, w, 3);
        assert_eq!(c.author.1 - c.author.0, 220.0);
        assert_eq!(c.date.1 - c.date.0, 120.0);
        assert!(c.msg.1 - c.msg.0 >= 100.0);
        // A huge author column gives way to the message.
        let c = columns(0.0, 900.0, ColWidths { author: 400.0, ..w }, 3);
        assert!(c.msg.1 - c.msg.0 >= 100.0, "{:?}", c.msg);
        // A narrower graph than the lanes need is allowed; None follows the lanes.
        let c = columns(
            0.0,
            1400.0,
            ColWidths {
                graph: Some(50.0),
                ..w
            },
            9,
        );
        assert_eq!(c.graph.1 - c.graph.0, 50.0);
        assert_eq!(
            columns(0.0, 1400.0, w, 2).graph.1 - columns(0.0, 1400.0, w, 2).graph.0,
            graph_fit(2, LANE_W)
        );
        // Narrow windows hide the author column.
        let narrow = columns(0.0, 800.0, w, 3);
        assert_eq!(narrow.author.1 - narrow.author.0, 0.0);
        // Turned off in Settings: no author column, the message gets the room.
        let off = columns(
            0.0,
            1400.0,
            ColWidths {
                show_author: false,
                ..w
            },
            3,
        );
        assert_eq!(off.author.1 - off.author.0, 0.0);
        assert_eq!(off.msg.1, off.date.0 - 12.0);
        // Initials on the dots: wider lanes.
        assert!(graph_fit(3, AVATAR_LANE_W) > graph_fit(3, LANE_W));
    }
}
