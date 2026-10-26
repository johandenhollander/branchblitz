//! Branch colours: which branch a commit belongs to, and which colour that branch gets.
//!
//! Colours come from user rules (`main, master` → red, `feature/*` → green, …). Every commit is
//! assigned to one branch: first the first-parent chains of the existing branches (in rule order,
//! so `main` claims shared history before `feature/x`), then the side of each merge, named after
//! the merge message when the branch itself is gone ("Merged in hotfix/x", "Merge branch 'x'").

use std::collections::HashMap;

use eframe::egui::Color32;
use serde::{Deserialize, Serialize};

use crate::git::{GitFlow, RefKind, RepoData};
use crate::theme::Palette;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRule {
    /// Comma-separated branch names; `*` matches anything (`feature/*`).
    pub patterns: String,
    pub color: [u8; 3],
}

impl ColorRule {
    fn new(patterns: &str, color: [u8; 3]) -> Self {
        ColorRule {
            patterns: patterns.into(),
            color,
        }
    }

    pub fn matches(&self, branch: &str) -> bool {
        self.patterns
            .split(',')
            .map(str::trim)
            .any(|p| !p.is_empty() && glob(p, branch))
    }
}

/// The built-in rules. Order is priority: earlier rules own shared history.
pub fn default_rules() -> Vec<ColorRule> {
    // Design system lanes (dark values; light mode uses the light value of the same lane).
    vec![
        ColorRule::new("master, main", [239, 68, 68]),
        ColorRule::new("develop, development", [59, 130, 246]),
        ColorRule::new("release/*", [45, 212, 191]),
        ColorRule::new("hotfix/*", [245, 158, 11]),
        ColorRule::new("bugfix/*", [167, 139, 250]),
        ColorRule::new("feature/*", [34, 197, 94]),
    ]
}

/// Rules as saved by earlier versions that only had the defaults. Users who never changed
/// them get the current defaults instead.
pub fn upgrade_rules(rules: &mut Vec<ColorRule>) {
    let before_design_system = vec![
        ColorRule::new("master, main", [229, 72, 77]),
        ColorRule::new("develop, development", [59, 130, 246]),
        ColorRule::new("release/*", [168, 85, 247]),
        ColorRule::new("hotfix/*", [249, 115, 22]),
        ColorRule::new("bugfix/*", [234, 179, 8]),
        ColorRule::new("feature/*", [34, 197, 94]),
    ];
    let first = vec![
        ColorRule::new("main, master", [229, 72, 77]),
        ColorRule::new("develop, development", [59, 130, 246]),
        ColorRule::new("hotfix/*", [249, 115, 22]),
        ColorRule::new("bugfix/*", [234, 179, 8]),
        ColorRule::new("feature/*", [34, 197, 94]),
    ];
    if *rules == first || *rules == before_design_system {
        *rules = default_rules();
    }
}

/// `*` matches any run of characters, including `/`.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() || !text.ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// The branch a merge commit brought in, from the usual merge messages of git and hosting tools.
pub fn merged_branch(subject: &str) -> Option<&str> {
    let name = if let Some(r) = subject.strip_prefix("Merge branch '") {
        r.split('\'').next()
    } else if let Some(r) = subject.strip_prefix("Merge remote-tracking branch '") {
        // 'origin/feature/x' → feature/x
        r.split('\'')
            .next()
            .map(|n| n.split_once('/').map_or(n, |(_, b)| b))
    } else if let Some(r) = subject.strip_prefix("Merged in ") {
        r.split_whitespace().next()
    } else if let Some(r) = subject.strip_prefix("Merge pull request #") {
        // "Merge pull request #12 from owner/feature/x"
        r.split_once(" from ")
            .and_then(|(_, f)| f.split_whitespace().next())
            .map(|f| f.split_once('/').map_or(f, |(_, b)| b))
    } else {
        None
    }?;
    (!name.is_empty()).then_some(name)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    Fixed(Color32),
    /// Index into the theme's lane palette.
    Auto(usize),
}

/// Result of [`assign`]: a colour key per commit (same order as `RepoData::commits`).
#[derive(Clone, Debug, Default)]
pub struct Coloring {
    pub commit: Vec<usize>,
    paints: Vec<Paint>,
    by_branch: HashMap<String, usize>,
}

impl Coloring {
    /// The colour for `key`, adjusted to keep enough contrast with the theme's background.
    pub fn color(&self, key: usize, pal: &Palette) -> Color32 {
        pal.graphic(match self.paints.get(key) {
            Some(Paint::Fixed(c)) => pal.rule_color(*c),
            Some(Paint::Auto(i)) => pal.lane(*i),
            None => pal.lane(key),
        })
    }

    /// Colour of a branch by name (without remote prefix), if it is in this repository.
    pub fn branch(&self, name: &str, pal: &Palette) -> Option<Color32> {
        self.by_branch.get(name).map(|&k| self.color(k, pal))
    }
}

