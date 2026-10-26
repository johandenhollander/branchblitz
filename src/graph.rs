//! Lane layout for the commit graph.
//!
//! Every row is split in a top half (from the previous row to this commit) and a
//! bottom half (from this commit to the next row). Each half holds segments that
//! go from one lane column to another.

use std::collections::HashMap;

use crate::git::Commit;

#[derive(Clone, Copy, Debug)]
pub struct Seg {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub col: usize,
    pub color: usize,
    pub top: Vec<Seg>,
    pub bottom: Vec<Seg>,
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub rows: Vec<Row>,
    pub lanes: usize,
}

/// `colors[i]` is the colour key of `commits[i]` (see `colors::assign`). A line takes the colour of
/// the commit above it, except a merge line, which takes the colour of the branch merged in.
pub fn build(commits: &[Commit], colors: &[usize]) -> Graph {
    let known: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.sha.as_str(), i))
        .collect();
    let color_of = |i: usize| colors.get(i).copied().unwrap_or(0);
    // Each lane waits for a commit sha and carries a colour.
    let mut lanes: Vec<Option<(&str, usize)>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    let mut max_lanes = 0;

    let free_slot = |lanes: &mut Vec<Option<(&str, usize)>>| -> usize {
        match lanes.iter().position(Option::is_none) {
            Some(i) => i,
            None => {
                lanes.push(None);
                lanes.len() - 1
            }
        }
    };

    for (ci, c) in commits.iter().enumerate() {
        let sha = c.sha.as_str();
        let mut col = None;
        let color = color_of(ci);
        let mut top = Vec::new();
        let mut converging = Vec::new();
        for (i, lane) in lanes.iter().enumerate() {
            if let Some((s, lc)) = lane {
                if *s == sha {
                    if col.is_none() {
                        col = Some(i);
                    }
                    converging.push((i, *lc));
                } else {
                    top.push(Seg {
                        from: i,
                        to: i,
                        color: *lc,
                    });
                }
            }
        }
        let col = match col {
            Some(i) => i,
            None => free_slot(&mut lanes),
        };
        for (i, lc) in converging {
            top.push(Seg {
                from: i,
                to: col,
                color: lc,
            });
            lanes[i] = None;
        }

        let mut touched = vec![false; lanes.len()];
        let mut bottom = Vec::new();
        for (pi, p) in c.parents.iter().enumerate() {
            let p = p.as_str();
            let Some(&parent) = known.get(p) else {
                // Parent is beyond the loaded history: draw a short stub.
                if pi == 0 {
                    bottom.push(Seg {
                        from: col,
                        to: col,
                        color,
                    });
                }
                continue;
            };
            if let Some(k) = lanes
                .iter()
                .position(|l| matches!(l, Some((s, _)) if *s == p))
            {
                let lc = lanes[k].map(|(_, c)| c).unwrap_or(color);
                if pi == 0 && k > col && lanes[col].is_none() {
                    // Pull the parent into this (more left) lane so main lines stay straight.
                    if touched.len() < lanes.len() {
                        touched.resize(lanes.len(), false);
                    }
                    lanes[col] = Some((p, color));
                    lanes[k] = None;
                    touched[col] = true;
                    touched[k] = true;
                    bottom.push(Seg {
                        from: col,
                        to: col,
                        color,
                    });
                    bottom.push(Seg {
                        from: k,
                        to: col,
                        color: lc,
                    });
                    continue;
                }
                bottom.push(Seg {
                    from: col,
                    to: k,
                    color: if pi == 0 { color } else { lc },
                });
                continue;
            }
            let (k, lc) = if pi == 0 {
                (col, color)
            } else {
                (free_slot(&mut lanes), color_of(parent))
            };
            if touched.len() < lanes.len() {
                touched.resize(lanes.len(), false);
            }
            lanes[k] = Some((p, lc));
            touched[k] = true;
            bottom.push(Seg {
                from: col,
                to: k,
                color: lc,
            });
        }
        for (i, lane) in lanes.iter().enumerate() {
            if let Some((_, lc)) = lane
                && !touched.get(i).copied().unwrap_or(false)
            {
                bottom.push(Seg {
                    from: i,
                    to: i,
                    color: *lc,
                });
            }
        }
        while matches!(lanes.last(), Some(None)) {
            lanes.pop();
        }
        max_lanes = max_lanes.max(lanes.len()).max(col + 1);
        rows.push(Row {
            col,
            color,
            top,
            bottom,
        });
    }

    Graph {
        rows,
        lanes: max_lanes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(sha: &str, parents: &[&str]) -> Commit {
        Commit {
            sha: sha.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: String::new(),
            time: 0,
            subject: String::new(),
            body: String::new(),
        }
    }

    fn build(commits: &[Commit]) -> Graph {
        super::build(commits, &(0..commits.len()).collect::<Vec<_>>())
    }

    #[test]
    fn linear_history_uses_one_lane() {
        let g = build(&[c("c", &["b"]), c("b", &["a"]), c("a", &[])]);
        assert_eq!(g.lanes, 1);
        assert!(g.rows.iter().all(|r| r.col == 0));
    }

    #[test]
    fn merge_opens_and_closes_a_lane() {
        // m merges f into main; f and b both branch off a.
        let g = build(&[
            c("m", &["b", "f"]),
            c("f", &["a"]),
            c("b", &["a"]),
            c("a", &[]),
        ]);
        assert_eq!(g.lanes, 2);
        assert_eq!(g.rows[0].col, 0);
        assert_eq!(g.rows[1].col, 1);
        assert_eq!(g.rows[2].col, 0);
        assert_eq!(g.rows[3].col, 0);
        // The side lane folds back into the main lane right below b.
        assert!(g.rows[2].bottom.iter().any(|s| s.from == 1 && s.to == 0));
        assert_eq!(g.rows[3].top.len(), 1);
    }

    #[test]
    fn merge_line_takes_the_colour_of_the_merged_branch() {
        // m (key 0) merges f (key 7) into b (key 0).
        let commits = [
            c("m", &["b", "f"]),
            c("f", &["a"]),
            c("b", &["a"]),
            c("a", &[]),
        ];
        let g = super::build(&commits, &[0, 7, 0, 0]);
        let merge_line = g.rows[0].bottom.iter().find(|s| s.to == 1).unwrap();
        assert_eq!(merge_line.color, 7);
        assert_eq!(g.rows[1].color, 7);
        assert!(g.rows[0].bottom.iter().any(|s| s.to == 0 && s.color == 0));
    }

    #[test]
    fn missing_parent_does_not_leak_lanes() {
        let g = build(&[c("b", &["a"]), c("x", &["zzz"])]);
        assert_eq!(g.lanes, 1);
        assert_eq!(g.rows[1].bottom.len(), 1);
    }
}
