#!/usr/bin/env bash
# Measures performance for a release: writes perf/<version>.json and compares it with the
# previous measurement (changes over 10% and 1 ms/MB are marked). Run from the repository root, on the same
# machine each time (the results include which one). Takes a few minutes (release builds).
set -euo pipefail
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
mkdir -p perf
out="perf/$version.json"
tmp=$(mktemp)

scripts/perf-repo.sh /tmp/bbperf >/dev/null
cargo build --profile dist --locked --quiet
BB_PERF=/tmp/bbperf/repo BB_PERF_OUT="$tmp" cargo test --release --quiet perf_baseline -- --ignored >/dev/null

cpu=$(grep -m1 "model name" /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || uname -m)
python3 - "$tmp" "$out" "$version" "$cpu" "$(rustc --version)" "$(stat -c %s target/dist/branchblitz)" <<'EOF'
import json, sys, datetime
tmp, out, version, cpu, rustc, size = sys.argv[1:]
m = json.load(open(tmp))
m["binary_mb"] = round(int(size) / 1048576, 1)
doc = {
    "version": version,
    "date": datetime.date.today().isoformat(),
    "machine": cpu,
    "rustc": rustc,
    "repo": "scripts/perf-repo.sh: 20000 commits on main, 61 branches, 50 tags",
    "metrics": m,
}
json.dump(doc, open(out, "w"), indent=2)
open(out, "a").write("\n")
EOF
rm -f "$tmp"

# The previous measurement: the newest other version.
prev=$(ls perf/*.json | grep -v "/$version.json" | sort -V | tail -1 || true)
python3 - "$out" "$prev" <<'EOF'
import json, sys
new = json.load(open(sys.argv[1]))
old = json.load(open(sys.argv[2])) if len(sys.argv) > 2 and sys.argv[2] else None
print(f"BranchBlitz {new['version']} on {new['machine']}")
if old and old["machine"] != new["machine"]:
    print(f"  (previous {old['version']} was measured on {old['machine']}: not comparable)")
for k, v in new["metrics"].items():
    line = f"  {k:<22} {v:>8}"
    if old and k in old["metrics"] and old["metrics"][k]:
        o = old["metrics"][k]
        d = (v - o) / o * 100
        # Sub-millisecond timings jitter: only a change that is large in both senses counts.
        big = abs(v - o) >= 1.0
        flag = "  <-- slower/larger" if d > 10 and big else ("  faster/smaller" if d < -10 and big else "")
        line += f"   {old['version']}: {o:>8}  ({d:+.0f}%){flag}"
    print(line)
EOF
echo "wrote $out"
