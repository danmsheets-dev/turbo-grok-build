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
# Four independent checks. The first three have already fired on this repo:
#   1. index line endings   — 34 files were stored CRLF until 06c749255a.
#   2. UTF-8 byte-order marks — 13 files carried one until fddc74d2d.
#   3. CR inside eol=lf paths — a `#!/bin/sh<CR>` shebang is "bad interpreter",
#      and a lone CR can hide inside an otherwise LF file, which check 1 still
#      classifies as `i/lf`.
#   4. embedded assets that are not eol=lf paths — the converse of 3, and the
#      invariant release.yml's deleted "Force LF" step used to cover by brute
#      force. A new `include_str!` of an unpinned path ships bytes that differ
#      by build host; nothing but this check would notice.
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

# ---------------------------------------------------------------------------
# 4. Every file compiled into the binary must BE an eol=lf (or binary) path.
# ---------------------------------------------------------------------------
# Check 3 asks "does this eol=lf path contain a CR?". This asks the converse,
# and it is the load-bearing half: "is every asset we bake into the binary
# pinned at all?" Nothing else enforces that. `.gitattributes` pins asset
# *extensions*, which is only as complete as the inventory someone took by
# hand — the next `include_str!("foo.toml")` would silently ship bytes that
# differ by build host (LF from a Linux runner, CRLF from a Windows one), and
# `git status` would show nothing. This is the invariant that let release.yml
# drop its "Force LF" step; without it that deletion is only true today.
#
# Resolution rules, applied to every tracked `.rs` file:
#   include_str!("rel/path")          -> that path, relative to the source file
#   include_str!(concat!("dir/", …))  -> every tracked file under `dir/`
#   rust_i18n::i18n!("locales")       -> every tracked file under `locales/`
#   concat!(env!("OUT_DIR"), …)       -> build-generated, never in git: skipped
#   anything not in the index         -> doc-comment example: skipped
echo "==> embedded assets are LF-pinned (include_str! / include_bytes!)"

# `.rs` sources included into a #[test] that only substring-scans them. CRLF
# vs LF cannot change a `contains()` result, and pinning `*.rs eol=lf` would
# rewrite every source file in every Windows worktree. Listed by exact path,
# not by extension, so a NEW `.rs` include still fails here and gets a
# deliberate decision instead of inheriting a blanket exemption.
scan_only_sources="
crates/codegen/xai-grok-pager-minimal/src/lib.rs
crates/codegen/xai-grok-pager-minimal/src/auth.rs
crates/codegen/xai-grok-pager-minimal/src/commit.rs
crates/codegen/xai-grok-pager-minimal/src/full_view.rs
crates/codegen/xai-grok-pager-minimal/src/live.rs
crates/codegen/xai-grok-pager-minimal/src/overlay.rs
crates/codegen/xai-grok-pager-minimal/src/panel.rs
crates/codegen/xai-grok-pager-minimal/src/plan.rs
crates/codegen/xai-grok-pager-minimal/src/todo.rs
crates/codegen/xai-grok-pager-minimal/src/welcome.rs
crates/codegen/xai-grok-shell/src/agent/config.rs
crates/codegen/xai-grok-tools/src/implementations/grok_build/read_file/mod.rs
"
printf '%s\n' $scan_only_sources | LC_ALL=C sort > "$tmp/scan_only"

git ls-files | LC_ALL=C sort > "$tmp/tracked"
git ls-files | git check-attr --stdin text | sed -n 's/: text: unset$//p' \
  | LC_ALL=C sort > "$tmp/binary"

# Collapse `path/a/../b` to `path/b` without touching the filesystem (the
# target may be generated, and `realpath -m` is not portable enough here).
normalize_path() {
  local part
  local -a parts out=()
  IFS='/' read -ra parts <<< "$1"
  for part in "${parts[@]}"; do
    case "$part" in
      '' | '.') ;;
      '..') [ "${#out[@]}" -gt 0 ] && unset 'out[${#out[@]}-1]' ;;
      *) out+=("$part") ;;
    esac
  done
  local IFS=/
  printf '%s' "${out[*]-}"
}

