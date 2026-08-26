# 1.0.0-rc.12 Subagent Hardening + Turbo Build Rename

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship **1.0.0-rc.12** as *Subagent Hardening* plus the user-facing product rename **Turbo Grok Build → Turbo Build**, with live Q&A proving isolation honesty and a smaller boot card.

**Architecture:** Keep the CLI binary `turbo`, git remote `turbo-grok-build`, `~/.grok` / `~/.turbo`, and the machine identity `product: "turbo-grok-build"` in `--version --json` (plugin/harness contract). Change **display strings** only. Unify the Windows short-worktree detector so the child boot card, start-gate, and parent isolation briefing agree. Densify boot cards by dropping duplicated ADL/FRL prose and teaching the real Windows path `{drive}:\t\w\{hash}\subagent-…`.

**Tech Stack:** Rust workspace (`xai-grok-agent`, `xai-grok-shell`, `xai-grok-pager`, `xai-grok-pager-bin`, `xai-grok-version`), cargo package-scoped tests, live `spawn_subagent` Q&A on Windows.

**Branch:** `rc12` off `dev` (`3bc348b4f` = 1.0.0-rc.11.1). Wire version `1.0.0-rc.12`. Do not push or tag until the human asks.

**Out of scope:** Renaming the GitHub repo, the `turbo` binary, Turborepo disambiguation JSON field, OS sandbox, meeting/browser features, shipping a GitHub Release (human).

---

## Files

| File | Role |
|------|------|
| `crates/codegen/xai-grok-agent/src/prompt/boot_card.rs` | Isolation label, child/short card text, tests |
| `crates/codegen/xai-grok-agent/src/prompt/context.rs` | Already infers isolation from tool CWD; no change if label is honest |
| `crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs` | Start-gate `path_looks_like_subagent_worktree` (already rc.11.1-correct; leave unless sharing helper) |
| `crates/codegen/xai-grok-version/src/lib.rs` | `PRODUCT_DISPLAY_NAME` |
| `crates/codegen/xai-grok-pager/src/views/welcome/mod.rs` | Welcome badge strings |
| `crates/codegen/xai-grok-pager/src/app/cli.rs` | clap `about` |
| `crates/codegen/xai-grok-pager/src/app/mod.rs` | TUI title |
| `crates/codegen/xai-grok-pager/src/project_picker/mod.rs` | Project-picker copy |
| `crates/codegen/xai-grok-pager/src/app/dispatch/notes.rs` | Feedback toast |
| `crates/codegen/xai-grok-pager/src/app/dispatch/session/lifecycle.rs` | Compaction copy |
| `crates/codegen/xai-grok-pager/src/settings/defs.rs` | Language setting copy |
| `README.md`, `CHANGELOG.md`, `AGENTS.md`, `docs/KNOWN_ISSUES.md`, `VERSION` | Product + release notes |
| `docs/RC12_SUBAGENT_QA.md` | Live Q&A matrix + results |

Do **not** rewrite archived `docs/archive/**` or historical CHANGELOG entries for past RCs.

---

## Key decisions

1. **Display name = Turbo Build.** Machine id stays `turbo-grok-build`. CLI stays `turbo`. That avoids breaking the Claude Code plugin and `--version --json` consumers.
2. **Isolation honesty is CWD-heuristic.** `context.rs` overwrites `ctx.isolation` via `infer_isolation_label(&tool_cwd)`. Fix the heuristic to match the rc.11.1 start-gate (short root + `GROK_WORKTREE_ROOT` + `.grok/worktrees` + `grok-subagent-worktrees`). Do not hardcode `"worktree"`.
3. **Child card stays tiny** (budget **320** tokens, down from 420). Parent short target **≤1200** tokens (soft cap remains 1650).
4. **Windows path on the parent card.** Today it still says `~/.grok/worktrees/<slug>/…`, which is wrong after rc.11. Teach `{drive}:\t\w\{8hex}\subagent-{id}` plus the env override.

---

### Task 1: Isolation label matches start-gate

