#!/usr/bin/env bash
# Builds the performance test repository: the same large history every time (default 20000
# commits on main, a feature branch merged back every 200 commits, 60 open branches, 100 tags,
# 500 files), via git fast-import. Usage: scripts/perf-repo.sh [dir] [commits] (only /tmp/*).
set -euo pipefail
DIR=${1:-/tmp/bbperf}
N=${2:-20000}
case "$DIR" in /tmp/*) ;; *) echo "refusing to use $DIR (only /tmp/* allowed)" >&2; exit 1;; esac
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
rm -rf "$DIR" && mkdir -p "$DIR" && cd "$DIR"
git init -q -b main repo && cd repo

awk -v n="$N" 'BEGIN {
    t = 1700000000; mark = 0
    # One commit: a mark, a fixed date, a file change. $1 branch, $2 parent mark (0 = none),
    # $3 second parent (merge, 0 = none).
    for (i = 1; i <= n; i++) {
        mark++
        main = mark
        printf "commit refs/heads/main\nmark :%d\n", mark
        printf "author Perf Author %d <perf%d@example.com> %d +0000\n", i % 7, i % 7, t + i * 60
        printf "committer Perf Author <perf@example.com> %d +0000\n", t + i * 60
        msg = sprintf("PERF-%d - Change number %d in module %d", i, i, i % 40)
        printf "data %d\n%s\n", length(msg), msg
        if (i > 1) printf "from :%d\n", prev
        if (merge) { printf "merge :%d\n", merge; merge = 0 }
        body = sprintf("line %d\n", i)
        printf "M 644 inline src/module%d/file%d.txt\ndata %d\n%s\n", i % 40, i % 500, length(body), body
        prev = main
        # A short feature branch every 200 commits, merged into the next main commit.
        if (i % 200 == 0) {
            last = main
            for (k = 1; k <= 3; k++) {
                mark++
                printf "commit refs/heads/feature/perf-%d\nmark :%d\n", i, mark
                printf "author Perf Author <perf@example.com> %d +0000\n", t + i * 60 + k
                printf "committer Perf Author <perf@example.com> %d +0000\n", t + i * 60 + k
                msg = sprintf("PERF-%d - Feature step %d", i, k)
                printf "data %d\n%s\nfrom :%d\n", length(msg), msg, last
                body = sprintf("feature %d %d\n", i, k)
                printf "M 644 inline features/f%d.txt\ndata %d\n%s\n", i, length(body), body
                last = mark
            }
            merge = last
            # Tag every other one.
            if (i % 400 == 0 && i / 200 <= 200) printf "tag v%d.%d.0\nfrom :%d\ntagger Perf <perf@example.com> %d +0000\ndata 3\nrel\n", i / 400 / 10, (i / 400) % 10, main, t + i * 60
        }
    }
}' | git fast-import --quiet

git checkout -q main
# Merged feature branches are deleted, except the last 60.
git for-each-ref --format='%(refname:short)' --sort=committerdate refs/heads/feature/ | head -n -60 | xargs -r git branch -q -D
# A remote with the same history, so ahead/behind and remote labels exist.
git clone -q --bare . ../origin.git && git remote add origin ../origin.git && git fetch -q origin
git branch -q -u origin/main main
echo "perf repo: $(git rev-list --count --all) commits, $(git branch | wc -l) branches, $(git tag | wc -l) tags in $DIR/repo"
