# RC6 Phase 3 — sandbox credential deny + browser policy

Worktree implementation for Turbo rc6 Phase 3. Does **not** land isolation /
keep-N / capability_mode files from Phase 2.

## Incidents closed in this change

| Id | Severity | Fix |
|----|----------|-----|
| `inc_01a028e429b370c3b85594b8637cbada` | P1 | Confining profiles still grant `~/.grok` for sessions/logs/`config.toml`, but **write-deny** `auth.json`, `credentials.json`, `*.pem`/`*.key`/certs |
| `fr_01a028eb90907fd3bbf684be3ae2d755` | FR | Same deny-list (known basenames + shallow scan) |
| `inc_01a028e429aa7a139d010466601aa71f` | P1 | OAuth popups require an **exact https origin** allowlist, not a substring heuristic |
| `inc_01a028dfe05774508a9466fd9ce5ed3f` | P1 | `browser_save` re-checks URL policy on every redirect hop |
| `inc_01a025f5922478b3a46b582d8a546f4b` / `inc_01a028c9eed372339a3dc7935bb4afb1` | P1/P2 | Direct zip/pdf/binary URLs are brokered into session `downloads/` instead of a silent navigate no-op |
| `inc_01a025f39e10768082ee83ba0ea48cc4` / `inc_01a028dfe06179d1a052daf933a96dbd` | P1/P2 | Empty DOM snapshot falls back to CDP AX; both empty (non-blank) fails closed with a `browser_save` hint |
| `inc_01a025f74c017070aadfc3eb46e5f6b7` / `inc_01a028e001c57223a41f3aa581c4cd2f` | P1/P3 | `browser_downloads wait_ms` clamped to 60s (same as `browser_wait`) so JS interstitials cannot hang |
| `inc_01a025fc5beb7373884278f829134240` | P1 | Host and tool share `path_is_under_session_folder`; workspace files are still copied into session `uploads/` first; raw workspace paths are refused with a clear error |
| `inc_01a028c941c775818606f35b413c91df` | P3 | HTML saves use Content-Type / Content-Disposition / URL path; default `page.html` not `download.bin` |

## Files

### Sandbox

- [`crates/codegen/xai-grok-sandbox/src/paths.rs`](../crates/codegen/xai-grok-sandbox/src/paths.rs) — `grok_home_credential_write_deny_paths(_in)`, basename/suffix matcher, shallow scan (skips `sessions/`, `logs/`, `worktrees/`, `agent-browser/`, …)
- [`crates/codegen/xai-grok-sandbox/src/profiles.rs`](../crates/codegen/xai-grok-sandbox/src/profiles.rs) — `SandboxProfile.credential_write_deny` on workspace, devbox, read-only, strict, and custom (including `extends = "devbox"`). Seatbelt write-deny via existing `apply_write_deny_paths_to_capability_set`
- [`crates/codegen/xai-grok-sandbox/src/lib.rs`](../crates/codegen/xai-grok-sandbox/src/lib.rs) — Linux bwrap `--ro-bind` of **existing** credential files on every confining profile

### Browser policy / host

- [`crates/codegen/xai-grok-browser/src/protocol.rs`](../crates/codegen/xai-grok-browser/src/protocol.rs) — `is_oauth_popup_url` / `oauth_popup_host` (exact https hosts); `path_is_under_session_folder`
- [`crates/codegen/xai-grok-browser/src/host/ax.rs`](../crates/codegen/xai-grok-browser/src/host/ax.rs) — `pick_snapshot_nodes` (empty DOM → AX fallback → fail closed)
- [`crates/codegen/xai-grok-browser/src/host/webview.rs`](../crates/codegen/xai-grok-browser/src/host/webview.rs) — snapshot uses that picker; failed zip/pdf navigations surface recent brokered downloads; download broker prefers WebView2 suggested filename
- [`crates/codegen/xai-grok-browser/src/host/mod.rs`](../crates/codegen/xai-grok-browser/src/host/mod.rs) — `browser.set_file` uses the shared session-folder allowlist