**Files:**
- Modify: `crates/codegen/xai-grok-agent/src/prompt/boot_card.rs` (`infer_isolation_label` + tests)
- Test: same file `#[cfg(test)]`

Live bug (rc.11.1 session): child Tool CWD was `H:\t\w\a86e802e\subagent-01a03ec1-…` but boot card said `Isolation claim: isolation=none`. Start-gate already accepted that path. Filed `inc_01a03ec65b1570209969050ffdd630a9`.

- [ ] **Step 1: Write failing tests** (add next to existing boot_card tests)

```rust
#[test]
fn infer_isolation_label_windows_short_root() {
    assert_eq!(
        infer_isolation_label(Path::new(
            r"H:\t\w\a86e802e\subagent-01a03ec1-9732-7770-bde6-b6a0e62098de"
        )),
        "worktree"
    );
    assert_eq!(
        infer_isolation_label(Path::new("h:/t/w/a86e802e/subagent-abc")),
        "worktree"
    );
    assert_eq!(
        infer_isolation_label(Path::new(r"C:\t\w\ffffffff\subagent-xyz")),
        "worktree"
    );
    assert_eq!(infer_isolation_label(Path::new(r"H:\t\w\a86e802e")), "none");
    assert_eq!(
        infer_isolation_label(Path::new(r"H:\t\w\notahex!\subagent-abc")),
        "none"
    );
    assert_eq!(
        infer_isolation_label(Path::new(r"H:\Apps\grok build\turbo-grok-build")),
        "none"
    );
}

#[test]
fn infer_isolation_label_legacy_and_temp_roots() {
    assert_eq!(
        infer_isolation_label(Path::new(
            r"C:\Users\me\.grok\worktrees\repo\subagent-xyz"
        )),
        "worktree"
    );
    assert_eq!(
        infer_isolation_label(Path::new("/tmp/grok-subagent-worktrees/subagent-id")),
        "worktree"
    );
}
```

- [ ] **Step 2: Run tests — expect FAIL** on short-root cases

```
cargo test -p xai-grok-agent --lib infer_isolation_label -- --test-threads=4
```

- [ ] **Step 3: Implement `infer_isolation_label` to match start-gate**

Replace the body of `infer_isolation_label` (keep the public signature). Mirror `path_looks_like_subagent_worktree` in `crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs` (~1508):

```rust
/// Infer isolation label from the real tool CWD path.
///
/// Accepted layouts (must also contain `/subagent-`):
/// - `…/.grok/worktrees/…/subagent-…`
/// - temp `grok-subagent-worktrees/…`
/// - Windows same-volume short root `{drive}:/t/w/{8hex}/subagent-…` (rc.11+)
/// - `$GROK_WORKTREE_ROOT/{8hex}/subagent-…`
pub fn infer_isolation_label(cwd: &Path) -> String {
    if path_looks_like_worktree_cwd(cwd) {
        "worktree".into()
    } else {
        "none".into()
    }
}

fn path_looks_like_worktree_cwd(path: &Path) -> bool {
    let s = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if !s.contains("/subagent-") {
        return false;
    }
    if s.contains("/.grok/worktrees/")
        || s.contains("/grok/worktrees/")
        || s.contains("grok-subagent-worktrees")
    {
        return true;
    }
    if short_volume_worktree_path(&s) {
        return true;
    }
    grok_worktree_root_override_path(&s)
}

fn short_volume_worktree_path(normalized: &str) -> bool {
    let mut rest = normalized;
    while let Some(i) = rest.find("/t/w/") {
        let after = &rest[i + 5..];
        if after.len() >= 8 {
            let hash = &after[..8];
            if hash.bytes().all(|b| b.is_ascii_hexdigit())
                && after[8..].starts_with("/subagent-")
            {
                return true;
            }
        }
        rest = &rest[i + 5..];
    }
    false
}

fn grok_worktree_root_override_path(normalized: &str) -> bool {
    let Ok(root) = std::env::var("GROK_WORKTREE_ROOT") else {
        return false;
    };
    let root = root
        .replace('\\', "/")
        .trim()
        .trim_end_matches('/')
        .to_ascii_lowercase();
    if root.is_empty() {
        return false;
    }
    normalized == root || normalized.starts_with(&format!("{root}/"))
}
```

