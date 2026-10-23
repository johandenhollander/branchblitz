//! Hooks for the MCP server (`mcp.rs`) and tests, which drive the app without a window.

use super::*;

impl BranchBlitz {
    #[cfg(test)]
    pub(crate) fn tabs_ref(&self) -> &[RepoTab] {
        &self.tabs
    }

    #[cfg(test)]
    pub(crate) fn tabs_mut(&mut self) -> &mut Vec<RepoTab> {
        &mut self.tabs
    }

    #[cfg(test)]
    pub(crate) fn clear_status(&mut self) {
        self.status_line = None;
    }

    pub(crate) fn set_headless(&mut self) {
        self.headless = true;
    }

    pub(crate) fn set_confirm_danger(&mut self, allow: bool) {
        self.confirm_danger = allow;
    }

    pub(crate) fn set_theme_choice(&mut self, choice: ThemeChoice) {
        self.settings.theme = choice;
    }

    /// Opens (or focuses) a repository; the error is returned instead of shown in a dialog.
    pub(crate) fn open_for_automation(&mut self, path: &Path) -> Result<(), String> {
        let before = self.modal.is_some();
        self.open_repo(path, true);
        if !before && let Some(Modal::Error { body, .. }) = &self.modal {
            let e = body.clone();
            self.modal = None;
            return Err(e);
        }
        Ok(())
    }

    /// Whether background work is still pending: git jobs, loads, commit details or a diff.
    pub(crate) fn is_busy(&self) -> bool {
        matches!(self.modal, Some(Modal::Clone { busy: true, .. }))
            || self.tabs.iter().any(|t| {
                !t.running.is_empty()
                || t.loads_done < t.loads_requested
                || t.tags_pending
                || (matches!(t.selected, Selection::Commit(_))
                    && t.details.is_none()
                    && t.details_error.is_none())
                || matches!(t.diff, Some((_, None)))
                || t.file_view.as_ref().is_some_and(|f| f.content.is_none())
                || t.find.as_ref().is_some_and(|f| f.search_at.is_some() || f.searching > 0)
                // An agent answering. (The agent's own tool calls wait with `tab_busy` instead.)
                || t.agent.busy()
            })
    }

    /// One-line summary for tool results.
    pub(crate) fn summary(&self) -> String {
        let tab = self.active.map(|i| &self.tabs[i]);
        let mut parts = vec![match tab {
            Some(t) => format!(
                "repo {} on {}",
                t.name,
                t.data
                    .as_ref()
                    .and_then(|d| d.head_branch.clone())
                    .unwrap_or_else(|| "(detached)".into())
            ),
            None => "no repo open".into(),
        }];
        if let Some(t) = tab
            && !t.running.is_empty()
        {
            parts.push(format!(
                "running: {}",
                Vec::from(t.running.clone()).join(", ")
            ));
        }
        if let Some(a) = tab.and_then(|t| t.alert.as_ref()) {
            parts.push(format!("alert: {}", a.title));
        }
        if let Some(m) = &self.modal {
            parts.push(format!("dialog: {}", modal_title(m)));
        }
        if let Some((ok, s)) = &self.status_line {
            parts.push(format!("status{}: {s}", if *ok { "" } else { " (error)" }));
        }
        parts.join(" | ")
    }

    /// Machine-readable view of what the user would see, for the `state` tool.
    pub(crate) fn snapshot(&self, max_commits: usize) -> serde_json::Value {
        self.snapshot_of(max_commits, self.active)
    }