: > "$tmp/assets"
while IFS= read -r source; do
  [ -n "$source" ] || continue
  source_dir="$(dirname "$source")"
  # The macro's argument can wrap across lines (the `concat!` forms do), so
  # flatten the file before matching.
  flat="$(tr '\n' ' ' < "$source")"

  # include_str!("…") / include_bytes!("…")
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    arg="${hit#*\"}"
    printf '%s\n' "$(normalize_path "$source_dir/${arg%\"}")" >> "$tmp/assets"
  done < <(printf '%s' "$flat" \
    | grep -oE 'include_(str|bytes)! *\( *"[^"]*"' || true)

  # include_str!(concat!("dir/", …)) — expand the directory.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    rest="${hit#*concat!}"
    rest="${rest#*(}"
    rest="${rest#"${rest%%[![:space:]]*}"}"
    case "$rest" in
      env!*) ;; # OUT_DIR and friends: produced by build.rs, not in the index
      '"'*)
        lit="${rest#\"}"
        git ls-files -- "$(normalize_path "$source_dir/${lit%%\"*}")" >> "$tmp/assets"
        ;;
      *)
        report "unparsed include_str!/include_bytes! argument in $source" \
          "teach scripts/check-line-endings.sh check 4 how to resolve it" \
          "$rest"
        ;;
    esac
  done < <(printf '%s' "$flat" \
    | grep -oE 'include_(str|bytes)! *\( *concat! *\( *[^,]*' || true)

  # `rust_i18n::i18n!("locales")` embeds the whole directory. Same invariant,
  # different macro — and the one asset that was NOT pinned when this check was
  # written, which is the point: the inventory has to be derived, not recalled.
  # rust-i18n resolves its argument against CARGO_MANIFEST_DIR, not the source
  # file, so walk up to the crate root first.
  while IFS= read -r hit; do
    [ -n "$hit" ] || continue
    crate_dir="$source_dir"
    while [ "$crate_dir" != "." ] && [ ! -f "$crate_dir/Cargo.toml" ]; do
      crate_dir="$(dirname "$crate_dir")"
    done
    lit="${hit#*\"}"
    git ls-files -- "$(normalize_path "$crate_dir/${lit%\"}")" >> "$tmp/assets"
  done < <(printf '%s' "$flat" | grep -oE 'i18n! *\( *"[^"]*"' || true)
done < <(git grep -l -E 'include_(str|bytes)!|i18n! *\(' -- '*.rs' || true)

LC_ALL=C sort -u "$tmp/assets" > "$tmp/assets.sorted"
# Only tracked paths are in scope: anything else is a doc-comment example or a
# build artefact, and neither is stored in git for git to pin.
LC_ALL=C comm -12 "$tmp/assets.sorted" "$tmp/tracked" > "$tmp/assets.tracked"
# Acceptable: pinned to LF, git-detected binary, or an allowlisted self-scan.
LC_ALL=C sort -u "$tmp/pinned" "$tmp/binary" "$tmp/scan_only" > "$tmp/assets.ok"
mapfile -t unpinned < <(LC_ALL=C comm -23 "$tmp/assets.tracked" "$tmp/assets.ok")
if [ "${#unpinned[@]}" -gt 0 ]; then
  report "${#unpinned[@]} embedded asset(s) are not pinned to eol=lf" \
    "add the extension (or path) to .gitattributes with 'text eol=lf', then git add --renormalize" \
    "${unpinned[@]}"
fi
echo "    $(wc -l < "$tmp/assets.tracked" | tr -d ' ') tracked path(s) are embedded via include_str!/include_bytes!"

if [ "$status" -ne 0 ]; then
  printf '\n!! repo hygiene FAILED\n' >&2
  printf '   `git status` cannot show this class of problem; use\n' >&2
  printf '   `git ls-files --eol` and ./scripts/check-line-endings.sh\n' >&2
  exit 1
fi

echo "==> OK (line endings, BOMs, shipped scripts)"