Do **not** depend on `xai-grok-shell` from `xai-grok-agent` (cycle risk). Duplicating the ~40-line detector is the accepted rc.12 shape. A later PR can move it to `xai-grok-workspace`.

- [ ] **Step 4: Re-run tests — expect PASS**

```
cargo test -p xai-grok-agent --lib infer_isolation_label -- --test-threads=4
cargo test -p xai-grok-agent --lib -- boot_card -- --test-threads=4
```

- [ ] **Step 5: Commit** `fix(isolation): boot card recognizes Windows short worktree CWD`

---

### Task 2: Densify child + parent boot cards

**Files:**
- Modify: `crates/codegen/xai-grok-agent/src/prompt/boot_card.rs` (`render_child`, `render_short`, tests `child_is_tiny`)

Token problem: parent **short** card repeats ADL/FRL as long prose after already listing the tools. Isolation section still teaches `~/.grok/worktrees/` only. Child card teaches `.grok/worktrees/…` and can **lie** (`isolation=none` on a short-root CWD) until Task 1 lands.

- [ ] **Step 1: Tighten `child_is_tiny` and add honesty tests**

Change the child budget from `420` to `320`. Add:

```rust
#[test]
fn child_card_windows_short_root_says_worktree() {
    let cwd = r"H:\t\w\a86e802e\subagent-01a03ec1-9732-7770-bde6-b6a0e62098de";
    let ctx = BootCardContext {
        model: "grok-4.6".into(),
        isolation: infer_isolation_label(Path::new(cwd)),
        cwd: cwd.into(),
        ..Default::default()
    };
    let card = render_boot_card(BootCardMode::Child, &ctx).unwrap();
    assert!(card.text.contains("isolation=worktree"), "{}", card.text);
    assert!(!card.text.contains("isolation=none"));
    assert!(card.token_estimate <= 320, "child tokens={}", card.token_estimate);
}

#[test]
fn short_card_mentions_windows_short_root() {
    let ctx = BootCardContext {
        isolation: "worktree".into(),
        os: "windows".into(),
        ..Default::default()
    };
    let card = render_boot_card(BootCardMode::Short, &ctx).unwrap();
    assert!(
        card.text.contains(r"{drive}:\t\w\") || card.text.contains("/t/w/"),
        "short card must teach Windows short worktree root: {}",
        card.text
    );
    assert!(
        card.token_estimate <= 1200,
        "short card tokens={} (target ≤1200, cap 1650)",
        card.token_estimate
    );
}
```

- [ ] **Step 2: Rewrite `render_child`**

Keep Nested spawn / Tool CWD / DisplayCwd remap / developer_log / model. Drop the outdated `.grok/worktrees` exclusive path. New body:

```rust
fn render_child(ctx: &BootCardContext) -> String {
    let nested_spawn = if ctx.spawn_tool_present {
        "Nested spawn: yes."
    } else {
        "Nested spawn: disabled at max depth — do not call spawn_subagent."
    };
    format!(
        "You are a Turbo subagent. Isolation claim: isolation={isolation}.\n\
         Tool CWD (real FS): `{cwd}`.\n\
         DisplayCwd/Get-Location may show the parent — remap, not isolation_fallback. Do not refuse.\n\
         - isolation=worktree: write here (`{{drive}}:\\t\\w\\{{hash}}\\subagent-…` or ~/.grok/worktrees/…/subagent-…). Never Copy-Item into the parent.\n\
         - isolation=none or isolation_fallback=true: shared parent CWD.\n\
         {nested_spawn} Product bugs → developer_log. Missing capability → feature_request_log.\n\
         Model: {model}",
        isolation = ctx.isolation,
        cwd = ctx.cwd,
        nested_spawn = nested_spawn,
        model = ctx.model,
    )
}
```

- [ ] **Step 3: Densify `render_short` isolation + ADL/FRL**

Replace the Subagents block (the `## Subagents (orchestrator)` section) with:

```
## Subagents (orchestrator)
- isolation=worktree (default) → child Tool CWD is a product worktree: Windows `{drive}:\t\w\{8hex}\subagent-{id}`; else `~/.grok/worktrees/<slug>/subagent-{id}`; override `$GROK_WORKTREE_ROOT`
- isolation=none → shares parent
- Prove isolation with completion tags: worktree_path · <isolation>worktree</isolation> · isolation_fallback absent/false. DisplayCwd may still be the parent (remap).
- Do not edit parent paths a RUNNING worktree child owns. Seed=clean (HEAD only) unless GROK_SUBAGENT_WORKTREE_SEED=dirty
- Keep-N=3 (GROK_SUBAGENT_KEEP_N) · free gate GROK_MIN_FREE_GB=40 · retain_worktree=true keeps the live tree
- Land via land_subagent / `{bin} subagent land` — never Copy-Item/cp from the worktree
```

Shrink ADL to:

```
## Auto Developer Log (REQUIRED)
- Call `developer_log` for Turbo product bugs/friction (one call per issue; store dedups).
- Required: title, summary, error_class (worktree_tombstone|isolation_fallback|work_lost_risk|subagent_stall|protocol_deser|provider_400|provider_429|feature_gap|docs_gap|land_conflict|unknown)
- Root: {dir} · `{bin} issues list|export|path`
```

Shrink FRL similarly (title, summary, request_class, root, `{bin} features list`).

Keep meeting/schedule/browser lines (they are the only launch facts). Drop duplicated "ALWAYS file product issues" from Operating rules if ADL section remains.

- [ ] **Step 4: Run tests**

```
cargo test -p xai-grok-agent --lib -- boot_card -- --test-threads=4
```

Expected: all boot_card tests PASS; `child_is_tiny` ≤320; short ≤1200.

- [ ] **Step 5: Commit** `fix(boot-card): honest isolation label and denser subagent briefing`

---

### Task 3: Product display name "Turbo Build"

**Files:** listed in Files table (pager + version + README/AGENTS/CHANGELOG header).

Keep:
- CLI binary `turbo`
- `--version --json` `"product": "turbo-grok-build"`
- Repo name, `~/.grok`, `~/.turbo`
- Historical CHANGELOG section titles for **already-shipped** RCs

- [ ] **Step 1: Add constant** in `crates/codegen/xai-grok-version/src/lib.rs`

```rust
/// User-facing product name for community Turbo builds.
/// Machine identity (`--version --json` `product`) remains `turbo-grok-build`.
pub const PRODUCT_DISPLAY_NAME: &str = "Turbo Build";
```

Add a one-liner test that the constant equals `"Turbo Build"`.

- [ ] **Step 2: Wire display strings**

| Location | Old | New |
|----------|-----|-----|
| `welcome/mod.rs` `"Turbo Grok Build  "` | `"Turbo Build  "` |
| `welcome/mod.rs` `"Turbo Grok Build Beta  "` | `"Turbo Build Beta  "` |
| `app/cli.rs` `about = "Turbo Grok Build TUI"` | `"Turbo Build TUI"` |
| `app/mod.rs` `"Turbo Grok Build TUI"` | `"Turbo Build TUI"` |
| `project_picker/mod.rs` both sentences | "Turbo Build" |
| `notes.rs` "The Turbo Grok Build team" | "The Turbo Build team" |
| `lifecycle.rs` "Turbo Grok Build will check" | "Turbo Build will check" |
| `settings/defs.rs` description | "Turbo Build is English-only." |

Prefer `xai_grok_version::PRODUCT_DISPLAY_NAME` in Rust user-facing strings when a format/concat is easy; clap `about` may stay a literal if the crate already depends on version.

- [ ] **Step 3: Docs (current product only)**

- `README.md` H1, intro, comparison table **Product** column, badge alt text
- `AGENTS.md` title + "Product:" line
- `CHANGELOG.md` intro line + pedigree row for **1.0 rc12**
- `docs/KNOWN_ISSUES.md` title "Turbo known issues" can stay; add rc.12 section later in Task 5

