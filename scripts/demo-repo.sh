#!/usr/bin/env bash
# Build a throwaway demo repository with remotes, merges, tags and WIP changes.
# Usage: scripts/demo-repo.sh [dir]   (default /tmp/bbdemo; the dir is wiped first)
set -euo pipefail
DIR=${1:-/tmp/bbdemo}
case "$DIR" in /tmp/*) ;; *) echo "refusing to wipe $DIR (only /tmp/* allowed)" >&2; exit 1;; esac
# Ignore the user's git config (signing, hooks, templates) so the script works everywhere.
# HOME/XDG_CONFIG_HOME cover git versions older than 2.32 that ignore GIT_CONFIG_GLOBAL.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 HOME="$DIR" XDG_CONFIG_HOME="$DIR/.config"
# An existing empty directory is used as is (the MCP server creates one securely).
if [ -e "$DIR" ] && [ -n "$(ls -A "$DIR")" ]; then rm -rf "$DIR"; fi
mkdir -p "$DIR" && cd "$DIR"
git init -q --bare -b main origin.git && git clone -q origin.git work 2>/dev/null && cd work
git config user.name "Demo Developer" && git config user.email dev@example.com
mk(){ echo "$RANDOM" >> "$1"; git add -A; git commit -qm "$2"; }
git switch -qc main; mk a.txt "Initial commit"; mk a.txt "Add README"; git push -qu origin main
git switch -qc develop; mk b.txt "Setup develop"; git push -qu origin develop
git switch -qc feature/PROJ-483; mk c.txt "PROJ-483 - Copy dummy files"; mk c.txt "PROJ-483 - Make Cypress suite run"; git push -qu origin feature/PROJ-483
git switch -q develop; git merge -q --no-ff --no-edit feature/PROJ-483 -m "Merged in feature/PROJ-483 (pull request #595)"; git push -q
git switch -qc hotfix/PROJ-573 main; mk d.txt "PROJ-573 - Remove config of deleted communities"; git push -qu origin hotfix/PROJ-573
git switch -q main; git merge -q --no-ff --no-edit hotfix/PROJ-573 -m "Merged in hotfix/PROJ-573 (pull request #591)"; git tag v3.1.4; git push -q --tags origin main
git switch -q develop; git merge -q --no-edit main -m "Merge branch main into develop"; git push -q
git switch -qc feature/QA-391-test; mk e.txt "QA-391 - Add Cypress test for preview links"; git push -qu origin feature/QA-391-test
git switch -qc bugfix/PROJ-610 develop; mk g.txt "PROJ-610 - Fix date format"; git push -qu origin bugfix/PROJ-610
git switch -qc feature/PROJ-523-ai-search develop; mk f.txt "PROJ-561 - Update search module"; mk f.txt "PROJ-561 - Show follow-up sources as cards"
cd "$DIR" && git clone -q origin.git other && cd other && git config user.name "Alex Example" && git config user.email alex@example.com && git switch -q develop && echo x>>x && git add x && git commit -qm "PROJ-600 - Update dependencies" && git push -q && git switch -qc feature/PROJ-601 && echo y>>y && git add y && git commit -qm "PROJ-601 - New search page" && git push -qu origin feature/PROJ-601
cd "$DIR"/work && echo wip >> a.txt && echo new > new.txt && git log --oneline --all | wc -l
