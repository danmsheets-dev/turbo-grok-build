# Changelog

All notable changes to **Turbo Grok Build** (`turbo` binary).

Format: [Keep a Changelog](https://keepachangelog.com/).  
Wire versions: [`VERSION`](./VERSION) (`1.0.0-rc.N` on Grok Build 1.0 core;
older community line was `0.2.119-rN`).

English-only product surface (UI and public docs) as of RC14.

---

## Pedigree (community line)

Turbo Grok Build evolved from the Hyper community fork of
[xAI Grok Build](https://github.com/xai-org/grok-build). Multi-agent work
accelerated at r6; product rebrand to **Turbo** at r10. From **1.0.0-rc.1**
onward, official Grok Build is the permanent upstream core remote
(`upstream` → `xai-org/grok-build`).

| RC | Wire | Theme |
|----|------|--------|
| r6 | `0.2.114-r6` | Isolation + headless honesty |
| r7 | `0.2.114-r7` | Folder worktrees by default |
| r8 | `0.2.114-r8` | `/deepaudit`, land/diff/discard |
| r9 | `0.2.114-r9` | Baselines, Boot Card, Auto Developer Log |
| r10 | `0.2.114-r10` | Turbo brand / `turbo` CLI |
| r11 | `0.2.114-r11` | Game Mode, Feature Request Log |
| r12 | `0.2.114-r12` | Isolation FS jail, densify, MCP harden |
| r13 | `0.2.114-r13` | Workspace Tree inject, Game Mode perf |
| r14 | `0.2.114-r14` | **web_fetch**, **workflow routing**, English-only |
| r15 | `0.2.119-r1` | Upstream 0.2.119 sync, security + Windows correctness |
| r2 | `0.2.119-r2` | Game Mode overhaul, disk gates, tools list |
| r3 | `0.2.119-r3` | Full Disk Clean (taxonomy, multi-path, prune, telemetry) |
| **1.0 rc1** | **`1.0.0-rc.1`** | **Grok Build 1.0.0 core + Turbo product layer** |
| **1.0 rc2** | **`1.0.0-rc.2`** | Agent WebView, Grok 4.6, confinement harden |
| **1.0 rc2.1** | **`1.0.0-rc.2.1`** | Agent WebView named-pipe hotfix |
| **1.0 rc3** | **`1.0.0-rc.3`** | Agent WebView field-test pass |
| **1.0 rc4** | **`1.0.0-rc.4`** | Fathom-style `/meeting` notetaker |
| **1.0 rc5** | **`1.0.0-rc.5`** | Harness Q&A: spawn catalog, WebView save, logs/redaction |
| **1.0 rc6** | **`1.0.0-rc.6`** | Providers ACP, isolation, Poolside hosted Chat API |

Older release notes (r1–r13 detail) are archived under
[`docs/archive/`](./docs/archive/).

---

## Unreleased

---

## [1.0.0-rc.6] - 2026-08-22

**Providers + isolation + browser policy.** rc.5 field tests found `/providers` ACP
dispatch holes, worktree keep-N/land fail-open, sandbox credential writes, and
NVIDIA catalog 400/410/hangs. This release lands those plus Poolside hosted Chat
API as a first-class `/providers` platform.

### Added
- **Poolside** (`/providers poolside <api_key>`). OpenAI-compatible Chat Completions
  at `https://inference.poolside.ai/v1`. Env: `POOLSIDE_API_KEY` /
  `GROK_POOLSIDE_API_KEY`. Offline catalog: Laguna S 2.1 (1M), XS 2.1 (256K),
  M.1 (256K). Thinking via `chat_template_kwargs.enable_thinking`; assistant
  `reasoning_content` is preserved on tool loops. Wire ids are
  `poolside/laguna-s-2.1` (catalog key `poolside/laguna-s-2.1`).
- **`turbo issues resolve --sha` / `turbo features ship --sha`** record a proving
  commit on the incident/request.
- **NVIDIA `agent_ready` spawn gate** — write-capable children cannot pin
  chat-only Integrate rows; glm-5.2 is catalog EOL (410). Extra `model_id` is
  stripped on NVIDIA Chat Completions.

### Fixed
- **`/providers` ACP** dispatches `x.ai/internal/set_platform_api_key` (OpenRouter
  and every other BYOK platform). rc.5 TUI saved keys; the agent match did not.
- **keep-N** never deletes `retain_worktree` or live-PID trees; land fail-closes
  on missing v1 `allowed_paths`; ReadOnly capability_mode clamps MCP and
  verbatim-fork tools.
- **Sandbox** write-denies `auth.json` / credential files under `~/.grok` on
  confining profiles (Windows enforcement remains advisory).
- **Agent WebView:** `browser_save` re-checks redirect policy; zip/pdf navigations
  broker into session downloads; PDF empty DOM falls back to AX; `wait_ms`
  clamped to 60s; HTML saves as `.html` not `download.bin`.
- **Land CLI** finds `--session` across cwd hashes and uses a nested git root
  when the umbrella workspace is not a repo.

### Known
- Windows sandbox credential write-deny is profile metadata only (no kernel jail).
- Named/required `tool_choice` is not supported on Poolside while thinking is on
  (Poolside API); Turbo uses auto tool_choice.

---

## [1.0.0-rc.5] - 2026-08-21

**Harness Q&A follow-through.** Isolated-worktree and Agent WebView field tests
on rc.4 produced a concrete list of spawn-catalog, download, log-redaction, and
timeout-finalize bugs. This release lands those product fixes.

### Added
- **`browser_downloads` on the live toolset** — the session inject list was
  missing the listing tool even though the host already brokered files.
- **`browser_save`** — save the current page URL (or an explicit https URL)
  into the session `downloads/` folder with sanitized names, size limits, and
  `browser_downloads` listing. Public PDF/guide save no longer depends on
  Chromium raising `DownloadStarting`.
- **`browser_downloads wait_ms`** — optional wait until a completed brokered
  file appears (JS download interstitials).

### Fixed
- **`openai/gpt-5.5` (and other `openai/gpt-5.*`) spawn slugs** alias to
  `openai-codex/…` the same way Luna/Sol already did. Spawn descriptions list
  only session-spawnable catalog keys plus those aliases.
- **Child boot card `Model:`** uses the child's resolved sampling model, not
  the parent `ModelsManager` current id.
- **Worktree DisplayCwd** remaps onto the **source git repo** (nested
  `turbo-grok-build` under an umbrella workspace), not the umbrella root.
- **HTTP error pages** (404 HTML) are a successful `browser_navigate`;
  download-shaped navigations that fire `DownloadStarting` are not reported as
  `-32000 navigation failed`.
- **`browser_set_file`** copies workspace/confine files into a session
  `uploads/` broker before the host path check (host remains session-folder
  only).
- **`mcp_server_health`** documents `failed` / overall `unknown`, and
  remediation names `turbo mcp doctor` (with `grok mcp doctor` as alias).
- **developer_log `snapshot_ref` redaction** and **feature_request evidence**
  sanitization.
- **Subagent turn-result `schema_version`** uses `GCS_SCHEMA_VERSION` (`v1.24`).
- **Explicit `timeout_ms` finalize grace** defaults to `timeout/6` clamped
  30–120s so children get a stop-and-summarize window before hard cancel.
- **`turbo tools list`** injects the same Windows `browser_*` set a live
  session gets (including `browser_downloads` / `browser_save`).
- **Windows release CI** — drop `/DEBUG:LongSymbolTruncate` (GitHub
  windows-2022 `link.exe` 14.44 rejects it with LNK1117). Keep
  `/DEBUG:FASTLINK` for PDB limits. Release.yml no longer blocks publishing
  on `cargo test --workspace`.

---

## [1.0.0-rc.4] - 2026-08-21

**Fathom-style meeting notetaker.** Join a Zoom/Teams/Meet/Webex link, transcribe all participants on Windows, auto-answer coworker `Turbo:` questions from the launch workspace, and save a work-only recap.

### Added
- **Fathom-style meeting notetaker** — `/meeting join <url> [name]` opens a Zoom/Teams/Meet/Webex **https** join URL. On Windows, captures **system playback (all participants) mixed with the mic** via WASAPI loopback, transcribed with Grok STT (falls back to mic if loopback cannot open, unless `GROK_MEETING_CAPTURE=loopback`). `/meeting stop` writes a **work-only** summary to `{workspace}/Meetings/YYYY-MM-DD - <Meeting Name>.md`. Set `GROK_MEETING_CAPTURE=mic` to force microphone-only.
- **Meeting Q&A** — Coworkers type or say `Turbo: …`. Turbo **auto-injects** a research turn (workspace files, MCP, web) and `meeting_reply` posts `[Turbo] …` to Teams when `GROK_GRAPH_TOKEN` is set. Coworker text is treated as untrusted data (not extra system instructions). Set `GROK_MEETING_AUTO_ASK=0` to queue only. `/meeting ask` still drains manually.
- **Meeting recap highlights** — `/meeting stop` summary includes **For you** (asks/actions for the operator) and **Projects** (matched to the launch workspace).

### Fixed
- Join URLs are parsed as real `https` URLs (host-based platform classify, no `cmd` metacharacters). Windows opens via `explorer.exe` instead of `cmd /c start`. `GROK_TEST_OPEN_URL_FILE` is test-only.
- `GROK_MEETING_CAPTURE=loopback` no longer falls back to the microphone when WASAPI mix fails.
- Recap writes stay inside `{workspace}/Meetings` (no `..` reuse, no symlinked Meetings folder). Meeting ids reject path separators.
- Graph `$filter` percent-encodes `JoinWebUrl` and matches the returned meeting; chat ids are path-encoded; Graph error bodies are truncated.
- WASAPI mix-format / event HANDLE leaks on loopback open failure; `CoUninitialize` only after a successful `CoInitializeEx`.
- Oracle execution-budget test matched the 24-turn / 48-tool definition (was still expecting 12/40).
- Nested-subagent depth test hung: default max depth is 2, so a depth-1 spawn waited forever on the mock backend.

---

## [1.0.0-rc.3] - 2026-08-19

**Agent WebView field-test pass.** rc.2.1 made the window usable. This release
is the evening QA round: new page-control tools, click/fill/eval policy that
matches job-hunt and 1:1 outreach, OAuth popups that do not hijack the only
tab, session-scoped profiles, and a compaction retry that recognizes more
context-window overflow wordings.

### Added
- **Brokered Agent WebView downloads** — page-initiated downloads are redirected into the session-scoped `downloads/` folder with sanitized collision-safe names; `browser_downloads` reports completed files without exposing arbitrary destination writes.
- **NVIDIA Integrate models** — Muse Glimmer (`meta/muse-glimmer-30b`, 131K),
  Poolside Laguna XS 2.1 (`poolside/laguna-xs-2.1`, 256K), and
  Mistral-Nemotron (`mistralai/mistral-nemotron`, 128K) via
  `https://integrate.api.nvidia.com/v1` / `$NVIDIA_API_KEY`. Catalog keys
  `nvidia/meta/…`, `nvidia/poolside/…`, `nvidia/mistralai/…` plus short
  `nvidia/<id>` aliases; same `request_compat` as other NIM chat models.
- **`browser_wait`** — poll until `text` is on the page or the URL contains
  `url_substring`. Timeout names what was waited for.
- **`browser_scroll` / `browser_press_key` / `browser_select` / `browser_hover` /
  `browser_set_file`** — below-the-fold Apply, combobox Enter, filters, hover
  menus, and workspace / session-folder file inputs (resume PDFs). Downloads
  stay blocked.
- **`browser_raise`** — bring the hidden window back for human login.
- **`browser_snapshot include_text`** — truncated main/article text so LinkedIn
  Experience and Indeed job bodies survive the 200-node cap.
- **Session-scoped WebView profile** — default
  `$GROK_HOME/agent-browser/sessions/<id>`. `GROK_BROWSER_PROFILE=durable` opts
  into a shared job-hunt profile.
- **LinkedIn 1:1 playbook** in the `agent-browser` skill (Connect vs Message,
  200-char notes, Pending stop, close the messaging overlay first).

### Fixed
- **Umbrella isolation source repo** — `isolation=worktree` plus `cwd` selects
  the source git checkout (child still runs in a new worktree). Unique nested
  git discovery and `GROK_SUBAGENT_REPO_ROOT` remain as fallbacks. Multiple
  nested git dirs without an explicit `cwd` still fail-closed.
- **Nested spawn default `max_depth=2`** — first-level children can spawn one
  grandchild. Operators can still set `GROK_SUBAGENTS_MAX_DEPTH=1`. Child boot
  card says when spawn is stripped at the ceiling.
- **Sol Medium spawn aliases** — `openai/gpt-5.6-sol` (and `-pro`, short
  `gpt-5.6-sol`) resolve to `openai-codex/gpt-5.6-sol` like Luna.
- **Explore/oracle wall-clock** — explore 5 min / 24 turns; oracle 10 min /
  24 turns so read-only children finish instead of 120–180s stalls.
- **Spawn start honesty** — background spawn result includes
  `isolation_requested` and the DisplayCwd-remap note.
- **Worktree `cargo` under confine** — `cargo` / `rustc` / `rustfmt` / `rustup`
  run|show|which` are modelled. Implicit `target/` plus `--target-dir` /
  `--root` are confine operands so `cargo test -p …` works and `cargo install`
  to `~/.cargo` still fail-closes.
- **CMake/MSBuild FileTracker FTK1011** — worktree children pin
  `CARGO_TARGET_DIR` to the real `{worktree}/target` instead of inheriting or
  remapping onto the parent `H:\…\target`.
- **Detached HEAD after worktree commit** — `git worktree add` uses
  `-B grok/<dest-basename>` so commits stay on a named child branch.
- **NVIDIA child wall-clock** — default 10 min → 1 hour; stall 10 min → 30 min.
  Explicit `timeout_ms` is not capped at 30 minutes.
- **Reviewer stall** — agents whose name contains `review` default to 48 tool
  calls and a 10 min budget unless the definition overrides them.
- **`review-current-branch`** — no longer hardcodes `origin/dev`. Scope uses
  `capability_mode=execute` so git works; empty baseline resolves
  `@{upstream}` then `HEAD`.
- **`allowed_paths` refusals** name the prefix to re-spawn with. Root
  `.gitattributes` / `.gitignore` are writable and landable even when the
  allowlist is crate-scoped.
- **`location.href` reads no longer need `confirm=true`**. Assignment / replace /
  assign / `el['click']` / `document.write` / `.src=` still do. The host
  re-checks mutating eval.
- **Denied `browser_navigate` no longer drops the snapshot**, so the next click
  is not "call browser_snapshot first".
- **Click reports a cancelled navigation** instead of success-on-the-old-page.
  Result includes the post-click URL.
- **Overlapping writes are refused** (`browser_busy`) instead of last-write-wins
  on the single tab.
- **Click confirm** gates Apply / Connect / Follow / Invite / Message; **Sign in**
  does not (the human types secrets in the window).
- **Snapshots** pick up `role=status|option|listbox|dialog`, `<label for>`,
  overlay Close, and keep Experience/About headings under the cap. Same-origin
  iframes are walked.
- **contenteditable / Lexical fill** uses `insertText` + composed InputEvents
  (and a paste-shaped fallback) so LinkedIn Send enables.
- **Google/Microsoft OAuth popups** are real windows. GSI is no longer
  Navigated into the only tab.
- **Iframe navigations** go through the same URL policy as the top-level frame.
- **Pipe binds before WebView2 init**; ensure wait is 45s. The host is enrolled
  in a Job Object / process scope so leftover `msedgewebview2.exe` dies with
  the pager.
- **Uid schema examples** are epoch-index (`4-17`), not positional `"2"`.
- **Boot card** documents close-hides. **chrome-mcp** skill now says: use
  `browser_*` unless the user asked for daily Chrome.
- **Compaction** treats more overflow phrasings (`context window`, `token
  limit`, `too many tokens`, `input too long`) as deterministic context-length
  errors so the input ladder can step down instead of failing closed.
  `should_compact_on_error` now recovers from CLE text even when stream
  metadata is missing or the token estimate sits under the advertised window.
  StreamError overflow is no longer retried as a blip. Checkpoint is written
  after the forked history is resolved, and the jsonl marker is not written if
  the checkpoint file cannot be queued. Truncated unclosed `<summary>` blocks
  are degenerate and retried.

### Policy
- Agent WebView is HITL assist, not a bot. Skill forbids bulk Indeed apply and
  LinkedIn connect/message loops.

### Changed
- Wire version **`1.0.0-rc.3`**.

## [1.0.0-rc.2.1] - 2026-08-19

**Agent WebView hotfix.** rc.2 shipped the Agent WebView with a defect that made
it unusable: the window opened and stayed white. Everything here is that bug and
the field report that followed it.

### Fixed
- **`browser_*` calls hung forever (release blocker)** — the named-pipe server
  created `SPARE_INSTANCES + 1` instances but called `connect()` on exactly one.
  Windows hands an incoming client to *any* listening instance, so a client that
  landed on one of the four nobody awaited was accepted and then blocked forever.
  A fresh `browser_navigate` had a **four-in-five chance of never returning**, which
  is why the window opened and stayed blank. Reproduced by driving the pipe
  directly: four hangs, one reply, then pool exhaustion. The server now runs one
  acceptor task per instance, each awaiting its own `connect()` and re-arming after
  it serves. Regression test included — verified to fail against the old shape.
- **A wedged host produced no reply at all** — the pipe side awaited the UI
  thread's response with no deadline, so a request the UI thread never reached
  left the connection open with nothing written on it and the caller saw a bare
  75s transport timeout. There is now a middle rung between `NAV_TIMEOUT` (60s)
  and the client's `CALL_TIMEOUT` (75s) that answers with a real JSON-RPC error on
  the caller's own id.
- **First paint was an empty white rectangle**, which reads as a crash. The host
  now paints a card naming itself, the profile path, and what it is waiting for.
  Written into the blank document rather than via `NavigateToString` (which the
  navigation policy would cancel as a `data:` URI), and `aria-hidden` so neither
  snapshot path mistakes it for page content.
- **Every window was titled `Turbo Agent Browser`**, so a leftover host was
  indistinguishable from the live one and could sit on top of it. The caption is
  now `Turbo Agent Browser — <host> [<session>]`, set before the load starts and
  again when it settles. Host extraction rejects userinfo spoofing
  (`https://bank.test@evil/`), treats a backslash as an authority terminator per
  WHATWG, and strips control characters.
- **The close button killed the host with no telemetry** — one stray click looked
  exactly like a crash. `X` now hides the window and marks the WebView2 controller
  invisible; the host keeps serving and any `browser_*` call re-shows it. Host exit
  and close-to-hide both log to stderr, which the shell already drains into tracing.
- **`file:` URLs were refused by the host** — the spawn never passed
  `--session-folder`, so the host had nothing to measure them against while the
  client-side policy allowed them (audit finding C1).
- **The Agent Boot Card omitted the browser** even with `browser_*` registered, so
  the agent had no idea the window existed and went looking for a command to open
  it. There is none — the window appears on the first tool call. The card now
  carries a gated launch line (guaranteed by test); the click/fill/uid loop stays
  in the `agent-browser` skill.
- **The boot card reported the wrong version** (`1.0.0-rc.1` while `turbo --version`
  said `1.0.0-rc.2`). Three crates carried independent version strings and
  `xai-grok-version` was never bumped. Both build scripts now resolve
  `GROK_VERSION` → workspace `VERSION` file → crate version, so the `VERSION` file
  is the single source of truth, with a test asserting they agree.

## [1.0.0-rc.2] - 2026-08-19

**Agent WebView, Grok 4.6, and a confinement hardening pass.** A week of RC2
work folded into the release: a product-owned WebView2 browser the agent drives
directly, the Grok 4.6 default catalog, MCP disk-wins merge, full Disk Clean,
and remediation of two P0 findings from the RC2 audit
([`docs/RC2_UNRELEASED_AUDIT.md`](./docs/RC2_UNRELEASED_AUDIT.md)).

### Added
- **Grok 4.6 default catalog** — compiled-in `default` / `web_search` / `image_description` / `session_summary` now `grok-4.6` (advertises `xhigh` reasoning effort; hosted backend search on). `grok-4.5` remains selectable. Config / env / CLI overrides still win. Resolved-checkpoint display (`requested (resolved)`) now treats all `grok-4.*` slugs as coding models, not only `grok-4.5`.
- **Prompt `work_policy` + `browser_verification`** — official completion discipline merged into the primary and subagent templates without dropping Turbo `<action_safety>`. The `<browser_verification>` block renders only when the finalized toolset includes `browser_*` tools (not plan-builtin-only). AGENTS.md user reminders are not suppressed.
- **Agent WebView** (`browser_*` tools + `turbo browser-host`) — product-owned WebView2 window on `~/.grok/agent-browser`. Ctrl+Shift+B mirrors URL/snapshot in the TUI. Not chrome-devtools MCP.

### Security
- **`--confine` write-boundary escape (P0)** — `cmd /c <engine>` handed the whole
  compound command to the Windows recovery path and returned early, so sibling
  invocations were never classified: `cmd /c blender -b ; powershell -c "Set-Content
  C:\outside\x 1"` was allowed. The escape survived a first fix that counted bash
  command nodes, because `cmd /c "A & B"` is a single node. The recovery now fails
  closed on any newline, on any statement separator (`&`, `&&`, `|`, `||`, `;`, `^`)
  at every recursion depth, and on late-expanded tokens (`%VAR%`, `$env:`, `$(…)`,
  backticks, `~`) whose target cannot be range-checked. Also closes the glued
  redirect form `>C:\path`, which resolved as a *relative* path under the root.
- **Snapshot uid forgery** — `data-turbo-uid` is page-writable and `elByUid` resolved
  it with `querySelector`, so a page could stamp a live uid onto a control of its
  choosing and capture the next `browser_click` / `browser_fill`. Resolution now runs
  through a registry held in the CDP isolated world. This also fixes uid lookup for
  nested shadow roots, which the old document-level query only reached one level deep.
- **`turbo disk clean` deleting non-product data** — `plugin-worktrees` hardcoded
  `H:\gb` / `H:\gb-work` and removed *every* child directory. Roots are now
  configuration-only (`GROK_BUILD_WORKTREE_ROOT` / `GROK_PLUGIN_WORKTREE_ROOT`, split
  on `;` only, since a comma is a legal path character), and a child must carry a
  product-shaped name and not be an ordinary clone. Separately, the shared temp-root
  filter no longer matches the generic `tmp.*` / `.tmp*` / `uid-*` / `kg-*` spellings
  that `mktemp -d` and `tempfile` produce for unrelated applications — reachable from
  a bare `turbo disk clean --safe`.

### Fixed
- **`disk clean --safe --if-low-space` was a no-op on a healthy disk** — the temp-grok
  always-sweep retained `TempGrok`, then fell into a volume filter whose low-volume set
  was empty and which stripped it back out, reporting `ok: false`. Subagent dispose runs
  exactly this command.
- **Windows path separators** — `get_worktree_info` returned libgit2's forward slashes,
  silently defeating the `collapse_home_path` prefix match; and `path_not_found_hint`
  joined a POSIX display path with the host separator, handing the model
  `/home/user/project\src`.

### Harness polish (first RC2 drop, 2026-08-11)

Identity, permission-rule compatibility, and job-object ergonomics so delegates
stop dying at launch against wrong `turbo` binaries or legacy deny prefixes. No interactive TUI redesign —
focus is identity, permission-rule compatibility, and job-object ergonomics so
delegates stop dying at launch against wrong `turbo` binaries or legacy deny
prefixes.

### Added
- **`turbo version --json` identity card** — besides `currentVersion` /
  `channel`, now emits:
  - `product` (`turbo-grok-build` | `grok-build`)
  - `binary` (`turbo` | `grok`)
  - `cliFamily: "grok-build"`
  - `agentCompatible: true`
  - `features` (`headless`, `confine`, `jsonSchema`, `jobObject`, …)
  - `permissionToolPrefixes` (stable list for harness filters)
  Harnesses can distinguish Turbo Grok Build from Vercel Turborepo without
  scraping `--help`.
- **`GROK_JOB_OBJECT=1`** env alias (alongside `TURBO_JOB_OBJECT` /
  `HYPER_JOB_OBJECT`) for Windows Job Object opt-in.
- **`turbo subagent land --json-union-by=<key>`** — union-merge landed JSON
  arrays of objects by that key (child wins). `assets/manifest/*.json` always
  merges by `name` so parallel densify lands keep sibling rows. Fail closed
  if a targeted file is not an array of objects or an object map.
- **`land_subagent.json_union_by`** — same merge on the tool path.
- **Imagine-web skill** — `bundled/skills/imagine-web` drives grok.com Imagine
  through chrome-devtools MCP (snapshot → fill → save). Pair with a user MCP pin
  (`turbo mcp add chrome-devtools`) using `~/.grok/browser-profile` and
  `--allow-unrestricted-paths`. Login is human-in-the-loop; not the Imagine API.
- **`turbo mcp restart <name>`** — disable then enable so a live session
  re-merges from disk.
- **`/chrome-mcp` skill** — chrome-devtools loop, `--autoConnect` daily Chrome,
  Cloudflare SSO warning, draft-not-send on social sites.
- **MCP merge: disk beats session snapshot** — TUI `session/new` injects
  `load_mcp_servers()` as the client list; a client `insert` froze
  chrome-devtools argv across `config.toml` edits. Client extras still
  survive; TOML/plugin args win on the same name.
- **Nemotron 3.5 Lightning** — spawnable catalog slugs
  `nvidia/nvidia/nemotron-3.5-lightning-30b-a3b` and
  `nvidia/nemotron-3.5-lightning-30b-a3b` (NVIDIA Integrate, 1M ctx,
  same request_compat as Ultra/Super).
- **Luna slug aliases** — `openai/gpt-5.6-luna` (and `-pro`) resolve to
  `openai-codex/gpt-5.6-luna` so keep-N / densify spawn does not 400.
- **`turbo disk` nested `target/` + plugin worktrees** — report/clean walk
  child `Cargo.toml` / `CACHEDIR.TAG` trees, and `--include plugin-worktrees`
  scans `GROK_BUILD_WORKTREE_ROOT` plus `H:\gb` / `H:\gb-work`. Live markers
  and recent unlanded dirs are skipped.
- **Keep-N land artifacts** — `meta.json` + `changes.patch` copied to
  `~/.grok/subagent-artifacts/<id>/`. `land_subagent` / `diff_subagent` fall
  back to that store, then `refs/grok/subagents/<id>`, when session meta is
  gone.

### Changed
- **Claude-compat deny prefixes** — `NotebookEdit` / `MultiEdit` → Edit,
  `NotebookRead` → Read. Older Grok Build plugins that still emit
  `--deny NotebookEdit(**)` no longer hard-abort headless starts with
  `unsupported tool prefix`. `EnterWorktree` remains unsupported.
- Wire version **`1.0.0-rc.2`**.

### Fixed (RC2 harness closeout)
- **Absolute worktree writes remapped to parent (P0)** — isolated
  `…/.grok/worktrees/…/subagent-…` paths are no longer DisplayCwd-folded
  onto the shared checkout.
- **Subagent 0-tool stalls** — first-progress timeout 60s after spawn
  (worktree setup excluded); `allowed_paths` stall 3 min so scoped
  children finish and the parent can land.
- **godot-docs-mcp pipe closed** — docker stdio wait + one respawn;
  handshake error names the Windows 232 / pin workaround.
- **Windows densify confine** — PowerShell `& "C:\\Program Files\\…\\blender.exe"`
  and `cmd /c` wrapping the same is now modelled (script/export operands only).
  Random Program Files exes stay fail-closed `shell-unparseable`.
  Session-configured `GROK_BLENDER` / `GROK_GODOT` paths (even renamed
  basenames) are also modelled.
- **Land + `allowed_paths`** — `.grok-subagent-live` and `.grok/` are harness
  markers, not payload. Land no longer refuse-closes on them and does not
  copy them into the parent.
- **`turbo disk temp-grok`** — report and `--safe` clean now include aged
  TEMP-root leftovers (`grok-*`, `nest-*`, `goal-*`, `kg_*`, empty `.tmp*`,
  …) not just `%TEMP%/grok`. Official `grok/sessions` still age-prunes with
  a fail-closed newest-mtime scan. Post-subagent `--if-low-space` still
  sweeps `temp-grok` when space is OK.
- **`turbo subagent list`** — default is the newest session for this cwd;
  discarded+cleaned leftovers are hidden; `running` + cleaned is shown as
  `stale`. Pass `--all` for the old dump.
- **Worktree seed HEAD race** — reset prefers the source commit SHA and
  retries once on `Could not parse object 'HEAD'`.

### Harness notes
- Prefer `turbo version --json` → `agentCompatible` / `cliFamily` for CLI
  selection on PATH.
- Isolated Windows delegates: pass `--job-object` or set `TURBO_JOB_OBJECT=1`
  so stop can tear down the whole process tree via the job handle.
- Confined blender-artist / Godot workers may invoke the installed engine
  binary even when it lives under Program Files; quote the path.
- Plugin / cargo tests should nest temps under `%TEMP%/grok/{plugin-tests,tests}`
  so `turbo disk clean --safe --include temp-grok` can reclaim leaks.

---

## [1.0.0-rc.1] - 2026-08-09

**Turbo Grok Build 1.0 RC1.** Merges official **xAI Grok Build 1.0.0**
(`75e73f3d6`, monorepo `SOURCE_REV` `a61c32b12…`) as the shared core while
preserving Turbo’s product overlay (`turbo` binary, disk clean, Game Mode,
workflows/deep-audit, ADL/FRL, isolation/land, English-only, multi-provider).

### Upstream core (adopted)
- Product ladder **0.2.120 → 0.2.121 → 1.0.0** (dashboard turn summaries,
  extensions modal, `/feedback` report box, SSH/tmux auto theme, table reflow,
  permission full-script + Ctrl-F, MCP image fixes, queue/cancel hardening,
  remote resume defaults, session fork memory, and related stability work).
- Rust toolchain pin **1.94.0**.
- Auth `bearer_fragment`, computer-hub connection work, tool-types task model
  updates, workspace restore hang fixes.

### Preserved (Turbo product layer)
- CLI **`turbo`**, `community-build`, install/update under `~/.turbo`.
- Disk clean / multi-path free-space gates / `turbo disk|issues|features|tree|tools`.
- Game Mode, workflows + stock deep-audit/deep-research/continuous-improve,
  Auto Developer Log + Feature Request Log, workspace tree inject, boot card.
- Headless **streaming-json schemaVersion 2** (subagent lifecycle, confine,
  tool_denied) — upstream reducer subtree not reintroduced.
- Windows correctness: line-ending CI, process-tree, `RUST_MIN_STACK`, CRT notes.

### Changed
- Wire version **`1.0.0-rc.1`** (semver pre-release on the 1.0.0 core).
- Tracking branch for this sync: `sync/1.0.0-rc1` (merge base `e5478eff1`).

### Fixed (RC1 hardening pass)
- **`turbo tree prune --execute`** — dry-run by default (parity with
  `subagent prune` / `disk prune`); pass `--execute` to delete.
- **Windows bare `bash` / `*.sh`** — agent terminals route through Git Bash when
  installed (never WSL `System32\bash.exe`); PowerShell remains default for
  native `/flag` toolchains. Opt out: `GROK_PREFER_GIT_BASH_FOR_SCRIPTS=0`.
- **Land allowlist case-fold on Windows** — `assets/Mini Games` matches
  allowlist `assets/mini games/` (NTFS); write-time and land share the rule.
- **Worktree seed honesty** — `<worktree_seed>clean|dirty</worktree_seed>` on
  completion; clean seed documents missing parent WIP.
- **Isolation CWD assert** — isolation=worktree without a real subagent worktree
  path fails closed (unless `GROK_SUBAGENT_ALLOW_SHARED_FALLBACK=1`).

### Added (RC1 hardening pass)
- **`turbo disk recover --safe`** — closed-loop check → clean `--if-low-space` →
  re-check; exit 1 if still under the free-space gate.
- **Post-subagent disk clean** — after dispose, best-effort
  `disk clean --safe --if-low-space` (5‑minute debounce). Disable with
  `GROK_POST_SUBAGENT_DISK_CLEAN=off`.

### Notes
- Full package-scoped compile/test campaign is part of this RC; treat as
  integration candidate until Round A–D gates are green on Windows.

---

## [0.2.119-r3] - 2026-08-07

**Turbo Grok Build RC3.** Full **Disk Clean** feature for post-agent Rust/cache
reclaim — category taxonomy, multi-path free-space gates, unified prune, and
JSON reclaim telemetry. Default `--safe` behavior remains RC2-compatible.

### Added
- **`turbo disk clean --safe --include <cats>`** — opt-in reclaim categories:
  `debug`, `debug-pdbs`, `debug-incremental`, `release`, `release-dist-caches`,
  `worktrees`, `tree-store`, `temp-grok`, `cargo-home` (requires
  `--i-accept-redownload`). Omitted `--include` keeps the RC2 default set
  (`debug` + aged worktrees + tree store).
- **`turbo disk clean --json`** — machine-readable `reclaimed_bytes` by category
  and `total_reclaimed_bytes` (agents / continuous-improve).
- **`release-dist-caches`** — removes only
  `incremental` / `deps` / `build` / `examples` / `.fingerprint` under the
  resolved target root’s `release-dist`; **keeps** ship binaries (`turbo.exe`).
  Safety is Cargo’s profile `.cargo-lock` (exclusive lock held through deletes);
  existence of the lock file alone is not treated as “active.” Optional mtime
  hints use `--active-build-grace-secs` (default 120).
- **`CARGO_TARGET_DIR`** — report/clean sizes and reclaim use this path when set
  (not only `<workspace>/target`).
- **`turbo disk prune`** — unified `--worktrees` / `--tree-store` /
  `--session-meta` / `--all` with dry-run default and `--execute`.
- **Multi-path free-space report/check** — gates workspace root, worktrees base,
  `CARGO_TARGET_DIR` (when set), and `GROK_HOME` (when set); fail closed with
  path-labeled remediation.
- **Report category breakdown** — debug PDBs, debug incremental, release-dist
  caches, cargo-home registry/git cache sizes.

### Safety (unchanged defaults)
- Clean still requires `--safe`; cargo-home requires a second consent flag.
- Fresh live worktrees (`.grok-subagent-live` within
  `GROK_SUBAGENT_LIVE_MARKER_MAX_SECS`, default 12h) are protected; **stale**
  markers are reclaimable (parity with spawn soft-preserve).
- Ship profile binaries are never deleted by default safe clean.
- Multi-path free-space also gates **cargo-home** and **TEMP** volumes.
- `--if-low-space` only reclaims categories on **failing volumes** (no wiping
  H:`target/debug` when only C: worktrees are low); gate disabled
  (`GROK_MIN_FREE_GB=0`) no longer forces a full clean.
- Profile locks for `debug` / `release` / `release-dist-caches` refuse reclaim
  while cargo holds the profile lock.
- Session-meta prune skips active statuses (`running`/`starting`/…).
- JSON clean results include `free_bytes_before`/`after`, stable `skipped_reason`,
  and report emits `suggested_clean` ranked by size for agents.

---

## [0.2.119-r2] - 2026-08-05

**Turbo Grok Build RC2.** Second RC on the `0.2.119` wire line. Headline is a
full Game Mode overhaul — every confirmed bug and performance finding from the
[RC2 Game Mode audit](./docs/RC2_GAME_MODE_AUDIT.md) fixed, two new hover
tooltips, and eleven new sprite animations. Game Mode test coverage went 25 →
132.

### Added
- **Game Mode — Supervisor hover tooltip:** model, phase, turn elapsed, context
  window used/total, seat + overflow counts, wall state, git branch.
- **Game Mode — MCP server rack:** the rack sprite is now composed into the
  office, with a hover tooltip listing each server's status, tool count and
  failure detail. Backed by a new per-agent MCP status cache so status is live
  outside the `/mcps` modal (pushes were previously dropped when it closed).
- **Game Mode — eleven sprite animations:** debug-rage fail pose with a red
  error monitor, arms-up celebrate with confetti, papers flying during handoff,
  monitor glow + compile flash, swinging door on spawn/exit, MCP rack LED bursts
  keyed to real tool calls, coffee-sip idle (which revives the previously dead
  idle steam and thinking-bubble blink), real day/night wall clock with hour
  tint, office-wide success wave on WORK FINISHED, typing cadence driven by
  token throughput, and a floor robot that patrols only while the office is busy.
- `turbo tools list [--json] [--require NAME]` — headless schema assert for registered model-facing tools (respects `GROK_SUBAGENTS` / `[subagents] enabled`).
- `turbo disk check [--min-free-gb N]` — fail closed under free-space gate (default `GROK_MIN_FREE_GB=40`).
- `turbo disk clean --safe --if-low-space` — reclaim only when under the free-space gate.
- Disk report surfaces **min free** threshold status and **keep-N** vs live `subagent-*` count.

### Fixed
- **Game Mode tier off-by-one:** the tick path re-peeled the status strip, so at
  a 19-row paint area every tick snap-cleared walks, celebrates and handoffs
  while the office still painted them — those animations were impossible at that
  terminal height. Tick and paint tiers now match by construction.
- **Game Mode animation cadence:** the 90 ms animation gate sat above the 83 ms
  Slow tick interval and dropped every other tick, so the office ran at ~6 Hz
  and the wall clock at half real time. The gate is now derived from
  `SLOW_TICK_INTERVAL` so the two cannot drift apart.
- **Exit walk:** after a handoff the developer teleported backward and walked
  into the supervisor again instead of leaving. It now exits through the door.
- Nine further Game Mode correctness bugs: hover popup clipping, failure-status
  vocabulary drift against the dashboard classifier, stale tool-call counts on
  finished agents, stale overflow badge, Unicode art alignment, token unit
  rounding at boundaries, and a dead supervisor-phase write.

### Changed
- **Game Mode idle cost:** an open office no longer pins the event loop at
  ~12 Hz. A frozen room parks (Compact/Unicode at zero wakeups; the pixel office
  falls through to a budgeted ~0.33 Hz ambient tick that drives the idle
  animations). Per-tick snapshot rebuilds over the insert-only subagent map are
  change-gated, image caches are released when the view closes (~8-10 MB that
  previously leaked for the process lifetime), and paint buffers are reused
  in place instead of reallocated per animation step.
- Soft-preserve keep-N default **3** (`GROK_SUBAGENT_KEEP_N`; alias `GROK_SUBAGENT_SOFT_PRESERVE_KEEP_N`). **`0` = age-only** prune (`GROK_SUBAGENT_KEEP_MAX_AGE_SECS`, default 24h).
- Pre-spawn free-space default **40 GiB** (`GROK_MIN_FREE_GB`; alias `GROK_SUBAGENT_MIN_FREE_BYTES`). Set `0` to disable.
- Safe clean skips live-marked worktrees (`.grok-subagent-live`).

### Known gaps
- Supervisor pacing (audit animation #10) is deferred: the supervisor sprite
  bakes his desk into the same image as the figure, so it needs an asset split
  rather than a code change. Reasoning is recorded in the audit doc.

---

## [0.2.119-r1] - 2026-08-04

**Turbo Grok Build RC15.** The wire version jumps `0.2.114` → `0.2.119`: Turbo's
upstream base was actually **0.2.112** (newest bundled release notes were
`0.2.112.md`) while `VERSION` advertised `0.2.114`, so `--version` and the
What's-New surface were both wrong. RC15 syncs seven upstream releases and
re-stamps everything to one value.

### Sync point (read this before the next sync)

RC15 is where Turbo re-synced with the **Hyper community line**
(`DaviRain-Su/hyper-grok-build`, remote `community`) and with **xAI upstream**:

| Ref | Commit | Meaning |
|-----|--------|---------|
| xAI upstream | `e5478eff1` (`SOURCE_REV` `27d2088ae…`) | 0.2.119, merged in full |
| Hyper fork point | `c260695cc` | where Turbo and Hyper diverged (2026-07-29) |
| Hyper compared at | `7a48dd755` | Hyper `community/dev` head at audit time |

Cherry-picked from Hyper: `9cd8dffca` (sampler privacy + WASM decode),
`783989c01` (credential rotation, voice), `2c68d9d26` (model config reload),
`d49c3feb3` + `a831b5620` (DeepSeek V4). Hand-ported from Hyper's merge
`488edc10d`, which is reachable from neither branch tip: the circuit-breaker
probe fix.

**Deliberately NOT taken** (evaluated and declined): the Comet desktop app (does
not run on Windows), Hypercore (default-off, would add a second turn engine),
the `packages/*` reorg (git rename detection already bridges the layouts at
2,945 renames, so declining costs nothing), upstream's headless reducer rewrite
(Turbo's `streaming-json` schemaVersion 2 contract is richer), and the Codex
turn-state / remote-compaction work.

### Security

- **`x-grok-*` headers no longer leak to third-party providers.** Turbo stamped
  `x-grok-deployment-id` / `x-grok-user-id` / `x-grok-client-identifier` on every
  sampling request with no base-URL check, and set no redirect policy, so
  reqwest followed up to 10 hops including cross-origin. As the heavier
  multi-provider fork (NVIDIA/Nemotron, OpenRouter, Ollama, Kimi, Azure-style
  proxies) Turbo leaked xAI product/session identity on ordinary traffic. Now
  gated on an HTTPS-only, suffix-safe first-party allowlist, stripped from the
  *finalized* header map so a late injector cannot reintroduce them, and
  redirects follow only same-origin HTTPS.
- **Allow-rule traversal escape** (introduced by the sync, caught by upstream's
  own tests): `./../../etc/passwd` textually matched the glob `./**` while
  denoting `/etc/passwd`. Allow rules no longer match a raw spelling containing
  a parent-dir component; deny/ask keep the full multi-spelling union.
- **Subagent path-segment validation.** Model-supplied task/session/agent ids
  reach filesystem paths in more places in Turbo than upstream and were
  unvalidated. Now fail-closed against traversal, separators, drive/UNC
  prefixes, NUL/control chars, alternate data streams, Windows reserved device
  names, and trailing dot/space.
- **Strict WASM guest decode** — over-long or invalid UTF-8 from a guest is
  rejected rather than silently truncated or lossily decoded.
- **MCP** credential storage hardened against multi-process wipe (via sync).

### Fixed

- **Self-update was broken on both platforms.** `release.yml` ships the whole
  `bundled/` tree; the extractor hard-bailed `"archive entry is nested"` on the
  second path component, so every `bundled/skills/*.md` aborted the update. The
  extractor now accepts `bundled/**` at any depth while rejecting zip-slip,
  absolute/rooted paths, drive prefixes, symlinks, reserved device names and
  depth > 32, with entry/byte caps. Activation is a compensating transaction
  (bundle → binary → state) so a crash leaves either the old bundle or the new
  one, never a merge.
- **Circuit breaker double-admitted half-open probes** — a zero-valued claim
  timestamp was ambiguous with "no claim", and reservation was not atomic
  against state transitions.
- **Credential rotation** now keys on the refresh-token *family* rather than
  access-token expiry, and four paths no longer discard a freshly minted token.
  Fires on concurrent refresh — Turbo's `spawn_many` / worktree profile.
- **`/compact` no longer orphans running background tasks and subagents**, and
  recovers instead of failing when the summarizer input exceeds the window.
- **DeepSeek V4 on Ollama Cloud** advertised 256k context / 32k max output
  against a real **1M / 384k** — users compacted at a quarter of the true window
  and were capped at a twelfth of the real output limit.
- **Windows: bundled skills shipped as CRLF** while Unix got LF, so the two
  platforms shipped different bytes for the same runtime.
- **Windows: mixed CRT.** The workspace sets `+crt-static` but had no CMake
  toolchain file, so a CMake-built native dep (bundled Opus) selected `/MD`
  against a `/MT` Rust runtime.
- **Windows: POSIX shell commands kept working.** The sync replaced the
  always-`sh -c` helper with hardcoded `cmd /C`, which would have silently
  broken any configured `auth_provider_command` using `$VAR`, `exit N` or
  pipes. Now routed through the shared shell detector (Git Bash when present,
  else pwsh/cmd, honouring `GROK_SHELL`).
- Stale RC14 rename leftovers: the published `streaming-json` schema still
  identified as Hyper, and the GitHub Release body still pointed at
  `DaviRain-Su/hyper-grok-build`'s install scripts.

### Added (from upstream 0.2.113–0.2.119)

- **LSP pull diagnostics** — the language server is no longer torn down on
  every edit (headline symptom: C# diagnostics never arriving).
- **`GROK_EXTRA_CA_BUNDLE`** — corporate TLS roots for inspecting proxies.
- **`/model` re-reads `config.toml`** on invocation, replacing only
  config-owned catalog rows so remote/provider/default rows survive.
- **`x.ai/task_completed` frames bounded to 32 KiB** so ACP clients with a
  line-length limit (Python asyncio, most Node readline setups) stop hard-failing
  on a large task log.
- File watching skips nested checkouts; `git-head-changed` fires on same-branch
  commits; session `/delete`; slash-command mode-support matrix; broad TUI
  polish across six releases.

### Changed

- **Rust 1.93.0** (required by upstream 0.2.118+ code and lint config).
- `RUST_MIN_STACK` is set for cargo-run processes: the 0.2.119 prompt-turn
  future exhausts the default 2 MiB Rust thread stack (the PE main thread's
  16 MiB `/STACK` does not cover libtest/tokio threads).
- Clippy bans unenrolled `Command::spawn` — an unenrolled child outlives its
  session.

### Known

- **The project picker is inert.** Upstream removed its own picker in 0.2.119
  and the sync accepted that in the non-conflicted files; Turbo's
  `project_picker` module and `AppView` state remain but nothing triggers them.
  Left in place rather than deleted or re-implemented.
- **The test suite is not green on Windows and never was.** 477 of ~5,900 tests
  fail on `dev` for POSIX reasons (test support hardcodes `/tmp`; some tests
  shell out with `printf` / `${VAR:-default}`). RC15's differential against that
  baseline shows **zero regressions**.
- `--include-partial-messages` is accepted but rejected with a pointer to
  `--output-format streaming-json`: it only ever applied to upstream's
  `streaming-messages-json` reducer, which Turbo does not carry.

---

## [0.2.114-r14] - 2026-08-04

**Turbo Grok Build RC14** — production `web_fetch`, workflow routing so free-text
deep audits launch real Rhai recipes, English-only UI, docs reset, and product
rename prep (`turbo-grok-build`).

### Added
- **`web_fetch`** — URL → clean markdown for agents (article/full/raw extract,
  SSRF + DNS pin, challenge detection, token-aware windowing/links, browser-shaped
  client, apex↔www redirects, doctor status).
- **Workflow routing** — boot card catalog + system-prompt policy; natural-language
  host soft-match (“run a deep audit on …”) launches stock recipes.
- **Game Mode screenshot** — `docs/assets/screenshot-game-mode.png` for README.

### Changed
- Wire version **`0.2.114-r14`**.
- **English-only UI** — non-English locale packs and zh-CN user guide removed;
  language setting resolves to `en` only.
- **Docs cleanup** — public docs slimmed; historical RC/Q&A material under
  `docs/archive/`; changelog starts fresh from this RC with pedigree table.
- Stock **deep-audit** is the only recommended audit path (no `deep-audit-fixed`).

### Fixed
- Full `web_fetch` security/token hardening suite (allowlist enforcement, non-2xx
  errors, body streaming limits, charset decode, cache purge, overflow budget).

### Notes
- CLI: **`turbo`**. Install: `~/.turbo/bin`. Config/auth still `~/.grok`.
- Rebuild for binary:  
  `cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo`
- Tracking remotes (post-rename):
  - `origin` → your Turbo fork
  - `upstream` → `xai-org/grok-build` (official)
  - `community` → `DaviRain-Su/hyper-grok-build` (Hyper community, fetch-only)
- For **Turbo vs Hyper** compare: `git fetch community` then
  `git log --oneline HEAD..community/dev` (do not merge casually).
