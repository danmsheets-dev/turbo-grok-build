# Agent instructions (Turbo Grok Build)

Guidance for AI agents and humans working in this monorepo. Keep builds and
tests from filling the disk; prefer targeted compile/test over full-workspace
sweeps when iterating.

## Rust disk hygiene (read this)

Rust `target/` on this monorepo routinely exceeds **100–200 GB** when debug
tests, incremental caches, and PDB sidecars accumulate—especially on Windows.
A full `cargo test --workspace` can exhaust free space and fail with
`os error 112` / `LNK1180` / `no space on device`.

### Profiles (do not mix casually)

| Profile | Path | Use |
|---------|------|-----|
| `dev` (default) | `target/debug/` | Day-to-day `cargo check` / `cargo test` |
| `release` | `target/release/` | Local optimized runs |
| `release-dist` | `target/release-dist/` | **Ship binary** (thin LTO, strip). Prefer this for RC builds. |

These directories are **independent**. Cleaning `debug` does **not** break an
in-flight `release-dist` link.

### Preferred commands (agents)

```powershell
# Prefer package-scoped work
cargo check -p xai-grok-tools -p xai-grok-shell
cargo test  -p xai-grok-tools --lib -- --test-threads=4
cargo test  -p xai-grok-shell --lib -- --test-threads=4

# Ship build (set wire version)
$env:GROK_VERSION = (Get-Content VERSION -Raw).Trim()
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
```

Avoid by default:

- `cargo test --workspace` without free disk check (unless releasing / CI)
- Parallel multi-package test + `release-dist` link at the same time on low free space
- Leaving multi‑GB log redirects (`*.txt`) in the repo root

### Cleanup recipes

**Safe while a `release-dist` build is running** (only touch debug):

```powershell
# Windows PDBs (huge; not needed for normal iteration)
Remove-Item -Recurse -Force target\debug\incremental -ErrorAction SilentlyContinue
Get-ChildItem target\debug -Filter *.pdb -Recurse -ErrorAction SilentlyContinue |
  Remove-Item -Force -ErrorAction SilentlyContinue

# Nuclear debug wipe (~tens–hundreds of GB; rebuild next cargo check/test)
Remove-Item -Recurse -Force target\debug -ErrorAction SilentlyContinue
```

**Between sessions / after green release-dist**:

```powershell
# Drop plain release if you only ship release-dist
Remove-Item -Recurse -Force target\release -ErrorAction SilentlyContinue

# Keep ship binary; free release-dist rebuild caches (~50–70+ GB typical)
# Do NOT run while cargo is still linking release-dist.
$rd = "target\release-dist"
foreach ($sub in @("incremental", "deps", "build", "examples", ".fingerprint")) {
  Remove-Item -Recurse -Force (Join-Path $rd $sub) -ErrorAction SilentlyContinue
}
# Optional nuclear debug wipe (independent of release-dist):
# Remove-Item -Recurse -Force target\debug -ErrorAction SilentlyContinue

# Full clean (last resort — long rebuild; also removes turbo.exe)
# cargo clean
```

**After a repo rename / move** (e.g. `hyper-grok-build` → `turbo-grok-build`):
build caches may still embed **absolute paths** to the old tree (cranelift,
opus, aws-lc). Symptoms: mid-build “cannot read file …\hyper-grok-build\…”.
Fix: delete polluted `target\*\build` / `.fingerprint` units, or wipe
`target\debug` and release-dist caches (keep `turbo.exe` if needed).

**Cargo global caches** (outside the repo; only if still tight on disk):

```powershell
# Lists size; does not delete source registry unless you choose to
cargo cache -a   # if cargo-cache is installed
# Or manually: %USERPROFILE%\.cargo\registry\cache  and  git\db
```

**Repo-root agent logs** (delete when nothing is writing to them):

