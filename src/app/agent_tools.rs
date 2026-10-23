//! The agent's tools: what the agent panel's CLI can see and do in the app (via `live`).

use super::*;

/// The tab is doing something the agent should wait for (its own answer excluded).
fn tab_busy(t: &RepoTab) -> bool {
    !t.running.is_empty()
        || t.loads_done < t.loads_requested
        || t.tags_pending
        || matches!(t.diff, Some((_, None)))
}

impl BranchBlitz {
    /// Handles the agent's tool calls (see `live`) and answers those whose result is in.
    pub(super) fn serve_agent_tools(&mut self) {
        let requests: Vec<crate::live::Request> = match &self.live {
            Some(l) => l.requests.try_iter().collect(),
            None => return,
        };
        for r in requests {
            let Some(i) = self.tab_index(r.tab) else {
                let _ = r
                    .reply
                    .send(Err("The repository was closed in BranchBlitz".into()));
                continue;
            };
            let id = self.tabs[i].id;
            match self.agent_call(i, &r.tool, &r.args) {
                Ok(Answer::Now(text)) => {
                    let _ = r.reply.send(Ok(text));
                }
                Ok(answer) => self.live_pending.push(Pending {
                    tab: id,
                    reply: r.reply,
                    diff: matches!(answer, Answer::WhenDiff),
                    note: match answer {
                        Answer::WhenIdleSaying(t) => Some(t),
                        _ => None,
                    },
                    deadline: Instant::now() + Duration::from_secs(300),
                }),
                Err(e) => {
                    let _ = r.reply.send(Err(e));
                }
            }
        }
        for p in std::mem::take(&mut self.live_pending) {
            let Some(i) = self.tab_index(p.tab) else {
                let _ = p
                    .reply
                    .send(Err("The repository was closed in BranchBlitz".into()));
                continue;
            };
            let t = &self.tabs[i];
            let ready = if p.diff {
                !matches!(t.diff, Some((_, None)))
            } else {
                !tab_busy(t)
            };
            if ready {
                let text = if p.diff {
                    self.diff_text(i)
                } else {
                    match &p.note {
                        Some(n) => format!("{n}\n{}", self.outcome(i)),
                        None => self.outcome(i),
                    }
                };
                let _ = p.reply.send(Ok(text));
            } else if Instant::now() > p.deadline {
                let _ = p
                    .reply
                    .send(Err("Still running; check `state` later".into()));
            } else {
                self.live_pending.push(p);
            }
        }
        if !self.live_pending.is_empty() {
            self.ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// What the user sees after an action: the status line, a question or error shown, and
    /// where the tab is now.
    fn outcome(&self, i: usize) -> String {
        let t = &self.tabs[i];
        let mut parts = Vec::new();
        if let Some((ok, s)) = &self.status_line {
            parts.push(if *ok {
                s.clone()
            } else {
                format!("Failed: {s}")
            });
        }
        if let Some(Modal::Error { title, body }) = &self.modal {
            parts.push(format!("{title}:\n{body}"));
        }
        if let Some(a) = &t.alert {
            parts.push(format!(
                "Waiting for the user to confirm in the red bar: {}",
                a.title
            ));
        }
        if let Some(d) = &t.data {
            let branch = d.head_branch.as_deref().unwrap_or("(detached HEAD)");
            let mut now = format!(
                "Now on {branch}; {} staged, {} unstaged, {} conflicted",
                d.status.staged.len(),
                d.status.unstaged.len(),
                d.status.conflicted.len()
            );
            if d.state != RepoState::Clean {
                now.push_str(&format!("; {:?} in progress", d.state));
            }
            parts.push(now);
        }
        parts.join("\n")
    }

    /// The diff shown in tab `i`, for the agent (long diffs cut).
    fn diff_text(&self, i: usize) -> String {
        match &self.tabs[i].diff {
            Some((t, Some(Ok(text)))) => {
                let mut s: String = text.chars().take(40_000).collect();
                if s.len() < text.len() {
                    s.push_str("\n[diff cut off]");
                }
                if s.trim().is_empty() {
                    format!("No textual changes in {}", t.path())
                } else {
                    s
                }
            }
            Some((_, Some(Err(e)))) => format!("Failed: {e}"),
            _ => "No diff shown".into(),
        }
    }

    /// One agent tool call on tab `i`, done the way the UI does it.
    fn agent_call(
        &mut self,
        i: usize,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<Answer, String> {
        use serde_json::json;
        let arg = |k: &str| args[k].as_str().map(str::to_string);
        let need = |k: &str| arg(k).ok_or_else(|| format!("`{k}` is required"));
        let data = self.tabs[i]
            .data
            .clone()
            .ok_or("The repository is still loading; try again")?;
        let mine_is_ours = data.state != RepoState::Rebasing;
        let op = |me: &mut Self, op: Op| {
            me.status_line = None;
            me.apply(i, vec![Action::Op(op)]);
            Ok(Answer::WhenIdle)
        };
        // Asks the user in the red bar under the toolbar; runs only on their click. Never
        // replaces a question the user still has open.
        // How much the agent may do without asking (the setting in the panel's header).
        let access = self.settings.agent_access;
        let run = op;
        let ask = |me: &mut Self, title: String, body: String, op: Op| {
            if access == AgentAccess::Full {
                return run(me, op);
            }
            if me.tabs[i].alert.is_some() {
                return Err(
                    "The user has a question open in the bar; wait until they answer".into(),
                );
            }
            // Always red: a red bar needs a click, so a stray Enter can't confirm something
            // the agent asked for.
            me.apply(
                i,
                vec![Action::Confirm {
                    title: title.clone(),
                    body,
                    op,
                    danger: true,
                }],
            );
            Ok(Answer::Now(format!(
                "Asked the user in the bar: \"{title}\". It runs only if they confirm; check \
                 `state` later."
            )))
        };
        // Publishes or rewrites, but nothing is lost for good: asks only when supervised.
        let check = |me: &mut Self, title: String, body: String, op: Op| {
            if access == AgentAccess::Supervised {
                ask(me, title, body, op)
            } else {
                run(me, op)
            }
        };
        // A loaded commit, by (a prefix of) its hash: the agent can't slip in rev expressions.
        let commit = |sha: &str| {
            data.commits
                .iter()
                .find(|c| !sha.is_empty() && c.sha.starts_with(sha))
                .ok_or_else(|| format!("No loaded commit {sha}"))
        };
        match tool {
            "state" => {
                let n = args["commits"].as_u64().unwrap_or(30) as usize;
                let v = self.snapshot_of(n, Some(i))["tabs"][i].clone();
                Ok(Answer::Now(
                    serde_json::to_string_pretty(&v).unwrap_or_default(),
                ))
            }
            "select_commit" => {
                let sha = need("sha")?;
                let c = data
                    .commits
                    .iter()
                    .find(|c| c.sha.starts_with(&sha))
                    .ok_or_else(|| format!("No loaded commit {sha}"))?;
                let text = format!("Selected {} {}", git::short(&c.sha), c.subject);
                self.apply(i, vec![Action::Jump(c.sha.clone())]);
                Ok(Answer::Now(text))
            }
            "show_diff" => {
                let path = need("path")?;
                let target = if let Some(sha) = arg("commit") {
                    DiffTarget::Commit { sha, path }
                } else if args["staged"].as_bool() == Some(true) {
                    DiffTarget::Staged { path }
                } else if data.status.conflicted.contains(&path) {
                    DiffTarget::Conflict { path }
                } else {
                    let untracked = data
                        .status
                        .unstaged
                        .iter()
                        .any(|f| f.path == path && f.status == '?');
                    DiffTarget::Unstaged { path, untracked }
                };
                self.apply(i, vec![Action::ShowDiff(target)]);
                Ok(Answer::WhenDiff)
            }
            "checkout" => {
                let name = need("name")?;
                let r = data
                    .refs
                    .iter()
                    .find(|r| r.name == name)
                    .ok_or_else(|| format!("No branch or tag {name}"))?;
                let action = crate::views::checkout_action(r, &data);
                let worktree = match &action {
                    Action::SwitchWorktree(p) => Some(p.display().to_string()),
                    _ => None,
                };
                self.status_line = None;
                self.apply(i, vec![action]);
                // Said the way the user sees it: the same window now shows that worktree.
                // Answered once that worktree is loaded, so the next call finds it.
                if let Some(path) = worktree {
                    return Ok(Answer::WhenIdleSaying(format!(
                        "{name} is checked out in the worktree {path} (git allows a branch in one \
                         worktree only), so BranchBlitz now shows that worktree, in the same window. \
                         You keep working there."
                    )));
                }
                Ok(Answer::WhenIdle)
            }
            "merge" => {
                let mode = match arg("mode").as_deref() {
                    Some("no_ff") => git::MergeMode::NoFastForward,
                    Some("ff_only") => git::MergeMode::FastForwardOnly,
                    Some("squash") => git::MergeMode::Squash,
                    _ => git::MergeMode::Default,
                };
                op(
                    self,
                    Op::Merge {
                        what: need("name")?,
                        mode,
                    },
                )
            }
            "rebase" => op(self, Op::Rebase(need("onto")?)),
            "branch" => {
                let name = need("name")?;
                match need("action")?.as_str() {
                    "create" => op(
                        self,
                        Op::CreateBranch {
                            name,
                            at: arg("at").unwrap_or_else(|| "HEAD".into()),
                            checkout: args["checkout"].as_bool().unwrap_or(true),
                        },
                    ),
                    "delete" => {
                        if data.head_branch.as_deref() == Some(name.as_str()) {
                            return Err(format!("{name} is checked out; check out another first"));
                        }
                        if data.worktree_of(&name).is_some() {
                            return Err(format!("{name} is checked out in another worktree"));
                        }
                        op(self, Op::DeleteBranch { name, force: false })
                    }
                    "rename" => op(
                        self,
                        Op::RenameBranch {
                            old: name,
                            new: need("new_name")?,
                        },
                    ),
                    "fast_forward" => {
                        let up = data
                            .local(&name)
                            .and_then(|r| r.upstream.clone())
                            .ok_or_else(|| format!("{name} has no upstream"))?;
                        op(
                            self,
                            Op::FastForward {
                                local: name,
                                to: up,
                            },
                        )
                    }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "sync" => match need("action")?.as_str() {
                "fetch" => {
                    self.status_line = None;
                    self.fetch(i, false);
                    Ok(Answer::WhenIdle)
                }
                "pull" => {
                    let mode = match arg("pull_mode").as_deref() {
                        Some("ff_only") => git::PullMode::FastForwardOnly,
                        Some("rebase") => git::PullMode::Rebase,
                        Some("merge") | None => self.settings.pull_mode(&self.tabs[i].path),
                        Some(m) => return Err(format!("unknown pull_mode {m}")),
                    };
                    op(self, Op::Pull(mode))
                }
                // Publishing is the user's call.
                "push" => {
                    let b = data.head_branch.clone().unwrap_or_default();
                    check(
                        self,
                        format!("Push {b}?"),
                        format!("The agent wants to push {b} to the remote."),
                        Op::Push { force: false },
                    )
                }
                "force_push" => {
                    let b = data.head_branch.clone().unwrap_or_default();
                    ask(
                        self,
                        format!("Force push {b}?"),
                        format!(
                            "This overwrites {b} on the remote with your local branch (--force-with-lease)."
                        ),
                        Op::Push { force: true },
                    )
                }
                a => Err(format!("unknown action {a}")),
            },
            "changes" => {
                let paths: Vec<String> = args["paths"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| p.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                match need("action")?.as_str() {
                    "stage" if !paths.is_empty() => op(self, Op::Stage(paths)),
                    "unstage" if !paths.is_empty() => op(self, Op::Unstage(paths)),
                    "stage" | "unstage" => Err("`paths` is required".into()),
                    "stage_all" => op(self, Op::StageAll),
                    "unstage_all" => op(self, Op::UnstageAll),
                    "discard" => {
                        let [path] = paths.as_slice() else {
                            return Err("discard one path at a time".into());
                        };
                        if !data.status.unstaged.iter().any(|f| &f.path == path) {
                            return Err(format!("{path} has no unstaged changes"));
                        }
                        let untracked = data
                            .status
                            .unstaged
                            .iter()
                            .any(|f| &f.path == path && f.status == '?');
                        ask(
                            self,
                            format!("Discard changes to {path}?"),
                            "Your changes to this file are lost.".into(),
                            Op::Discard {
                                path: path.clone(),
                                untracked,
                            },
                        )
                    }
                    "commit" => {
                        let message = need("message")?;
                        if message.trim().is_empty() {
                            return Err("the message is empty".into());
                        }
                        if args["amend"].as_bool() == Some(true) {
                            // Rewrites the last commit, which may be pushed already.
                            return check(
                                self,
                                "Amend the last commit?".into(),
                                format!(
                                    "The agent wants to replace the last commit's message with: {}",
                                    message.lines().next().unwrap_or("")
                                ),
                                Op::Commit {
                                    message,
                                    amend: true,
                                },
                            );
                        }
                        op(
                            self,
                            Op::Commit {
                                message,
                                amend: false,
                            },
                        )
                    }
                    a => Err(format!("unknown action {a}")),
                }
            }
            "commit_action" => {
                let c = commit(&need("sha")?)?;
                let (sha, what) = (
                    c.sha.clone(),
                    format!("{} {}", git::short(&c.sha), c.subject),
                );
                let reset = |mode| Op::Reset {
                    to: sha.clone(),
                    mode,
                };
                let branch = data.head_branch.clone().unwrap_or_else(|| "HEAD".into());
                match need("action")?.as_str() {
                    "cherry_pick" => op(self, Op::CherryPick(sha.clone())),
                    "revert" => op(self, Op::Revert(sha.clone())),
                    // Resets rewrite the branch: always the user's call.
                    "reset_soft" | "reset_mixed" => check(
                        self,
                        format!("Reset {branch} to {what}?"),
                        "The commits after it leave the branch; their changes stay in the working copy.".into(),
                        reset(if args["action"] == "reset_soft" {
                            git::ResetMode::Soft
                        } else {
                            git::ResetMode::Mixed
                        }),
                    ),
                    "reset_hard" => ask(
                        self,
                        format!("Hard reset {branch} to {what}?"),
                        "Uncommitted changes and the commits after it on this branch are lost.".into(),
                        reset(git::ResetMode::Hard),
                    ),
                    "tag" => op(
                        self,
                        Op::CreateTag {
                            name: need("tag")?,
                            at: sha.clone(),
                        },
                    ),
                    a => Err(format!("unknown action {a}")),
                }
            }
            "stash" => match need("action")?.as_str() {
                "save" => op(self, Op::Stash),
                "pop" => op(self, Op::StashPop(arg("name"))),
                "apply" => op(
                    self,
                    Op::StashApply(arg("name").unwrap_or_else(|| "stash@{0}".into())),
                ),
                "drop" => {
                    let name = arg("name").unwrap_or_else(|| "stash@{0}".into());
                    ask(
                        self,
                        format!("Drop {name}?"),
                        "The stash is deleted.".into(),
                        Op::StashDrop(name),
                    )
                }
                a => Err(format!("unknown action {a}")),
            },
            "read_conflict" => {
                let path = need("path")?;
                if !data.status.conflicted.contains(&path) {
                    return Err(format!("{path} is not conflicted"));
                }
                let text = git::diff(
                    &self.tabs[i].path,
                    &DiffTarget::Conflict { path: path.clone() },
                )?;
                self.apply(
                    i,
                    vec![Action::ShowDiff(DiffTarget::Conflict {
                        path: path.clone(),
                    })],
                );
                let Some(parts) = crate::conflict::parse(&text) else {
                    return Ok(Answer::Now(format!(
                        "{path} has no conflict markers left; resolve it with side or mark it \
                         resolved by staging (changes stage)."
                    )));
                };
                let blocks: Vec<_> = parts
                    .iter()
                    .filter_map(|p| match p {
                        crate::conflict::Part::Block(b) => Some(if mine_is_ours {
                            json!({"mine": b.ours, "incoming": b.theirs})
                        } else {
                            json!({"mine": b.theirs, "incoming": b.ours})
                        }),
                        _ => None,
                    })
                    .collect();
                Ok(Answer::Now(
                    serde_json::to_string_pretty(&json!({"path": path, "blocks": blocks}))
                        .unwrap_or_default(),
                ))
            }
            "resolve_conflict" => {
                let path = need("path")?;
                if !data.status.conflicted.contains(&path) {
                    return Err(format!("{path} is not conflicted"));
                }
                if let Some(side) = arg("side") {
                    let mine = match side.as_str() {
                        "mine" => true,
                        "incoming" => false,
                        s => return Err(format!("unknown side {s}")),
                    };
                    // Overwrites the file, edits included.
                    return check(
                        self,
                        format!("Take the {side} version of {path}?"),
                        "The whole file becomes that version; edits in it are lost.".into(),
                        Op::TakeSide {
                            path,
                            ours: mine == mine_is_ours,
                        },
                    );
                }
                use crate::conflict::Choice;
                let choices: Vec<Option<Choice>> = args["choices"]
                    .as_array()
                    .ok_or("give `choices` or `side`")?
                    .iter()
                    .map(|c| match (c.as_str(), mine_is_ours) {
                        (Some("mine"), true) | (Some("incoming"), false) => Ok(Some(Choice::Ours)),
                        (Some("mine"), false) | (Some("incoming"), true) => {
                            Ok(Some(Choice::Theirs))
                        }
                        (Some("both"), true) => Ok(Some(Choice::OursThenTheirs)),
                        (Some("both"), false) => Ok(Some(Choice::TheirsThenOurs)),
                        _ => Err(format!("unknown choice {c}")),
                    })
                    .collect::<Result<_, _>>()?;
                let text = git::diff(
                    &self.tabs[i].path,
                    &DiffTarget::Conflict { path: path.clone() },
                )?;
                let parts =
                    crate::conflict::parse(&text).ok_or("no conflict markers in the file")?;
                let n = crate::conflict::blocks(&parts);
                if choices.len() != n {
                    return Err(format!(
                        "{path} has {n} conflict blocks; give one choice for each"
                    ));
                }
                let content =
                    crate::conflict::resolve(&parts, &choices).ok_or("missing a choice")?;
                op(
                    self,
                    Op::ResolveFile {
                        path,
                        content,
                        expected: text,
                    },
                )
            }
            "operation" => {
                let (cont, abort) = match data.state {
                    RepoState::Merging => (Op::ContinueMerge, Op::AbortMerge),
                    RepoState::Rebasing => (Op::ContinueRebase, Op::AbortRebase),
                    RepoState::Clean => {
                        return Err("No merge, rebase or cherry-pick in progress".into());
                    }
                    _ => (Op::ContinueMerge, Op::AbortCherryPick),
                };
                match need("action")?.as_str() {
                    "continue" => op(self, cont),
                    // Throws away the resolutions so far.
                    "abort" => ask(
                        self,
                        format!(
                            "Abort the {}?",
                            match data.state {
                                RepoState::Rebasing => "rebase",
                                RepoState::CherryPicking => "cherry-pick",
                                RepoState::Reverting => "revert",
                                _ => "merge",
                            }
                        ),
                        "Resolved conflicts and other progress are lost.".into(),
                        abort,
                    ),
                    a => Err(format!("unknown action {a}")),
                }
            }
            _ => Err(format!("unknown tool {tool}")),
        }
    }
}
