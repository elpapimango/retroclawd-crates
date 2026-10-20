#!/usr/bin/env bash
# Downloads the 68000 tests of SingleStepTests/680x0 (https://github.com/SingleStepTests/680x0) at a pinned commit into
# crates/core/68000/tests/sst/, which is git-ignored: the repository states no licence, so the files are fetched for
# local testing and never committed. `cargo test -p mc68000 --test singlestep` skips with a message when they are absent.
#
#   scripts/fetch_singlesteptests.sh [--force]
#
# Exits 1 if any file fails to download.
set -euo pipefail
cd "$(dirname "$0")/.."

COMMIT=e0d5ece9670205cc84a0101081837deb446f86a3
DEST=crates/core/68000/tests/sst
FORCE=0
[[ ${1:-} == --force ]] && FORCE=1

mkdir -p "$DEST"
names=$(curl -fsSL -A retroclawd-fetch "https://api.github.com/repos/SingleStepTests/680x0/contents/68000/v1?ref=$COMMIT" \
  | grep -o '"name": *"[^"]*"' | sed 's/.*"\([^"]*\)"$/\1/')
count=0; failed=0
for n in $names; do
  out="$DEST/$n"
  if [[ -s $out && $FORCE == 0 ]]; then count=$((count + 1)); continue; fi
  if curl -fsSL -o "$out.part" "https://raw.githubusercontent.com/SingleStepTests/680x0/$COMMIT/68000/v1/$n"; then
    mv "$out.part" "$out"; count=$((count + 1))
  else
    rm -f "$out.part"; echo "failed: $n" >&2; failed=$((failed + 1))
  fi
done
echo "$count files in $DEST (commit ${COMMIT:0:7})"
[[ $failed == 0 ]]