```powershell
Remove-Item full_test*.txt, check_warn*.txt, p0_*.txt, p1_*.txt, p2_*.txt,
  warn_now.txt, core_test*.txt -ErrorAction SilentlyContinue
# Only after the matching cargo job finished:
# Remove-Item build_rc*.txt -ErrorAction SilentlyContinue
```

Do not delete a `build_*.txt` / `*_now.txt` that an active `Out-File` is still writing.

### Free-space gate (agents must check)

Before `cargo test --workspace` or `cargo build --profile release-dist`:

```powershell
# Product gate (exit 1 under GROK_MIN_FREE_GB, default 40)
turbo disk check
# Or dry-run safe reclaim of target/debug + old worktrees:
turbo disk clean --safe --dry-run
turbo disk clean --safe --if-low-space
```

Fallback without turbo:

```powershell
$freeGB = [math]::Round((Get-PSDrive H).Free / 1GB, 1)   # adjust drive
if ($freeGB -lt 40) {
  Write-Error "Only ${freeGB} GB free — clean target\debug before continuing"
}
```

Guidelines:

- **&lt; 20 GB free**: clean `target/debug` (and stop parallel builds) before anything large.
- **&lt; 40 GB free**: do not start full workspace tests; package-scoped only.
- **≥ 60 GB free**: full workspace test or `release-dist` is usually safe.
- Prefer **package-scoped** tests (`cargo test -p … --lib`) and `CARGO_INCREMENTAL=0` for one-shot CI-style runs.
- After a green `release-dist` ship build, reclaim debug bloat with `turbo disk clean --safe`.

### Windows-specific notes

- **PDB files** under `target/debug` are often the largest consumers (many GB).
  Prefer `CARGO_INCREMENTAL=0` for one-shot CI-style runs if incremental is
  thrashing disk; for day-to-day work, wipe incremental periodically instead.
- Linker errors `LNK1140` / `LNK1180` almost always mean **disk full**, not a
  code bug. Free space and retry.
- Do **not** delete `target/release-dist` while `cargo build --profile release-dist`
  is running.

### What not to delete

- `~/.cargo/registry` wholesale (slow cold builds for all projects)
- Active worktrees under `~/.grok/worktrees/` unless pruning soft-preserved
  subagent trees with product tools (`turbo subagent …` / keep-N)
- `target/release-dist` mid-ship-build
- `target/release-dist/turbo.exe` (or `turbo`) when you still need the ship binary

## Product context (short)

- Product: **Turbo Grok Build** · CLI: **`turbo`** · wire version in `VERSION`
- Agent runtime crate: **`xai-grok-shell`** (not bash); TUI: **`xai-grok-pager`**
- Composition root binary: **`xai-grok-pager-bin`** → `turbo`
- Config/auth/sessions: **`~/.grok`** (shared with official `grok` if installed)
- Changelog: [`CHANGELOG.md`](./CHANGELOG.md) · current RC line: see `VERSION`
- Hyper compare remote: **`community`** → `DaviRain-Su/hyper-grok-build` (fetch-only)
- Last sync: **RC15 / `0.2.119-r1`** — xAI upstream `e5478eff1` merged in full;
  Turbo/Hyper fork point `c260695cc`. Some Hyper fixes live only inside its merge
  commits, so survey with `git show --cc`, not `git log --no-merges`.
- Toolchain is pinned to **Rust 1.93.0**; `RUST_MIN_STACK` is set in
  `.cargo/config.toml` because the 0.2.119 turn future overflows the default
  2 MiB Rust thread stack under `cargo test`.

## When fixing isolation / subagents

Prefer package tests:

```powershell
cargo test -p xai-grok-tools --lib spawn_queues -- --nocapture
cargo test -p xai-grok-shell --lib live_worktree -- --nocapture
cargo test -p xai-grok-shell --lib prune_soft_preserved -- --nocapture
```

File product issues with `developer_log` / `turbo issues`; capability gaps with
`feature_request_log` / `turbo features`.
