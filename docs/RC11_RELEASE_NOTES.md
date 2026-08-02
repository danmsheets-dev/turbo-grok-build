# Turbo RC11 — Release notes (in progress)

**Base:** 0.2.114-r10  
**Focus:** Game Mode pixel office + RC10/RC11 harness incident fixes (2026-08-02)

---

## Game Mode

| Item | Detail |
|------|--------|
| Toggle | **Ctrl+G** (tasks pane: **Ctrl+Shift+G**) |
| Visual | Mockup + badge/walk overlays at **cell resolution** (no PNG hot path); halfblock paint |
| Layout | Game Mode hides top banners/tips/status chrome — office + chat only |
| Data | Live `subagent_sessions` → desk badges / walk / wall ribbon |
| Composer | Stays open → Supervisor (main agent) |
| Animation | `TickDemand::Slow` while open (~8–10 Hz recompose) |
| Resize | Compact / Normal / Comfort / Wide; Compact uses card/Unicode fallback |
| Playground | `cargo run -p xai-grok-pager --bin game-mode-playground` |

Spec: `docs/design-game-mode-rc11.md`  
Plan: `docs/superpowers/plans/2026-08-02-game-mode-rc11.md`

---

## Round-2 harness Q&A (r10) → RC11 disposition

Source report: `results/harness-qa-r10-round2-20260802/TURBO_RC10_ROUND2_QA_REPORT.md`  
Session: `019fc3aa-a985-73c2-ab5a-1cf4cb46500c` · product `0.2.114-r10` · ADL under `developer_log/incidents/2026-08-02/`.

### Round-2 matrix (already healthy on r10 — keep / no code)

| Area | Round-2 | RC11 |
|------|---------|------|
| Nemotron / extra NVIDIA tiny probes | PASS (most) | Keep |
| Concurrent multi-agent worktrees | PASS | Keep |
| capability_mode RO / RW | PASS | Keep |
| isolation=none parent write | PASS | Keep |
| timeout_ms cancel | PASS | Keep |
| Invalid model fail-closed | PASS | Keep |
| Child developer_log | PASS | Keep |
| Agent-only diff/land (when baseline present) | PASS | Keep + hardened |
| Stock deep-audit (models=0 inherit) | PASS ~6 min | Keep; optional multi-model pin retest |
| Oracle Ultra pin read-only smoke | PASS | Keep (pin Grok for tool oracle until Ultra agent-ready) |

### Round-2 P1/P2 defects

| ID | Round-2 result | RC11 status | Notes |
|----|----------------|-------------|-------|
| **allowed_paths land** `inc_…0357c91…` | **FAIL** (outside path merged twice) | **Fixed (land/diff)** | CLI land/diff filter; patch refuse; tool already refused. **Write-time still not blocked** (by design / boot card: land-diff only). |
| **resume_from baseline loss** `inc_…d3f17143…` | **FAIL** (no baseline_ref, 554-file inflate) | **Fixed** | Fresh baseline at resume start; inherit source allowlist. |
| **Resume land conflicts** `inc_…f2767960…` | **FAIL** (dirty-parent merge) | **Fixed** (same as resume baseline) | Agent-only baseline..snap path; conflict stderr truncated with baseline hint. |
| **Super empty snapshot** `inc_…aef02a7c…` | **FAIL** intermittent under concurrency | **Mitigated** | `LocalFs::write_file` post-write exist+size verify + one retry; refuse success if still missing. Land prints `files_landed`. Concurrent stress still for retest. |
| **MiniMax (and peers) post-tool hang** `inc_…b473fb73…` | **PARTIAL** (write OK, stall to timeout; land recovered) | **Deferred** | Provider post-tool silence; land-from-cancelled still works. Stall detector = post-RC11. |

### Feature Request Log (RC11)

Enterprise twin of Auto Developer Log for **missing product surface**:

| Piece | Detail |
|-------|--------|
| Tool | `feature_request_log` (all default toolsets + explore) |
| CLI | `turbo features list\|show\|export\|ack\|plan\|ship\|decline\|set-dir\|file` |
| Store | `$GROK_HOME/feature-request-log/` (env `GROK_FEATURE_REQUEST_LOG_DIR`) |
| Docs | `docs/FEATURE_REQUEST_LOG.md` |
| Boot card | Instructs: bugs → `developer_log`; missing capability → `feature_request_log` |

**Hold for install:** RC11 binary may be built; do not install until Q&A says so.