### Tools

- [`crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/save.rs`](../crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/save.rs) — custom redirect policy (max 5 hops, re-check `check_url_in_session`); filename from headers
- [`.../browser/navigate.rs`](../crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/navigate.rs) — http(s) URLs with download extensions broker via `browser_save` instead of navigating
- [`.../browser/downloads.rs`](../crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/downloads.rs) — `clamp_downloads_wait_ms` (max 60_000)
- [`.../browser/set_file.rs`](../crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/set_file.rs) — broker then host allowlist; schema text matches host

## Tests

```powershell
cargo test -p xai-grok-sandbox --lib -- --test-threads=4
cargo test -p xai-grok-browser --lib -- --test-threads=4
cargo test -p xai-grok-tools --lib implementations::grok_build::browser -- --test-threads=4
```

Run on this worktree (2026-08-22): sandbox 26/26, browser 84/84 (1 ignored WebView IT), tools browser filter 32/32.

Notable unit tests:

- `paths::tests::credential_file_matcher_covers_auth_and_keys`
- `paths::tests::credential_write_deny_lists_known_names_and_existing_pem`
- `profiles::tests::confining_profiles_write_deny_grok_home_credentials`
- `profiles::tests::custom_devbox_keeps_credential_write_deny`
- `protocol::tests::oauth_popup_requires_exact_https_origin`
- `protocol::tests::path_under_session_folder_is_component_prefix`
- `host::ax::tests::empty_dom_uses_ax_fallback` / `empty_dom_and_empty_ax_fails_closed`
- `save::tests::redirect_hop_rechecks_url_policy` + wiremock `redirect_to_public_http_is_blocked`
- `save::tests::filename_from_html_content_type_is_not_bin` + `html_body_is_saved_as_page_html`
- `downloads::tests::wait_ms_is_clamped_to_page_wait_ceiling`
- `navigate::tests::zip_url_is_brokered_instead_of_silent_navigate`
- `host::tests::decode_set_file_refuses_workspace_path`

## Remaining gaps

1. **Windows kernel enforcement.** Sandbox `apply()` is still advisory on Windows (`applied=false`) — no Job Object / AppContainer jail. RC6.1 adds a **userspace write-deny** at LocalFs + write/search_replace/apply_patch/bash/monitor so agents cannot rewrite `$GROK_HOME/auth.json` (or `*.pem`/`*.key`) even without kernel enforcement. Remaining gap: a child process that is not spawned through those tools.
2. **Linux create-if-missing.** bwrap `--ro-bind` only overlays files that already exist. A missing `auth.json` can still be *created* under the writable `~/.grok` grant. Seatbelt can deny the literal path even when missing; Landlock cannot deny a subpath of an allowed tree.
3. **Host token refresh.** Write-deny applies to the same process that may need to refresh OAuth tokens into `auth.json`. That is the incident’s intent (agent/child must not write credentials); token refresh inside a confined process is still a product question.
4. **OAuth popup after open.** Exact-origin stops `evil.test/?ux_mode=popup` from getting a real window. A legitimate Google/Microsoft popup is still a runtime-owned WebView2 that this host does not policy-check after `SetHandled(false)`.
5. **Extension-less attachment URLs.** `https://cdn.example/download?id=1` with `Content-Disposition: attachment` still depends on WebView2 `DownloadStarting` (plus a 15s “recent file” fallback on failed navigation). Tool-layer intercept is extension-based (`*.zip`, `*.pdf`, …).
6. **PDF AX quality.** Fallback uids are `ax-N` and **not** actionable. Inline Chrome PDF viewer may still yield an empty AX tree; the snapshot then errors and tells the agent to `browser_save`.

Do not land this worktree from here. Parent should review `git diff` / `turbo subagent diff` and land explicitly.