struct Builder<'a> {
    rules: &'a [ColorRule],
    flow: &'a GitFlow,
    c: Coloring,
    anonymous: usize,
}

impl Builder<'_> {
    fn named(&mut self, name: &str) -> usize {
        if let Some(&k) = self.c.by_branch.get(name) {
            return k;
        }
        let role = self.flow.canonical(name);
        let paint = match self
            .rules
            .iter()
            .find(|r| r.matches(name) || r.matches(&role))
        {
            Some(r) => Paint::Fixed(Color32::from_rgb(r.color[0], r.color[1], r.color[2])),
            None => {
                // FNV-1a: the same name gets the same palette colour in every run and build.
                let h = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                    (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
                });
                Paint::Auto(h as usize)
            }
        };
        self.c.paints.push(paint);
        let k = self.c.paints.len() - 1;
        self.c.by_branch.insert(name.to_string(), k);
        k
    }

    fn unnamed(&mut self) -> usize {
        self.anonymous += 1;
        self.c.paints.push(Paint::Auto(self.anonymous * 3));
        self.c.paints.len() - 1
    }
}

pub fn assign(data: &RepoData, rules: &[ColorRule]) -> Coloring {
    let commits = &data.commits;
    let index: HashMap<&str, usize> = commits
        .iter()
        .enumerate()
        .map(|(i, c)| (c.sha.as_str(), i))
        .collect();
    let mut b = Builder {
        rules,
        flow: &data.flow,
        c: Coloring::default(),
        anonymous: 0,
    };
    let mut owner: Vec<Option<usize>> = vec![None; commits.len()];

    // Claim the first-parent chain from `start` until it reaches a commit that is already owned.
    let claim = |owner: &mut Vec<Option<usize>>, start: usize, key: usize| {
        let mut i = Some(start);
        while let Some(n) = i {
            if owner[n].is_some() {
                break;
            }
            owner[n] = Some(key);
            i = commits[n]
                .parents
                .first()
                .and_then(|p| index.get(p.as_str()))
                .copied();
        }
    };

    let mut tips: Vec<(usize, bool, &str, usize)> = data
        .refs
        .iter()
        .filter(|r| r.kind != RefKind::Tag)
        .filter_map(|r| {
            let name = r.branch_name();
            let tip = *index.get(r.target.as_str())?;
            let role = data.flow.canonical(name);
            let rank = rules
                .iter()
                .position(|rule| rule.matches(name) || rule.matches(&role))
                .unwrap_or(rules.len());
            let current = data.head_branch.as_deref() == Some(name);
            Some((rank, !current, name, tip))
        })
        .collect();
    tips.sort();
    for (_, _, name, tip) in tips {
        let key = b.named(name);
        claim(&mut owner, tip, key);
    }

    for i in 0..commits.len() {
        if owner[i].is_none() {
            let key = b.unnamed();
            claim(&mut owner, i, key);
        }
        for (pi, p) in commits[i].parents.iter().enumerate().skip(1) {
            let Some(&pidx) = index.get(p.as_str()) else {
                continue;
            };
            if owner[pidx].is_some() {
                continue;
            }
            let key = match merged_branch(&commits[i].subject) {
                Some(name) if pi == 1 => b.named(name),
                _ => b.unnamed(),
            };
            claim(&mut owner, pidx, key);
        }
    }

    b.c.commit = owner.into_iter().map(|o| o.unwrap_or(0)).collect();
    b.c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{Commit, RefInfo};

    fn c(sha: &str, parents: &[&str], subject: &str) -> Commit {
        Commit {
            sha: sha.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: String::new(),
            time: 0,
            subject: subject.into(),
            body: String::new(),
        }
    }

    fn r(name: &str, kind: RefKind, target: &str) -> RefInfo {
        RefInfo {
            name: name.into(),
            full: String::new(),
            kind,
            target: target.into(),
            upstream: None,
            ahead: 0,
            behind: 0,
            upstream_gone: false,
            remote: (kind == RefKind::Remote).then(|| "origin".into()),
        }
    }

    #[test]
    fn patterns_and_merge_messages() {
        let rules = default_rules();
        assert!(rules[0].matches("main") && rules[0].matches("master"));
        assert!(rules[5].matches("feature/PROJ-1/sub") && !rules[5].matches("feature"));
        assert!(glob("release-*-rc", "release-3.1-rc") && !glob("release-*-rc", "release-3.1"));
        assert_eq!(
            merged_branch("Merge branch 'feature/x' into develop"),
            Some("feature/x")
        );
        assert_eq!(
            merged_branch("Merge remote-tracking branch 'origin/develop' into f"),
            Some("develop")
        );
        assert_eq!(
            merged_branch("Merged in hotfix/PROJ-573 (pull request #591)"),
            Some("hotfix/PROJ-573")
        );
        assert_eq!(
            merged_branch("Merge pull request #12 from someone/bugfix/y"),
            Some("bugfix/y")
        );
        assert_eq!(merged_branch("Fix the merge button"), None);
        assert!(!glob("a*a", "a") && glob("a*a", "aa") && glob("*", "x") && glob("**", ""));
        let empty = ColorRule {
            patterns: " , ".into(),
            color: [0, 0, 0],
        };
        assert!(!empty.matches("main"));
    }

    #[test]
    fn branches_keep_their_colour_and_deleted_ones_are_recognised() {
        // main: a - m (merges deleted hotfix/h: h1); develop: d on top of a; origin/main at m.
        let data = RepoData {
            worktrees: Default::default(),
            limit: 0,
            head_branch: Some("develop".into()),
            head_sha: Some("d".into()),
            refs: vec![
                r("develop", RefKind::Local, "d"),
                r("origin/main", RefKind::Remote, "m"),
                r("v1", RefKind::Tag, "m"),
            ],
            commits: vec![
                c("d", &["a"], "work"),
                c("m", &["a", "h1"], "Merged in hotfix/h (pull request #1)"),
                c("h1", &["a"], "fix"),
                c("a", &[], "init"),
            ],
            status: Default::default(),
            state: crate::git::RepoState::Clean,
            stashes: vec![],
            remotes: vec!["origin".into()],
            head_history: Default::default(),
            default_branches: vec![],
            flow: Default::default(),
            remote_tags: None,
        };
        let col = assign(&data, &default_rules());
        let pal = Palette::for_theme(eframe::egui::Theme::Dark);
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        let color_of = |i: usize| rgb(col.color(col.commit[i], &pal));
        let rules = default_rules();
        assert_eq!(color_of(0), rules[1].color, "develop is blue");
        assert_eq!(color_of(1), rules[0].color, "main is red");
        assert_eq!(
            color_of(3),
            rules[0].color,
            "shared history belongs to main"
        );
        assert_eq!(color_of(2), rules[3].color, "deleted hotfix/h is orange");
        assert_eq!(rgb(col.branch("main", &pal).unwrap()), rules[0].color);
    }

    #[test]
    fn git_flow_names_get_the_colour_of_their_role() {
        let data = RepoData {
            worktrees: Default::default(),
            limit: 0,
            head_branch: None,
            head_sha: None,
            refs: vec![
                r("production", RefKind::Local, "p"),
                r("feat-login", RefKind::Local, "f"),
            ],
            commits: vec![c("f", &["p"], "login"), c("p", &[], "init")],
            status: Default::default(),
            state: crate::git::RepoState::Clean,
            stashes: vec![],
            remotes: vec![],
            head_history: Default::default(),
            default_branches: vec![],
            flow: GitFlow {
                master: Some("production".into()),
                develop: None,
                prefixes: vec![("feature".into(), "feat-".into())],
            },
            remote_tags: None,
        };
        let rules = default_rules();
        let col = assign(&data, &rules);
        let pal = Palette::for_theme(eframe::egui::Theme::Dark);
        let rgb = |c: Color32| [c.r(), c.g(), c.b()];
        assert_eq!(rgb(col.branch("production", &pal).unwrap()), rules[0].color);
        assert_eq!(rgb(col.branch("feat-login", &pal).unwrap()), rules[5].color);
        // A rule for the real name still applies (it comes first here).
        let mut own = vec![ColorRule::new("feat-*", [255, 0, 255])];
        own.extend(default_rules());
        let col = assign(&data, &own);
        assert_eq!(rgb(col.branch("feat-login", &pal).unwrap()), [255, 0, 255]);
    }

    #[test]
    fn untouched_old_defaults_are_upgraded() {
        let mut first = vec![
            ColorRule::new("main, master", [229, 72, 77]),
            ColorRule::new("develop, development", [59, 130, 246]),
            ColorRule::new("hotfix/*", [249, 115, 22]),
            ColorRule::new("bugfix/*", [234, 179, 8]),
            ColorRule::new("feature/*", [34, 197, 94]),
        ];
        upgrade_rules(&mut first);
        assert_eq!(first, default_rules());
        let mut before_design_system = vec![
            ColorRule::new("master, main", [229, 72, 77]),
            ColorRule::new("develop, development", [59, 130, 246]),
            ColorRule::new("release/*", [168, 85, 247]),
            ColorRule::new("hotfix/*", [249, 115, 22]),
            ColorRule::new("bugfix/*", [234, 179, 8]),
            ColorRule::new("feature/*", [34, 197, 94]),
        ];
        upgrade_rules(&mut before_design_system);
        assert_eq!(before_design_system, default_rules());
        let mut custom = vec![ColorRule::new("trunk", [1, 2, 3])];
        upgrade_rules(&mut custom);
        assert_eq!(custom.len(), 1);
    }
}