### Fixed in RC11 (code)

| Incident | Class | Fix |
|----------|-------|-----|
| **allowed_paths not enforced at land** | land_conflict P1 | CLI `turbo subagent land/diff` filters by `meta.allowed_paths` (snapshot + live worktree); patch land **refuses** if any path outside allowlist. Tool-side `land_subagent` already refused. |
| **resume_from loses baseline_ref / land bulk** | work_lost_risk / land_conflict P1 | On resume Reuse/Rehydrate, capture **fresh** `refs/grok/subagent-baselines/<new-id>` before agent runs; fall back to source meta baseline. Resume inherits source `allowed_paths` when omitted. |
| **Worktree .git missing HEAD object** | worktree_tombstone P2 | Standalone isolation worktrees write `.git/objects/info/alternates` → parent objects (after create). |
| Stock deep-audit `models` not found | unknown P1 | Already fixed in `deep_audit.rhai` (`model_for_index(models, idx)`); retest PASS logged. |
| Shift+G steals vim GotoBottom | feature_gap P2 | **Ctrl+G** (tasks: Ctrl+Shift+G). |
| tick_demand for animated views | docs_gap P3 | Game Mode registers `TickDemand::Slow`. |
| capability_mode RO strips write | verified | No further code (r10). |
| open/diff/land agent-only baseline | verified | Extended by resume + allowlist land above. |

### Deferred / out of scope for RC11 core

| Incident | Class | Why deferred |
|----------|-------|--------------|
| MCP connect failures (blender, guardian, resend, sentry, sourcegraph) | mcp_connect P2 | Environment / credentials / host install — not CLI ship blockers. |
| Godot MCP port 9080 bind during `--import` | mcp_connect P3 | External Godot MCP single-instance; document don’t run dual MCP. |
| Nemotron Super write success / empty snapshot | work_lost_risk P1 | Needs write-tool fsync verification + concurrent-load repro; not a one-line land fix. |
| MiniMax-m3 hang until timeout_ms | subagent_stall P2 | Provider/model stall; keep timeout_ms; no harness root cause yet. |
| Windows shimmer on port builders | unknown P2 | **Pirates** art/shader (not Turbo). |
| Scheduler keep-N-workers for art loops | feature_gap P2 | Product feature (scheduler); post-RC11. |
| Manual multi-agent art land / hull.json merge | feature_gap / land_conflict P2 | **Pirates** content workflow; use allowlist + single-writer hull or merge tool later. |
| Turbo isolation blocks `git checkout sha -- paths` | isolation_fallback P2 | Expected with standalone clones; land via `turbo subagent land` / snapshot refs, not raw checkout across trees. |
| Sprint docs accidental PORT-LORE rewrite | work_lost_risk P2 | Operator git hygiene (`git add` pathspecs); process note only. |
| Session-start / probe / retest PASS logs | unknown p3 | Informational — not defects. |

---

## How to try Game Mode in full Turbo

```powershell
cd "H:\Apps\grok build\hyper-grok-build"
cargo run -p xai-grok-pager-bin --bin turbo
```

1. Open / create a session  
2. **Ctrl+G** → pixel office  
3. Spawn subagents → desks fill  
4. Type in the composer as usual  
5. **Ctrl+G** again → Normal  

Maximize the terminal for halfblock quality.

---

## Harness recovery (land / resume)

```text
# Agent-only review (requires baseline_ref in meta.json)
turbo subagent open <id>
turbo subagent diff <id>          # baseline_snapshot + optional allowed_paths filter
turbo subagent land <id>          # refuses bulk without baseline; filters allowlist

# Resume continues the same worktree; new child gets a fresh baseline at start
# so land only applies edits from the resume turn (not dirty-parent bulk).
```

---

## Known limitations (RC11)

- Halfblock quality is chunky (not Kitty-native yet).  
- Desk anchors are approximate vs mockup (tune in `compose.rs` DESK_ANCHORS).  
- No desk click → open subagent yet (spectator).  
- MCP third-party connect failures on Windows remain environmental.  
- Write-tool “success but empty snapshot” under multi-provider load still under investigation.

---

## Verification

```text
cargo test -p xai-grok-pager --lib views::game_mode
# + subagent_cmd allowlist unit tests

cargo test -p xai-fast-worktree --lib worktree::execute
# objects alternates + marker tests

# targeted (when linking allows):
cargo test -p xai-grok-shell --lib agent::subagent
```