    /// Like `snapshot`, with the details of tab `detail` (usually the active one).
    pub(super) fn snapshot_of(
        &self,
        max_commits: usize,
        detail: Option<usize>,
    ) -> serde_json::Value {
        use serde_json::json;
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(n, t)| {
                let mut v = json!({
                    "name": t.name,
                    "path": t.path,
                    "active": self.active == Some(n),
                    "running": t.running,
                    "load_error": t.load_error,
                });
                if detail != Some(n) {
                    return v;
                }
                v["selected"] = match &t.selected {
                    Selection::None => json!(null),
                    Selection::Wip => json!("WIP"),
                    Selection::Commit(sha) => json!(sha),
                };
                v["columns"] = json!({
                    "branch": t.ref_col_w,
                    "graph": t.graph_col_w,
                    "author": t.author_col_w,
                    "date": t.date_col_w,
                });
                v["alert"] = json!(
                    t.alert
                        .as_ref()
                        .map(|a| json!({"title": a.title, "body": a.body, "danger": a.danger}))
                );
                v["new_branch"] = json!(
                    t.new_branch
                        .as_ref()
                        .map(|b| json!({"at": b.at, "name": b.name}))
                );
                v["diff_open"] = json!(t.diff.as_ref().map(|(d, _)| format!("{d:?}")));
                v["file_view"] = json!(t.file_view.as_ref().map(|f| f.summary()));
                let Some(d) = &t.data else { return v };
                let refs = |kind: git::RefKind| -> Vec<serde_json::Value> {
                    d.refs
                        .iter()
                        .filter(|r| r.kind == kind)
                        .map(|r| {
                            let mut e = json!({"name": r.name, "commit": git::short(&r.target)});
                            if r.kind == git::RefKind::Tag {
                                // null = not known yet (no fetch since start).
                                e["on_remote"] = json!(d.tag_on_remote(r));
                            }
                            if let Some(u) = &r.upstream {
                                e["upstream"] = json!(u);
                                e["ahead"] = json!(r.ahead);
                                e["behind"] = json!(r.behind);
                                if r.upstream_gone {
                                    e["upstream_gone"] = json!(true);
                                }
                            }
                            e
                        })
                        .collect()
                };
                let files = |fs: &[git::FileChange]| -> Vec<String> {
                    fs.iter()
                        .map(|f| format!("{} {}", f.status, f.path))
                        .collect()
                };
                v["head_branch"] = json!(d.head_branch);
                v["head"] = json!(d.head_sha.as_deref().map(git::short));
                v["repo_state"] = json!(format!("{:?}", d.state));
                v["local_branches"] = json!(refs(git::RefKind::Local));
                v["remote_branches"] = json!(refs(git::RefKind::Remote));
                v["tags"] = json!(refs(git::RefKind::Tag));
                v["stashes"] = json!(d.stashes.iter().map(|s| &s.message).collect::<Vec<_>>());
                v["staged"] = json!(files(&d.status.staged));
                v["unstaged"] = json!(files(&d.status.unstaged));
                v["conflicted"] = json!(d.status.conflicted);
                v["commits_loaded"] = json!(d.commits.len());
                v["commits"] = json!(
                    d.commits
                        .iter()
                        .take(max_commits)
                        .map(|c| {
                            let labels: Vec<&str> = t
                                .refs_at
                                .get(&c.sha)
                                .map(|ix| ix.iter().map(|&k| d.refs[k].name.as_str()).collect())
                                .unwrap_or_default();
                            json!({
                                "sha": git::short(&c.sha),
                                "subject": c.subject,
                                "author": c.author,
                                "parents": c.parents.len(),
                                "refs": labels,
                            })
                        })
                        .collect::<Vec<_>>()
                );
                v
            })
            .collect();
        json!({
            "tabs": tabs,
            "dialog": self.modal.as_ref().map(|m| {
                let mut v = json!({"title": modal_title(m)});
                match m {
                    Modal::Error { body, .. } => v["body"] = json!(body),
                    Modal::Input { value, .. } => v["value"] = json!(value),
                    Modal::Notice { body, .. } => v["body"] = json!(body),
                    Modal::Clone { error, busy, .. } => {
                        v["busy"] = json!(busy);
                        v["error"] = json!(error);
                    }
                    Modal::Settings => {}
                }
                v
            }),
            "status_line": self.status_line.as_ref().map(|(ok, s)| json!({"ok": ok, "text": s})),
            "theme": format!("{:?}", self.settings.theme),
            "theme_name": self.settings.theme_name,
            "busy": self.is_busy(),
        })
    }
}
