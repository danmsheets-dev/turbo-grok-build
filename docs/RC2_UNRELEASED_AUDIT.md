# RC2 Unreleased Audit

**Scope:** `860e8817a..HEAD` — the 15 unreleased commits on top of the tagged
`1.0.0-rc.2` line. 104 files, +13,927 / −337.
**Branch:** `dev` == `sync/1.0.0-rc1` == `2d63aee0b` (identical; there is no
separate sync worktree).
**Date:** 2026-08-18.
**Method:** 47 agents in two passes — 9 area-scoped finders, adversarial
refutation of every high/medium finding, a completeness critic, then a
payload-level adjudication round for the disputed security items. Every severity
below survived at least one attempt to refute it.

Prior art: commit `2d63aee0b` already remediated 34 findings from the RC2 Agent
WebView audit. Everything here is **new relative to that pass**.

---

## Baseline

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | **green** (5 dead-code warnings, all in test code) |
| `cargo test --workspace --lib` | 27,576 passed / 7 failed / 56 ignored — **none of the 7 are from this range** |
| `cargo test --workspace` (all targets) | **unusable** — hangs indefinitely in the PTY harness |
| `TODO`/`FIXME`/`unimplemented!` added in range | none |
| Hardcoded developer paths in shipping code | **1** — see A2 |

---

## Release readiness

**Verdict: not ready.** The optimized build itself is fine; the blockers are A1,
A2, and an unresolved version collision.

### The production build passes

`cargo build --profile release-dist -p xai-grok-pager-bin --features community-build --locked`
— the exact command from [`release.yml`](../.github/workflows/release.yml) — succeeds
on `x86_64-pc-windows-msvc`:

| | |
|---|---|
| Exit | 0 |
| Time | 31 m 56 s (thin LTO, `codegen-units=1`) |
| Binary | `target/release-dist/turbo.exe`, **142.7 MiB** |
| CI gate | `MAX_RELEASE_BINARY_BYTES` = 256 MiB — **passes with headroom** |

Warnings under this profile are cosmetic and none are in this range: 29
pre-existing `unreachable pub` in `xai-grok-shell`; a test-only re-export of
`BROWSER_WEBVIEW2_RUNTIME_ID` in `diagnostics/mod.rs:37` (the probe itself is
wired up in `diagnostics/browser.rs`); and a dead `check_ssrf` test wrapper — the
real `resolve_and_check_ssrf` **is** called from `web_fetch/client.rs:573`, so
SSRF protection is intact.

Only the Windows target was built locally. CI builds five.

### Two configurations are shipped but under-tested

- `--release` is **not** the distribution profile; `release-dist` is. Anything
  validated only under `--release` has not been through LTO or `codegen-units=1`.
- The shipped binary carries `--features community-build`, which nothing in this
  audit or the 27,576-test `--lib` run exercised. It is not cosmetic: it gates
  behavior in `app/dispatch/billing.rs:280` and compiles several tests out via
  `#[cfg(not(feature = "community-build"))]`, so the shipping configuration is
  *less* covered than the default one.

### The version is already taken

| Fact | Value |
|---|---|
| `VERSION` | `1.0.0-rc.2` |
| CHANGELOG | `## [1.0.0-rc.2] - 2026-08-11` exists as a **released** section |
| This range | sits above it under `## [Unreleased]` |
| Latest tag | **`v1.0.0-rc.1`** — `v1.0.0-rc.2` was never tagged |

`release.yml` fires on `v*` and hard-fails unless the tag matches `VERSION`.
Pushing `v1.0.0-rc.2` therefore *passes* that gate while publishing a binary that
self-reports `1.0.0-rc.2` but contains 15 commits the Aug-11 rc.2 entry does not
describe. Because rc.2 was never tagged, either resolution is clean: fold
`[Unreleased]` into the rc.2 section and tag it, or bump to rc.3. It must be
decided before any tag push.

---

## P0 — release blockers

### A1. `--confine` write-boundary escape via the `cmd /c` peel

`crates/codegen/xai-grok-workspace/src/permission/shell_access.rs:424` ·
introduced by `9ca2468fd` in this range · **regression**

