//! Results from the worker threads and background jobs, applied to the tabs.

use super::*;

impl BranchBlitz {
    pub(super) fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            if let MsgKind::PickedFolder(p) = &msg.kind {
                let p = p.clone();
                self.open_repo(&p, true);
                continue;
            }
            if let MsgKind::Cloned(r) = &msg.kind {
                self.cloned(r.clone());
                continue;
            }
            if let MsgKind::PickedCloneParent(p) = &msg.kind {
                if let Some(Modal::Clone { parent, .. }) = &mut self.modal {
                    *parent = p.display().to_string();
                }
                continue;
            }
            if let MsgKind::Latest(r) = &msg.kind {
                match r {
                    Ok(v) => {
                        self.settings.latest_version = v.clone();
                        self.settings.updates_checked = chrono::Utc::now().timestamp();
                    }
                    Err(e) => crate::log::write("INFO", &format!("update check: {e}")),
                }
                continue;
            }
            // Agent events follow their conversation into a replaced tab; the rest belongs to
            // the old worker.
            let i = match &msg.kind {
                MsgKind::Agent(_) => self.tab_index(msg.tab),
                _ => self.tabs.iter().position(|t| t.id == msg.tab),
            };
            let Some(i) = i else {
                continue;
            };
            match msg.kind {
                MsgKind::Loaded(generation, Ok(data)) => {
                    let coloring = colors::assign(&data, &self.settings.color_rules);
                    let tab = &mut self.tabs[i];
                    tab.loads_done = tab.loads_done.max(generation);
                    tab.tags_pending = false;
                    tab.graph = graph::build(&data.commits, &coloring.commit);
                    tab.coloring = coloring;
                    tab.refs_at = git::refs_by_commit(&data.refs);
                    tab.row_of = data
                        .commits
                        .iter()
                        .enumerate()
                        .map(|(n, c)| (c.sha.clone(), n))
                        .collect();
                    tab.load_error = None;
                    tab.loading_more = false;
                    let loaded_all = data.commits.len() < tab.limit.load(Ordering::Relaxed);
                    let dirty = data.status.is_dirty() || data.state != RepoState::Clean;
                    tab.data = Some(data);
                    if tab.selected == Selection::Wip && !dirty {
                        tab.selected = Selection::None;
                    }
                    if let Selection::Commit(sha) = &tab.selected
                        && !tab.row_of.contains_key(sha)
                    {
                        tab.selected = Selection::None;
                        tab.details = None;
                    }
                    // A resolved conflict leaves the panel; others follow the working tree.
                    if let Some((DiffTarget::Conflict { path }, _)) = &tab.diff
                        && tab
                            .data
                            .as_ref()
                            .is_some_and(|d| !d.status.conflicted.contains(path))
                    {
                        tab.diff = None;
                        tab.conflict_choices.clear();
                    }
                    let refresh = match &tab.diff {
                        Some((
                            t @ (DiffTarget::Unstaged { .. }
                            | DiffTarget::Staged { .. }
                            | DiffTarget::Conflict { .. }),
                            _,
                        )) => Some(t.clone()),
                        _ => None,
                    };
                    if let Some(t) = refresh {
                        self.request_diff(i, t);
                    }
                    // Loading towards a find match in older history: there yet, or further.
                    if let Some(sha) = self.tabs[i].load_until.clone() {
                        if self.tabs[i].row_of.contains_key(&sha) {
                            self.tabs[i].load_until = None;
                            self.apply(i, vec![Action::Jump(sha)]);
                        } else if loaded_all {
                            self.tabs[i].load_until = None;
                        } else {
                            self.load_more(i);
                        }
                    }
                }
                MsgKind::Loaded(generation, Err(e)) => {
                    crate::log::write(
                        "ERROR",
                        &format!("loading {}: {e}", self.tabs[i].path.display()),
                    );
                    let tab = &mut self.tabs[i];
                    tab.loads_done = tab.loads_done.max(generation);
                    tab.tags_pending = false;
                    tab.load_error = Some(e);
                }
                MsgKind::OpDone { op, quiet, result } => {
                    let tab = &mut self.tabs[i];
                    tab.running.pop_front();
                    tab.tags_pending = result.is_ok() && matches!(op, Op::Fetch | Op::Pull(_));
                    match result {
                        Ok(out) => {
                            if matches!(op, Op::Commit { .. }) {
                                tab.commit_msg.clear();
                                tab.commit_body.clear();
                                tab.amend = false;
                                tab.amend_prefill = None;
                            }
                            let first = out
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            let text = if first.is_empty() {
                                format!("{} done", op.label())
                            } else {
                                format!("{}: {}", op.label(), first)
                            };
                            self.status_line = Some((true, text));
                        }
                        Err(e) => {
                            crate::log::write(
                                "ERROR",
                                &format!(
                                    "{} failed in {}: {e}",
                                    op.label(),
                                    self.tabs[i].path.display()
                                ),
                            );
                            self.status_line = Some((
                                false,
                                format!(
                                    "{} failed: {}",
                                    op.label(),
                                    e.lines().next().unwrap_or("")
                                ),
                            ));
                            if let Op::DeleteBranch { name, force: false } = &op
                                && e.contains("not fully merged")
                            {
                                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == msg.tab)
                                    && t.alert.is_none()
                                {
                                    t.alert = Some(Alert {
                                        title: format!("Force delete {name}?"),
                                        body: format!(
                                            "Branch {name} is not fully merged. Deleting it may lose commits."
                                        ),
                                        op: Op::DeleteBranch {
                                            name: name.clone(),
                                            force: true,
                                        },
                                        danger: true,
                                    });
                                }
                                continue;
                            }
                            // Stopping on conflicts is an outcome, not a failure: the banner and
                            // the conflict view take over.
                            // The worker reloads before it reports, so the status is known; with
                            // nothing conflicted it was another error after all.
                            let first = self.tabs[i]
                                .data
                                .as_ref()
                                .and_then(|d| d.status.conflicted.first().cloned());
                            if e.contains("CONFLICT")
                                && let Some(path) = first
                            {
                                self.status_line =
                                    Some((true, format!("{} stopped on conflicts", op.label())));
                                let tab = &mut self.tabs[i];
                                tab.selected = Selection::Wip;
                                tab.details = None;
                                self.apply(
                                    i,
                                    vec![Action::ShowDiff(DiffTarget::Conflict { path })],
                                );
                                continue;
                            }
                            if !quiet {
                                self.offer_modal(Modal::Error {
                                    title: format!("{} failed", op.label()),
                                    body: e,
                                });
                            }
                        }
                    }
                }
                MsgKind::Details(sha, r) => {
                    let tab = &mut self.tabs[i];
                    if tab.selected == Selection::Commit(sha) {
                        match r {
                            Ok(d) => tab.details = Some(d),
                            Err(e) => tab.details_error = Some(e),
                        }
                    }
                }
                MsgKind::AmendMessage(r) => {
                    let tab = &mut self.tabs[i];
                    match r {
                        // Only fill fields the user has not started on.
                        Ok(msg)
                            if tab.amend
                                && tab.commit_msg.trim().is_empty()
                                && tab.commit_body.trim().is_empty() =>
                        {
                            let (summary, body) = split_message(&msg);
                            tab.commit_msg = summary.clone();
                            tab.commit_body = body.clone();
                            tab.amend_prefill = Some((summary, body));
                        }
                        Ok(_) => {}
                        Err(e) => self.status_line = Some((false, e)),
                    }
                }
                MsgKind::Diff(generation, target, r) => {
                    let tab = &mut self.tabs[i];
                    if let Some((t, content)) = &mut tab.diff
                        && *t == target
                        && generation == tab.diff_gen
                    {
                        // A changed diff makes the chosen line numbers meaningless.
                        if content.as_ref() != Some(&r) {
                            tab.diff_selection.clear();
                            tab.diff_anchor = None;
                        }
                        *content = Some(r);
                    }
                }
                MsgKind::FileView(generation, r) => {
                    if let Some(fv) = &mut self.tabs[i].file_view
                        && fv.generation == generation
                    {
                        fv.content = Some(r);
                    }
                }
                MsgKind::FoundInHistory(query, r) => {
                    let tab = &mut self.tabs[i];
                    if let (Some(f), Ok(found)) = (&mut tab.find, r)
                        && f.query.trim().to_lowercase() == query.to_lowercase()
                    {
                        f.older = Some((query, found));
                    }
                }
                MsgKind::Agent(event) => {
                    if let crate::agent::Event::Error(e) = &event {
                        crate::log::write("AGENT", e);
                    }
                    self.tabs[i].agent.apply(event)
                }
                MsgKind::PickedFolder(_)
                | MsgKind::Latest(_)
                | MsgKind::Cloned(_)
                | MsgKind::PickedCloneParent(_) => unreachable!(),
            }
        }
    }
}
