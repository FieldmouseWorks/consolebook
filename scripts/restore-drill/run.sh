#!/usr/bin/env bash
# Scripted restore drill (docs/restore-drill.md), following the install
# guide's recovery steps on one Linux machine.
#   scripts/restore-drill/run.sh TARBALL SHA256FILE WORK [HOST_B_BINARY]
# Host A: the release from TARBALL with an off-server copy directory on
# another filesystem (/dev/shm). Seed invented records, collect a manifest,
# then delete host A entirely. Host B: a fresh extraction of the same release
# (or HOST_B_BINARY), installed "with the same --backup-dir", stopped, and
# restored from the copy directory. Then doctor, collect, compare.
set -euo pipefail
TARBALL=$1 SUMS=$2 WORK=$3 HOST_B_BINARY=${4:-}
HERE=$(cd "$(dirname "$0")" && pwd)
PORT_A=7811 PORT_B=7812
export NO_COLOR=1 RUST_LOG=info
t0=$(date +%s.%N)
step() { printf '\n== [%6.1fs] %s\n' "$(awk "BEGIN { print $(date +%s.%N) - $t0 }")" "$*"; }
health() { for _ in $(seq 100); do curl -fsS "http://127.0.0.1:$1/api/health" 2>/dev/null && return 0; sleep 0.1; done; return 1; }
stop() { kill -TERM "$1"; wait "$1" || true; }
doctor() {  # 0.1.0's doctor has no --backup-copy-dir
  if "$1" doctor --help | grep -q backup-copy-dir; then "$1" doctor --backup-copy-dir "$COPY" || true; else "$1" doctor || true; fi
}

rm -rf "$WORK"; mkdir -p "$WORK"
COPY=$(mktemp -d /dev/shm/consolebook-drill-copy.XXXXXX)
echo "copy directory: $COPY (filesystem $(stat -f -c %T "$COPY")); data on $(stat -f -c %T "$WORK")"

step "verify the release archive"
(cd "$(dirname "$TARBALL")" && sha256sum -c "$(basename "$SUMS")")

step "host A: install the release"
mkdir -p "$WORK/hostA"
tar -xzf "$TARBALL" -C "$WORK/hostA"
BIN_A=$(echo "$WORK"/hostA/consolebook-*/consolebook)
"$BIN_A" --version
export CONSOLEBOOK_DATA_DIR="$WORK/hostA/data"
"$BIN_A" serve --bind 127.0.0.1:$PORT_A --backup-copy-dir "$COPY" >"$WORK/hostA.log" 2>&1 &
PID=$!
health $PORT_A; echo
CODE=$(grep -m1 -o 'setup_code="[0-9a-f]*"' "$WORK/hostA.log" | cut -d'"' -f2)

step "host A: seed invented records and collect the 'before' manifest"
python3 -I "$HERE/seed_collect.py" seed "http://127.0.0.1:$PORT_A" "$CODE" "$WORK/before"

step "host A: take a backup now (copied off the server)"
"$BIN_A" backup --backup-copy-dir "$COPY"
stop $PID
sha256sum "$WORK/hostA/data/consolebook.db"
ls -l --time-style=+%H:%M:%S "$COPY"

step "destroy host A: binary, data, backups, logs"
cp "$WORK/hostA.log" "$WORK/hostA.log.kept"
rm -rf "$WORK/hostA"
test ! -e "$WORK/hostA" && echo "host A is gone; only the copy directory and the recorded manifest remain"

# A real replacement starts long after the last backup; snapshot names carry
# the second, so keep host B's first backup from sharing host A's last name.
sleep 2
step "host B: fresh install with the same --backup-dir (the service starts)"
mkdir -p "$WORK/hostB"
if [[ -n $HOST_B_BINARY ]]; then
  mkdir -p "$WORK/hostB/rel"; cp "$HOST_B_BINARY" "$WORK/hostB/rel/consolebook"; BIN_B="$WORK/hostB/rel/consolebook"
else
  tar -xzf "$TARBALL" -C "$WORK/hostB"; BIN_B=$(echo "$WORK"/hostB/consolebook-*/consolebook)
fi
"$BIN_B" --version
export CONSOLEBOOK_DATA_DIR="$WORK/hostB/data"
"$BIN_B" serve --bind 127.0.0.1:$PORT_B --backup-copy-dir "$COPY" >"$WORK/hostB-1.log" 2>&1 &
PID=$!
health $PORT_B; echo
sleep 2  # let the scheduler's first pass run, as it would on a real host
step "host B: stop the service"
stop $PID
grep -E 'backup|cop(y|ied)' "$WORK/hostB-1.log" || true
ls -l --time-style=+%H:%M:%S "$COPY"

NEWEST=$(ls "$COPY"/consolebook-*.db | grep -v premigration | sort | tail -1)
step "host B: restore the newest file from the copy directory, as the guide says: $(basename "$NEWEST")"
"$BIN_B" restore "$NEWEST"

step "host B: start and check"
"$BIN_B" serve --bind 127.0.0.1:$PORT_B --backup-copy-dir "$COPY" >"$WORK/hostB-2.log" 2>&1 &
PID=$!
health $PORT_B; echo
doctor "$BIN_B"
if python3 -I "$HERE/seed_collect.py" collect "http://127.0.0.1:$PORT_B" "$WORK/after-newest"; then
  RESTORED_OK=1
else
  RESTORED_OK=0
  echo "SURPRISE: the newest copy does not hold the records"
fi
stop $PID

if [[ $RESTORED_OK == 0 ]]; then
  PRE=$(ls "$COPY"/consolebook-*.db | grep -v premigration | sort | awk '{l[NR]=$0} END{print l[NR-1]}')
  step "host B: restore the newest copy from before the loss instead: $(basename "$PRE")"
  "$BIN_B" restore "$PRE"
  "$BIN_B" serve --bind 127.0.0.1:$PORT_B --backup-copy-dir "$COPY" >"$WORK/hostB-3.log" 2>&1 &
  PID=$!
  health $PORT_B; echo
  doctor "$BIN_B"
  python3 -I "$HERE/seed_collect.py" collect "http://127.0.0.1:$PORT_B" "$WORK/after"
  stop $PID
else
  mv "$WORK/after-newest" "$WORK/after"
fi

step "compare before and after"
python3 -I - "$WORK/before/manifest.json" "$WORK/after/manifest.json" <<'EOF'
import json, sys
a, b = (json.load(open(p)) for p in sys.argv[1:3])
for key in ("installation_export_bytes",):
    a.pop(key, None); b.pop(key, None)
added = sorted(set(b["instance"]) - set(a["instance"]))
for key in added:  # fields a newer version adds to /api/instance
    b["instance"].pop(key)
same = a == b
print("instance:", a["instance"] == b["instance"], "| fields only the restoring version reports:", added,
      "| records:", len(a["records"]), "vs", len(b["records"]))
for ra, rb in zip(a["records"], b["records"]):
    print(f"  record {ra['record_id']}: content {ra['meta'].get('content_hash','?')[:16]} chain {ra['meta'].get('chain_hash','?')[:16]} "
          f"meta equal={ra['meta']==rb['meta']} ack equal={ra['acknowledgment']==rb['acknowledgment']}")
print("MANIFESTS EQUAL" if same else "MANIFESTS DIFFER")
sys.exit(0 if same else 1)
EOF
for side in before after; do
  for zip in "$WORK/$side"/*.zip; do
    printf '%s/%s: ' "$side" "$(basename "$zip")"
    "$BIN_B" export verify "$zip" | tail -1
  done
done
step "done"