`analyse_shell_tree` iterates shell invocations; when any invocation's program
normalizes to `cmd`, it hands the **entire source string** to
`try_analyse_windows_engine_invocation` and returns its `Some(..)` immediately.
Two consequences compound:

1. The early return means **sibling invocations in a compound command are never
   classified**. `powershell -c ...` would otherwise be rejected by
   `powershell_file_is_modelled_for_confine` (`:1088`) and fail closed.
2. `tokenize_windows_cmdline` (`:667`) is a flat argv splitter with **no notion
   of `;`, `&&`, `||`, `|`, or newline**, and operands come only from
   `densify_engine_path_operands`, which harvests values after a fixed
   engine-flag allowlist.

Working payload under `--confine` rooted at a worktree:

    cmd /c blender.exe -b ; powershell -c "Set-Content C:\outside\pwn.ps1 x"

Analysis returns `Modelled { operands: ["Set-Content C:\\outside\\pwn.ps1 x"] }`.
The real write target is **glued inside one multi-word token**, so it is not a
standalone operand. The crux was verified directly with `rustc` on Windows:

    operand="Set-Content C:\\outside\\pwn.ps1 x"
      is_absolute=false  first=Some(Normal("Set-Content C:"))
      joined="H:\\worktree\\root\\Set-Content C:\\outside\\pwn.ps1 x"
      under_cwd=true          <-- no ConfineHit, command permitted

Because the token is not absolute, `cwd.join()` rebases it *under* the root and
`path_is_under_confine_root` returns true. The command runs; PowerShell writes
outside the root. The engine need not exist on disk — classification is by name
only, and `;` runs the tail regardless. Generalizes to arbitrary code execution.

**Why the other shapes are caught:** `> C:\outside\out.txt`,
`; del /q C:\important\*`, and `curl -o C:\outside\z.exe` all surface the path as
a *clean absolute token*, which is correctly denied — by accident, not by design
(`-o` happens to be in the densify value-flag allowlist).

This defeats the property the code itself documents as "isolation=worktree
children cannot shell-escape (C1)". Before `9ca2468fd`, `cmd /c ...` was
`Unparseable` and denied.

