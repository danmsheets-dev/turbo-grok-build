#!/usr/bin/env bash
# Repo hygiene guards for the byte-level corruption classes that `git status`
# structurally cannot show.
#
# `git status` is blind to line-ending drift by design. Git's safe-autocrlf
# conversion is deliberately asymmetric — checkout refuses to add CR to a blob
# that already carries any, and check-in refuses to strip CR from a blob that
# carries none — so a worktree holding either spelling cleans back to the same
# blob and the tree reports clean. The diagnostic is `git ls-files --eol`, not
# `git status`.
#
# Three independent checks. Each has already fired on this repository:
#   1. index line endings   — 34 files were stored CRLF until 06c749255a.
#   2. UTF-8 byte-order marks — 13 files carried one until fddc74d2d.
#   3. CR inside eol=lf paths — a `#!/bin/sh<CR>` shebang is "bad interpreter",
#      and a lone CR can hide inside an otherwise LF file, which check 1 still
#      classifies as `i/lf`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

status=0
report() {
  status=1
  printf '\n!! %s\n' "$1" >&2
  printf '   fix: %s\n' "$2" >&2
  shift 2
  printf '     %s\n' "$@" >&2
}

# ---------------------------------------------------------------------------
# 1. The index must be pure LF.
# ---------------------------------------------------------------------------
# `i/` is the class of the stored blob, independent of any checkout filter, so
# this is the one check that describes what other clones will actually receive.
echo "==> index line endings (git ls-files --eol)"
mapfile -t index_bad < <(git ls-files --eol | grep -E '^i/(crlf|mixed)' || true)
if [ "${#index_bad[@]}" -gt 0 ]; then
  report "${#index_bad[@]} path(s) are stored with CR in the index" \
    "git add --renormalize . && git commit" \
    "${index_bad[@]}"
fi

# ---------------------------------------------------------------------------
# 2. No UTF-8 byte-order marks.
# ---------------------------------------------------------------------------
# A second corruption axis that `eol=lf` cannot touch: a BOM survives every
# line-ending filter. `-I` leaves git-detected binaries alone, so this only
# looks at text. The match is not anchored to offset 0 — a stray EF BB BF
# anywhere in a text file is worth failing on. If a fixture ever needs those
# bytes on purpose, exclude it with a `:(exclude)` pathspec below rather than
# weakening the check.
echo "==> UTF-8 byte-order marks"
bom="$(printf '\357\273\277')"
mapfile -t bom_hits < <(git grep --no-color -l -I -F -e "$bom" -- . || true)
if [ "${#bom_hits[@]}" -gt 0 ]; then
  report "${#bom_hits[@]} text file(s) contain a UTF-8 BOM" \
    "strip the leading EF BB BF bytes; keep the file UTF-8 without signature" \
    "${bom_hits[@]}"
fi

# ---------------------------------------------------------------------------
# 3. No CR bytes in any path pinned to eol=lf.
# ---------------------------------------------------------------------------
# The pinned set is read back out of .gitattributes rather than hard-coded, so
# adding a new `eol=lf` rule extends this check automatically and the two can
# never drift apart. Covers install.sh, scripts/*.sh, the bundled runtime, the
# npm bin entry point, bin/protoc, and every embedded markdown template.
echo "==> CR bytes in eol=lf paths"
cr="$(printf '\r')"
git ls-files | git check-attr --stdin eol | sed -n 's/: eol: lf$//p' \
  | LC_ALL=C sort > "$tmp/pinned"
git grep --no-color -l -I -F -e "$cr" -- . | LC_ALL=C sort > "$tmp/carriage" || true
mapfile -t cr_hits < <(LC_ALL=C comm -12 "$tmp/pinned" "$tmp/carriage")
if [ "${#cr_hits[@]}" -gt 0 ]; then
  report "${#cr_hits[@]} LF-pinned path(s) contain a CR byte" \
    "remove the CR; these files must be byte-identical on every host" \
    "${cr_hits[@]}"
fi
echo "    $(wc -l < "$tmp/pinned" | tr -d ' ') path(s) are pinned to eol=lf"

if [ "$status" -ne 0 ]; then
  printf '\n!! repo hygiene FAILED\n' >&2
  printf '   `git status` cannot show this class of problem; use\n' >&2
  printf '   `git ls-files --eol` and ./scripts/check-line-endings.sh\n' >&2
  exit 1
fi

echo "==> OK (line endings, BOMs, shipped scripts)"
