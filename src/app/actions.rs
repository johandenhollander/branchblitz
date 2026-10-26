//! Applying the actions the UI collected during a frame.

use super::*;

impl BranchBlitz {
    /// Shows a modal that was not directly asked for (a failed background operation),
    /// without throwing away a dialog the user is typing in. The status bar still shows it.
    pub(super) fn offer_modal(&mut self, modal: Modal) {
        if self.modal.is_none() {
            self.modal = Some(modal);
        }
    }

    pub(crate) fn apply(&mut self, i: usize, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Op(op) => self.submit(i, op, false),
                Action::Confirm {
                    title,
                    body,
                    op,
                    danger,
                } => {
                    self.tabs[i].alert = Some(Alert {
                        title,
                        body,
                        op,
                        danger,
                    })
                }
                Action::Select(sel) => {
                    let tab = &mut self.tabs[i];
                    if tab.selected == sel {
                        continue;
                    }
                    tab.selected = sel.clone();
                    tab.details = None;
                    tab.details_error = None;
                    if let Selection::Commit(sha) = sel {
                        self.request_details(i, sha);
                    }
                }
                Action::ShowDiff(target) => {
                    self.tabs[i].diff_selection.clear();
                    self.tabs[i].diff_anchor = None;
                    self.tabs[i].conflict_choices.clear();
                    self.tabs[i].diff = Some((target.clone(), None));
                    self.request_diff(i, target);
                }
                Action::CloseDiff => self.tabs[i].diff = None,
                Action::FileView { kind, path, rev } => {
                    let tab = &mut self.tabs[i];
                    tab.diff = None;
                    tab.file_gen += 1;
                    let generation = tab.file_gen;
                    tab.file_view = Some(crate::file_view::FileView {
                        kind,
                        path: path.clone(),
                        rev: rev.clone(),
                        content: None,
                        generation,
                    });
                    let (tx, ctx, repo, id) =
                        (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
                    std::thread::spawn(move || {
                        let r = crate::file_view::load(&repo, kind, &path, rev.as_deref());
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::FileView(generation, r),
                        });
                        ctx.request_repaint();
                    });
                }
                Action::CloseFileView => self.tabs[i].file_view = None,
                Action::Jump(sha) => {
                    let tab = &mut self.tabs[i];
                    if let Some(&row) = tab.row_of.get(&sha) {
                        tab.scroll_to = Some(row + tab.has_wip_row() as usize);
                    }
                    self.apply(i, vec![Action::Select(Selection::Commit(sha))]);
                }
                // On a row of the graph; a commit outside the loaded history gets the dialog.
                Action::Input(InputKind::Branch { at })
                    if self.tabs[i].row_of.contains_key(&at) =>
                {
                    let tab = &mut self.tabs[i];
                    let row = tab.row_of[&at];
                    tab.scroll_to = Some(row + tab.has_wip_row() as usize);
                    tab.new_branch = Some(NewBranch {
                        at,
                        name: String::new(),
                        focused: false,
                    });
                }
                Action::Input(kind) => {
                    let value = match &kind {
                        InputKind::Rename { old } => old.clone(),
                        _ => String::new(),
                    };
                    self.modal = Some(Modal::Input {
                        tab: self.tabs[i].id,
                        kind,
                        value,
                    });
                }
                Action::FlowInit => {
                    let tab = &self.tabs[i];
                    let flow = tab
                        .data
                        .as_ref()
                        .map(|d| d.flow.clone())
                        .unwrap_or_default();
                    if flow.configured() {
                        self.modal = Some(Modal::FlowInit {
                            tab: tab.id,
                            config: flow.config(),
                            update: true,
                        });
                        continue;
                    }
                    let has =
                        |name: &str| tab.data.as_ref().is_some_and(|d| d.local(name).is_some());
                    // git-flow's own guess: main or master, else the current branch.
                    let master = ["main", "master"]
                        .into_iter()
                        .find(|b| has(b))
                        .map(str::to_string)
                        .or_else(|| tab.data.as_ref().and_then(|d| d.head_branch.clone()))
                        .unwrap_or_default();
                    self.modal = Some(Modal::FlowInit {
                        tab: tab.id,
                        config: git::FlowConfig {
                            master,
                            ..flow.config()
                        },
                        update: false,
                    });
                }
                Action::Copy(s) => self.ctx.copy_text(s),
                Action::SwitchWorktree(path) => self.switch_worktree(i, &path),
                Action::OpenRepo(path) => self.open_repo(&path, true),
                Action::LoadMore => self.load_more(i),
                Action::LoadUntil(sha) => {
                    self.tabs[i].load_until = Some(sha);
                    self.load_more(i);
                }
                Action::FindHistory(query) => {
                    let tab = &mut self.tabs[i];
                    if let Some(f) = &mut tab.find {
                        f.searching += 1;
                    }
                    let (tx, ctx, path, id) =
                        (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
                    std::thread::spawn(move || {
                        let r = git::search_commits(&path, &query, FIND_HISTORY_MAX);
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::FoundInHistory(query, r),
                        });
                        ctx.request_repaint();
                    });
                }
                Action::AgentSend => {
                    let (tx, ctx, id) = (self.tx.clone(), self.ctx.clone(), self.tabs[i].id);
                    let attach = self.agent_attach(i);
                    let model = self.settings.agent_model.as_deref();
                    let extra = self.settings.agent_prompt();
                    let tab = &mut self.tabs[i];
                    tab.agent.send(
                        &self.agent_backend,
                        &tab.path,
                        attach,
                        model,
                        extra,
                        move |event| {
                            let _ = tx.send(Msg {
                                tab: id,
                                kind: MsgKind::Agent(event),
                            });
                            ctx.request_repaint();
                        },
                    );
                }
                Action::AgentStop => self.tabs[i].agent.stop(),
                Action::AgentClear => self.tabs[i].agent.clear(),
                Action::AgentModel(m) => self.settings.agent_model = m,
                Action::AgentAccess(a) => self.settings.agent_access = a,
                Action::AgentOpen(open) => {
                    self.settings.agent_open = open;
                    if !open {
                        self.agent_max = false;
                    }
                }
                Action::AgentMaximize(max) => {
                    self.agent_max = max;
                    if max {
                        self.settings.agent_open = true;
                    }
                }
                Action::OpenFile(rel) => {
                    let path = self.tabs[i].path.join(rel);
                    self.open_file(&path);
                }
                Action::Fetch => self.fetch(i, false),
                Action::Amend(true) => {
                    let tab = &self.tabs[i];
                    let Some(sha) = tab.data.as_ref().and_then(|d| d.head_sha.clone()) else {
                        continue;
                    };
                    let (tx, ctx, path, id) =
                        (self.tx.clone(), self.ctx.clone(), tab.path.clone(), tab.id);
                    std::thread::spawn(move || {
                        let r = git::commit_details(&path, &sha).map(|d| d.message);
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::AmendMessage(r),
                        });
                        ctx.request_repaint();
                    });
                }
                Action::Amend(false) => {
                    let tab = &mut self.tabs[i];
                    if let Some((summary, body)) = tab.amend_prefill.take()
                        && tab.commit_msg == summary
                        && tab.commit_body == body
                    {
                        tab.commit_msg.clear();
                        tab.commit_body.clear();
                    }
                }
                Action::SetPullMode(mode) => {
                    let key = self.tabs[i].path.display().to_string();
                    match mode {
                        Some(mode) => {
                            self.settings.fetch_default.remove(&key);
                            self.settings.pull_modes.insert(key, mode);
                        }
                        None => {
                            self.settings.fetch_default.insert(key);
                        }
                    }
                }
            }
        }
    }
}