**Fix.** Gate the whole-src recovery on `shell_command_invocations(...).len() == 1`,
or pass only that invocation's own words, merge its operands, and **continue** the
loop instead of returning. Independently, fail closed on any candidate operand
containing an interior absolute-path fragment (` [A-Za-z]:\` or ` /`) rather than
treating the concatenation as one relative subpath.

### A2. `disk clean` hardcodes `H:\gb` / `H:\gb-work` and deletes their subdirectories

`crates/codegen/xai-grok-pager/src/disk_cmd.rs:642` ·
`reclaim_plugin_worktrees` at `:1810`

`plugin_worktree_roots()` appends two hardcoded developer paths on Windows.
`reclaim_plugin_worktrees` then `remove_dir_all`s **every** subdirectory under
them that is older than the cutoff and lacks a `.grok-subagent-live` marker —
with **no name filter**, unlike the `worktrees` category which is scoped to
`subagent-*`.

Any user with an `H:` drive and a folder named `gb` loses its contents.

*Severity corrected from the finder's "high" claim that `--safe` reaches it:*
this category is **not** in `default_safe()`
(`[Debug, Worktrees, TreeStore, TempGrok]`), so it requires an explicit
`--include plugin-worktrees`. Still P0 here, because shipping a release binary
that deletes from a hardcoded path on someone else's machine is not recoverable
by the user.

**Fix.** Delete the `#[cfg(windows)]` candidate block; resolve roots only from
`GROK_BUILD_WORKTREE_ROOT` / `GROK_PLUGIN_WORKTREE_ROOT` or a product-owned path
under `GROK_HOME`. Add a `subagent-*`-style name filter. Update the enum doc at
`:59`.

---

## High

### B1. `disk clean --safe` deletes unrelated applications' temp directories

`crates/codegen/xai-grok-pager/src/disk_cmd.rs:2109`

`TempGrok` **is** in `default_safe()`, so a bare `turbo disk clean --safe` — what
the low-space warning tells users to run — reaches it.
`is_temp_harness_leftover_name` matches generic patterns including `tmp.` and
`.tmp*`. `tmp.XXXXXXXX` is the standard `mktemp -d` output name and `.tmp*` is a
common `tempfile` prefix, so any unrelated app's >24h-old temp directory is
deleted. `uid-`, `kg-`, `goal-`, `real-target` are similarly generic.

**Fix.** Keep only unambiguous product-owned prefixes (`grok-`, `nest-`,
`turbo-rc15`, `gh-export-test-`); drop the generic ones, or require an ownership
marker file inside the directory.

### B2. Linked-worktree clean modes silently discard the requested `--ref`

`crates/codegen/xai-fast-worktree/src/worktree/execute.rs:1063`

`WorkingTreeMode::CleanTracked` / `CleanAll` reset to
`git::get_head_commit(source)` rather than the requested ref, so `--ref` is
accepted and ignored. The user gets a worktree at the wrong commit with no error.

### B3. Unconditional 60s first-progress kill cancels healthy subagents

`crates/codegen/xai-grok-shell/src/agent/subagent/mod.rs:2049`

The clock counts wall time during which `tool_call_count`, `tokens`, and
`model_calls` are all necessarily 0 — including `wait_for_mcp_initialized` under
`McpInitStrategy::Blocking`. A workspace with an `npx` stdio MCP server and a
cold package cache blows the budget before the child's first model call.

**Fix.** Arm the clock when the prompt turn is acknowledged, and exclude the
MCP-init / tool-preparation interval.

### B4. Named-pipe instance pool is used LIFO, stranding four listeners

`crates/codegen/xai-grok-browser/src/host/rpc.rs:147`

The pool refills by pushing and serves by popping from the same end, so four
created instances are never armed. A client the OS attaches to one of them blocks
in `read_line` forever — no `UiJob` is queued and no reply is sent. Two
concurrent `browser_*` calls, or the Ctrl+Shift+B pane refresh racing a tool
call, is enough to hit it.

**Fix.** `VecDeque` with `push_back`/`pop_front` at minimum; better, arm
instances eagerly.

---

## Medium

| # | Location | Issue |
|---|---|---|
| C1 | `xai-grok-browser/src/client.rs:279` | Spawned host never receives `--session-folder`, so every `file:` URL the client permits is rejected host-side — the feature is dead on arrival |
| C2 | `xai-grok-browser/assets/turbo_ax.js:105` | Snapshot returns values of OTP and credit-card fields that the **fill** policy already classifies as secrets |
| C3 | `xai-grok-browser/src/host/webview.rs:330` | `eval_in_world` retries on `Err(_)`, re-dispatching a non-idempotent click after a CDP timeout — double-submit |
| C4 | `xai-grok-tools/.../browser/tabs.rs:71` | `browser_tabs` / `browser_navigate` return page-controlled text without the untrusted-content banner that `snapshot` uses |
| C5 | `xai-grok-shell/src/session/agent_browser.rs:219` | Host orphaned on every force-exit path (second Ctrl+C, exit timeout, panic hook) — not enrolled in the process scope |
| C6 | `xai-grok-tools/.../subagent_worktree/mod.rs:317` | Snapshot-ref fallback fabricates an empty meta, dropping the `allowed_paths` land allowlist entirely |
| C7 | `xai-grok-pager/src/disk_cmd.rs:1481` | Cargo profile lock covers only the primary `target/`, but deletion fans out to every discovered nested `target/` |
| C8 | `xai-grok-shell/src/session/managed_mcp.rs:142` | Disk-wins `or_insert` silently discards credentials/env an external ACP client passed at `session/new` |
| C9 | `xai-grok-shell/src/agent/models.rs:170` | Luna alias table hijacks exact fully-qualified catalog keys and ignores availability |
| C10 | `xai-grok-models/src/platforms.rs:2541` | Bare Nemotron Lightning row sends an unroutable wire id and shadows the correct vendor-prefixed row |
| C11 | `xai-grok-browser/src/client.rs:235` | Snapshot node values never length-capped — one page drives unbounded allocation |
| C12 | `xai-grok-workspace/.../shell_access.rs:630` | Neither Windows recovery route models redirects. The spaced `> C:\path` form is caught by accident; the **glued** `>C:\path` form (valid in cmd and PowerShell) tokenizes as one non-absolute operand and escapes — same mechanism as A1 |

---

## Low

- `xai-grok-tools/.../browser/snapshot.rs:90` — no byte cap; values cached
  forever in a process-global map.
- `xai-grok-mcp/src/servers.rs:4458` — respawn drains two stderr pipes into the
  same truncating log, corrupting what `turbo mcp doctor` reads.
- `xai-grok-pager/src/app/agent_view/render.rs:1344` — Ctrl+Shift+B inside Game
  Mode focuses a pane that is never rendered.
- `xai-grok-pager/src/app/agent_view/panes.rs:522` — the "mirror" never refreshes
  as the agent browses; it is a manual-refresh snapshot. Discoverability gap, not
  a perf bug — the no-blocking-IO-on-the-UI-thread design is deliberate and correct.
- `xai-grok-pager/src/app/agent_view/panes.rs:556` — `refresh_browser_pane`
  replaces the line list without clamping `scroll`; pane goes blank.
- `xai-grok-pager/src/views/agent.rs:555` — "N more" footer overwrites the last
  snapshot line and undercounts hidden rows by one.
- `xai-grok-tools/.../browser/eval.rs:70` — the `mutates_page` write-gate is a
  30-needle lowercased substring scan, defeated by `el["inner"+"HTML"]=...`,
  `window["fe"+"tch"](...)`, or `Function(...)()`. **Low, not high**:
  `browser_eval.function` is populated only by the model's own tool-call
  arguments, and no path feeds page text into it automatically, so this is a
  guard-rail against the model's mistakes, not a security boundary. Still worth
  fixing because evasion also skips snapshot-cache invalidation.
- `xai-grok-shell/src/session/agent_browser.rs:153` — `browser_*` tools and the
  `<browser_verification>` prompt block are injected on macOS/Linux where every
  call fails Windows-only. Wasted tokens plus confusing errors.
- `xai-grok-shell/src/agent/subagent/mod.rs:3638` —
  `~/.grok/subagent-artifacts/<id>/` grows monotonically; no `turbo disk`
  category reclaims it.
- `xai-grok-agent/src/prompt/browser_verification.rs:24` — `synthetic_user_rules`
  is new dead public API with no call site.

---

## Refuted

Recorded so they are not re-litigated. Each was reported by a finder and killed
by an independent verifier that read the code and its callers.

| Claim | Why it does not hold |
|---|---|
| JSON-RPC responses accepted without checking the correlation id | The transport opens a fresh pipe connection per call and reads exactly one line; there is no multiplexing to mismatch |
| `shutdown_browser_host` burns half the 10s session-exit grace | Shutdown is bounded well under the grace and runs after memory save |
| `turbo mcp restart` never restarts anything | The two config writes are observed by the watcher; they do not cancel out |
| `<browser_verification>` is unconditional and wrong on non-Windows | The block is genuinely gated on the finalized toolset; the cross-platform injection issue is real but separate (see Low) |
| `JOB_OBJECT_LIMIT_BREAKAWAY_OK` weakens kill-on-close containment | Flag exists as cited, but the stated mechanism is false — it does not weaken `KILL_ON_JOB_CLOSE` |
| `ensure_browser_host` pipe pre-check allows same-user channel hijack | Code behaves as described, but the security conclusion does not survive the actual threat model |
| Windows engine recovery ignores redirects (`> C:\outside\file`) | The *literal* payload is refuted — that form yields a clean absolute operand and is denied. The mechanism is real in the **glued** `>C:\path` form; retained as C12 |

---

## Not ours

### The PTY harness hangs `cargo test --workspace` indefinitely

`plan_approval_restored_after_resume`
(`crates/codegen/xai-grok-pager-pty-harness/tests/plan_approval_resume.rs`) wedged
a full-target test run for **6.6 hours** before it was killed.

Observed sequence, from process creation times:

1. Test binary starts. `PAGER_BINARY` is unset, and `CARGO_BIN_EXE_turbo` is set
   only for the package that *defines* the binary (`xai-grok-pager-bin`), not for
   this harness package — so `pager_binary()` falls through to resolution step 3
   and shells out to a **nested `cargo build`** against the same target directory
   the outer run is using. That burned 8.5 minutes and rebuilt `target/debug/turbo.exe`.
2. ConPTY spawns (`conhost --headless --width 120 --height 50`).
3. The `turbo` child dies. `conhost.exe` enters a busy-spin — measured at
   **23,781 s of CPU**, a full core — while the harness blocks forever and the
   test binary sits at 0.06 s CPU.

Nothing times out because the scenario's 5–30 s timeouts wrap only
`wait_for_text`, not spawn or teardown.

This is not RC2 code — the harness has zero files in `860e8817a..HEAD` and was
last touched by upstream sync commits `3af4d5d39` / `c68e39f60`. It is latent
because **CI never runs it**: `.github/workflows/keep-features.yml` only issues
narrow `cargo test -p <crate> --lib <filter>` commands, so `--workspace` across
all targets is an unexercised entry point.

Note the orphaned-child shape is the same failure class as **C5**.

**Use `cargo test --workspace --lib` for a full-suite signal** until the harness
is fixed or the test is marked `#[ignore]`.

### The 7 `--lib` failures, none from this range

| Test | Verdict |
|---|---|
| `auth::recovery::…::dispatch_external_binary_uses_refresh_chain` | **Parallel lock contention** on `auth.json.lock` — passes single-threaded |
| `session::slash_commands::…::parse_skill_refs_multi_skill` | **Real, pre-existing bug** (below) |
| `session::slash_commands::…::workflow_collision_policy_includes_aliases_and_ambiguous_skills` | same |
| `session::slash_commands::…::mixed_case_workflow_does_not_take_reserved_name` | same |
| `session::git::…::get_worktree_info_linked_git_worktree` | Path-separator normalization: got `H:/…/main`, expected `H:\…\main` |
| `workspace_ops::…::repos_list_does_not_load_user_global_manifest` | Sensitive to this machine's `TEMP=H:\dev-cache\tmp` (separate drive, so the manifest walk-up differs) |
| `workspace_ops::…::repos_manifest_search_dirs_skips_user_global_grok_home` | same |

None of the four owning files are in `860e8817a..HEAD`.

**The `slash_commands` trio is a genuine contradiction in the test suite.**
`"review"` was added to the reserved-command list at
`crates/codegen/xai-grok-shell/src/session/slash_commands.rs:595` by commit
`a76bf8991` (2026-08-10, the rc.1 sync — confirmed an ancestor of `860e8817a`,
so it predates this range). Two tests now assert opposite things about it:

- `:3505` — `assert!(names.iter().any(|name| name == "review"))`
- `:3566` — `assert!(!names.iter().any(|name| name == "review"))`

One of them has to change. Worth fixing since it is not environment-dependent —
these tests build their own fixtures and call pure functions.

### Flaky, not broken

`queue::tests::upload_with_retries_aborts_immediately_on_404`
(`crates/codegen/xai-file-utils/src/queue.rs:4816`) failed the first run with
`request_count == 2` but **passed** on re-run. The retry *policy* is correct
(`item.attempts == 1`, `resolve_count == 1` both held); two HTTP requests went out
inside one attempt, which points at a transport-level retry race, not a policy bug.
`xai-file-utils` also has zero files in this range.

Also noted: `cargo test -p xai-file-utils` cannot build standalone here — feature
unification differs outside `--workspace`, and `aws-lc-sys` then requires NASM.

---

## Coverage

Areas audited: browser protocol/client/mock · WebView2 host and injected
collector · `browser_*` tools · session lifecycle integration ·
permission/isolation/confine · `disk` and `subagent` CLI · MCP servers and
bundled skills · models/prompt/catalog · TUI panes and diagnostics.

36 additional low-severity findings were surfaced but not put through
verification, and are therefore **not** listed above. They are in the run journal
if you want them triaged.