Do not churn `docs/archive/**`.

- [ ] **Step 4: Tests**

```
cargo test -p xai-grok-version --lib -- --test-threads=4
cargo test -p xai-grok-pager --lib -- welcome -- --test-threads=4
```

Fix any snapshot/string assertions. As of rc.11.1, no test file asserted the old display name.

- [ ] **Step 5: Commit** `feat(brand): user-facing product name is Turbo Build`

---

### Task 4: Wire version, changelog, known issues

**Files:** `VERSION`, `CHANGELOG.md`, `docs/KNOWN_ISSUES.md`, `README.md` version badges if they pin `1.0.0-rc.11.1`

- [ ] Set `VERSION` to `1.0.0-rc.12`
- [ ] Pedigree table: `**1.0 rc12** | **1.0.0-rc.12** | Subagent hardening + Turbo Build rename`
- [ ] New CHANGELOG section (template):

```markdown
## [1.0.0-rc.12] - 2026-08-26

**Subagent hardening + Turbo Build.** Child boot cards tell the truth about
Windows short worktrees (`{drive}:\t\w\{hash}\subagent-…`). Parent boot card
teaches that path and drops duplicated ADL/FRL prose. User-facing product
name is **Turbo Build**; CLI remains `turbo`.

#### Subagents
- Boot-card `infer_isolation_label` accepts the rc.11 short root and
  `$GROK_WORKTREE_ROOT` (same patterns as the start-gate).
- Child card budget 320 tokens; parent short target ≤1200.

#### Brand
- Display name **Turbo Build**. Machine id `turbo-grok-build` unchanged.
```

- [ ] KNOWN_ISSUES: last reviewed 1.0.0-rc.12; note boot-card isolation is now the start-gate detector (short root included). Residual: detector is still a path heuristic, not spawn metadata.

- [ ] Commit `release(1.0.0-rc.12): subagent hardening, Turbo Build name`

Do **not** tag or push.

---

### Task 5: Live Q&A (orchestrator, Windows)

Use `docs/RC12_SUBAGENT_QA.md`. After Tasks 1–2 are landed on `rc12`, the parent session must:

1. Spawn `isolation=worktree` `cwd=H:\Apps\grok build\turbo-grok-build` `retain_worktree=true`.
2. Child writes `ISOLATION_PROBE_RC12.txt` only in Tool CWD.
3. Parent asserts: worktree under `H:\t\w\{8hex}\subagent-…`, parent repo clean, completion `isolation=worktree` / `isolation_fallback` false.
4. Read child `system_prompt.txt` turbo_boot_card: **must** say `isolation=worktree` (ISO-10). This is the rc.11.1 residual.
5. Explore default: `isolation=none`, no worktree.
6. Non-git umbrella spawn `isolation=worktree`: fail-closed, no shared fallback.
7. Record token_estimate from child card (session system_prompt or unit test output).

Fill the Results table in `docs/RC12_SUBAGENT_QA.md`. Any fail → developer_log + fix on `rc12`, re-test. Loop until the P0 rows are green.

---

## Test commands (disk-safe)

```powershell
cargo test -p xai-grok-agent --lib -- boot_card -- --test-threads=4
cargo test -p xai-grok-agent --lib infer_isolation_label -- --test-threads=4
cargo test -p xai-grok-version --lib -- --test-threads=4
cargo test -p xai-grok-pager --lib -- welcome -- --test-threads=4
```

Do **not** `cargo test --workspace`. Do **not** `release-dist` until the human asks to ship.

---

## Spec coverage

| Requirement | Task |
|-------------|------|
| Child boot card isolation=worktree on `H:\t\w\…` | 1, 5 ISO-10 |
| Start-gate still fail-closed outside git | 5 |
| Token savings child ≤320, short ≤1200 | 2 |
| Parent card teaches Windows short root | 2 |
| Display name Turbo Build | 3 |
| Machine id / CLI / paths unchanged | 3 (explicit keep) |
| VERSION 1.0.0-rc.12 | 4 |
| Live Q&A loop | 5 |
