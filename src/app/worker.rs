//! The per-tab worker thread: runs git jobs in order and reports back.

use super::*;

pub(super) fn spawn_worker(
    id: u64,
    path: PathBuf,
    tx: Sender<Msg>,
    ctx: egui::Context,
    limit: Arc<AtomicUsize>,
) -> Sender<Job> {
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        let mut last: Option<RepoData> = None;
        // Highest load request seen; every load sent afterwards includes it.
        let mut generation = 0;
        // Remote tags are only known after a fetch; every load carries the last answer.
        let mut remote_tags: Option<HashMap<String, String>> = None;
        while let Ok(first) = job_rx.recv() {
            let mut jobs = vec![first];
            while let Ok(j) = job_rx.try_recv() {
                jobs.push(j);
            }
            for j in &jobs {
                if let Job::Load(g) = j {
                    generation = generation.max(*g);
                }
            }
            let send_load = |last: &mut Option<RepoData>, remote_tags: &Option<_>| {
                let loaded = git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                    d.remote_tags = Clone::clone(remote_tags);
                    d
                });
                *last = loaded.as_ref().ok().cloned();
                let _ = tx.send(Msg {
                    tab: id,
                    kind: MsgKind::Loaded(generation, loaded),
                });
                ctx.request_repaint();
            };
            let mut need_load = false;
            for job in jobs {
                match job {
                    Job::Load(_) => need_load = true,
                    Job::Op { op, quiet } => {
                        // Show the local state first; the operation (e.g. fetch) may be slow.
                        if need_load {
                            send_load(&mut last, &remote_tags);
                        }
                        let result = op.run(&path, last.as_ref());
                        if result.is_ok()
                            && let Op::PushTag { name, .. } = &op
                            && let Some(t) = &mut remote_tags
                            && let Some(r) = last.as_ref().and_then(|d| {
                                d.refs
                                    .iter()
                                    .find(|r| r.kind == git::RefKind::Tag && &r.name == name)
                            })
                        {
                            t.insert(name.clone(), r.target.clone());
                        }
                        let check_tags = result.is_ok() && matches!(op, Op::Fetch | Op::Pull(_));
                        // Reload before reporting so the graph is current when the spinner stops.
                        let loaded =
                            git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                                d.remote_tags = remote_tags.clone();
                                d
                            });
                        last = loaded.as_ref().ok().cloned();
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::Loaded(generation, loaded),
                        });
                        let _ = tx.send(Msg {
                            tab: id,
                            kind: MsgKind::OpDone { op, quiet, result },
                        });
                        ctx.request_repaint();
                        need_load = false;
                        // Which tags the remotes have: asked after the fetch is reported, so a slow
                        // remote doesn't keep the fetch busy. Unreachable means unknown, not "unpushed".
                        if check_tags {
                            let remotes =
                                last.as_ref().map(|d| d.remotes.clone()).unwrap_or_default();
                            remote_tags = git::remote_tags(&path, &remotes).ok();
                            let loaded = match last.clone() {
                                Some(mut d) => {
                                    d.remote_tags = remote_tags.clone();
                                    Ok(d)
                                }
                                None => git::load(&path, limit.load(Ordering::Relaxed)),
                            };
                            last = loaded.as_ref().ok().cloned();
                            let _ = tx.send(Msg {
                                tab: id,
                                kind: MsgKind::Loaded(generation, loaded),
                            });
                            ctx.request_repaint();
                        }
                    }
                }
            }
            if need_load {
                let loaded = git::load(&path, limit.load(Ordering::Relaxed)).map(|mut d| {
                    d.remote_tags = remote_tags.clone();
                    d
                });
                last = loaded.as_ref().ok().cloned();
                let _ = tx.send(Msg {
                    tab: id,
                    kind: MsgKind::Loaded(generation, loaded),
                });
                ctx.request_repaint();
            }
        }
    });
    job_tx
}
