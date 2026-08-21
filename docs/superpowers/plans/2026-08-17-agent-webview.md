# Agent WebView Window Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Turbo a product-owned **WebView2 window** (full HTML/CSS/JS) on a dedicated profile, driven by first-class agent tools, that never attaches to the user’s daily Chrome.

**Architecture:** A **sidecar host** (`turbo browser-host`) owns the Win32 HWND + WebView2 message loop. The TUI / agent runtime never embeds a browser in ratatui. Control is JSON-RPC 2.0 over a session-private named pipe. Tools in `xai-grok-tools` are a thin client. The host uses WebView2 **DevTools Protocol** (`CallDevToolsProtocolMethod`) plus `Navigate` / `ExecuteScript` so Turbo has the same class of control as chrome-devtools MCP, without sharing the user’s Chrome profile.

**Tech Stack:** Rust, WebView2 (`webview2-com` + `windows` crate), JSON-RPC 2.0 over named pipe, existing `xai-grok-tools` `NewTool` registry, Job Object (`GROK_JOB_OBJECT` / `TURBO_JOB_OBJECT`) so the host dies with Turbo. Windows-first. macOS/Linux tools return a clear “Windows-only in v1” error.

**Product FR:** `fr_01a00c24` (native first-class browser). Do **not** start a `release-dist` rebuild unless the user asks.

---

## Decisions (locked)

1. **Not a ratatui HTML engine.** The TUI cannot paint CSS/JS. The page lives in a real WebView2 window. A later TUI pane only *mirrors* URL + a11y snapshot / screenshot.
2. **Not `--autoConnect` to daily Chrome.** Profile is always `%GROK_HOME%/agent-browser` (`C:\Users\<user>\.grok\agent-browser`). Opt-in “use my Chrome” stays MCP-only and is out of this plan.
3. **Sidecar, not in-process.** crossterm owns the console. WebView2 needs an HWND and a Win32 pump. Same `turbo.exe`, subcommand `browser-host`.
4. **Full control = CDP + script injection**, not “navigate and hope.” Snapshot uses Accessibility.getFullAXTree (or a compact injected uid map). Click/fill use those uids. `browser_eval` is JSON-only (no arbitrary DOM dumps).
5. **No password / 2FA automation.** Tools refuse inputs that look like passwords, OTP, or recovery codes. Human signs in *in the agent window* if a site needs a session.
6. **Windows v1.** Other OS: compile the protocol + tools; host is `cfg(windows)`.
7. **Package-scoped tests.** `cargo test -p xai-grok-browser --lib` covers protocol/client/mock/policy/unit behavior. `cargo test -p xai-grok-browser` also runs integration tests; host UI tests are `#[ignore]` unless `TURBO_WEBVIEW_IT=1`. On Windows, run both commands when validating WebView changes.

---

## File map

| Path | Role |
|------|------|
| `crates/codegen/xai-grok-browser/Cargo.toml` | New crate (lib + optional host bits) |
| `crates/codegen/xai-grok-browser/src/lib.rs` | Public API: protocol, client, profile path |
| `crates/codegen/xai-grok-browser/src/protocol.rs` | JSON-RPC types (requests, results, events) |
| `crates/codegen/xai-grok-browser/src/client.rs` | Named-pipe client used by tools + TUI |
| `crates/codegen/xai-grok-browser/src/profile.rs` | `agent_browser_user_data_dir()` |
| `crates/codegen/xai-grok-browser/src/host/mod.rs` | Windows host (cfg) |
| `crates/codegen/xai-grok-browser/src/host/window.rs` | HWND, title “Turbo Agent Browser” |
| `crates/codegen/xai-grok-browser/src/host/webview.rs` | WebView2 create, CDP, scripts |
| `crates/codegen/xai-grok-browser/src/host/rpc.rs` | Pipe server + dispatch |
| `crates/codegen/xai-grok-browser/src/host/ax.rs` | A11y snapshot → uid list |
| `crates/codegen/xai-grok-browser/src/bin/browser_host.rs` | Thin `main` if we ship a second bin; prefer `turbo browser-host` |
| `Cargo.toml` (workspace) | Add member `crates/codegen/xai-grok-browser` |
| `crates/codegen/xai-grok-pager/src/app/cli.rs` | `Command::BrowserHost(BrowserHostArgs)` |
| `crates/codegen/xai-grok-pager-bin/src/main.rs` | Dispatch `browser-host` **before** TUI init |
| `crates/codegen/xai-grok-pager-bin/Cargo.toml` | Depend on `xai-grok-browser` |
| `crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/` | Agent tools |
| `crates/codegen/xai-grok-tools/src/implementations/grok_build/mod.rs` | `pub mod browser` + re-exports |
| `crates/codegen/xai-grok-tools/src/registry/types.rs` | Register browser tools + inject `BrowserClient` |
| `crates/codegen/xai-grok-shell/src/session/` (new small module or hook in spawn) | Start/stop host with session; assign Job Object |
| `crates/codegen/xai-grok-pager/src/views/agent.rs` | `ActivePane::Browser` (Task 7) |
| `crates/codegen/xai-grok-pager/src/actions/defaults.rs` | `ToggleBrowser` → `Ctrl+Shift+B` (Task 7) |
| `crates/codegen/xai-grok-pager/docs/user-guide/07-mcp-servers.md` | Contrast MCP Chrome vs agent WebView |
| `CHANGELOG.md` | Note when first tools land |
| `bundled/skills/agent-browser/SKILL.md` | When to use agent browser vs chrome-mcp |

---

## Control protocol (v1)

Newline-delimited JSON-RPC 2.0. Client (Turbo) connects to:

```text
\\.\pipe\turbo-browser-<session_id>
```

`session_id` is the existing pager/session UUID (same segment rules as `is_safe_path_segment`).

### Requests (client → host)

```rust
// crates/codegen/xai-grok-browser/src/protocol.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum BrowserRequest {
    #[serde(rename = "browser.navigate")]
    Navigate { url: String },
    #[serde(rename = "browser.tabs")]
    Tabs,
    #[serde(rename = "browser.new_tab")]
    NewTab { url: Option<String> },
    #[serde(rename = "browser.select_tab")]
    SelectTab { tab_id: u32 },
    #[serde(rename = "browser.close_tab")]
    CloseTab { tab_id: u32 },
    #[serde(rename = "browser.snapshot")]
    Snapshot { verbose: bool },
    #[serde(rename = "browser.click")]
    Click { uid: String },
    #[serde(rename = "browser.fill")]
    Fill { uid: String, value: String },
    #[serde(rename = "browser.eval")]
    Eval { function: String },
    #[serde(rename = "browser.screenshot")]
    Screenshot,
    #[serde(rename = "browser.raise")]
    Raise,
    #[serde(rename = "browser.shutdown")]
    Shutdown,
}
```

JSON-RPC envelope (not the enum above on the wire — use standard `{ "jsonrpc":"2.0", "id", "method", "params" }` so a mock host is easy):

```json
{"jsonrpc":"2.0","id":1,"method":"browser.navigate","params":{"url":"https://example.com"}}
{"jsonrpc":"2.0","id":2,"method":"browser.snapshot","params":{"verbose":false}}
```

### Results

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigateResult {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotResult {
    pub url: String,
    pub title: String,
    pub nodes: Vec<AxNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AxNode {
    pub uid: String,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotResult {
    pub path: String,
    pub width: u32,
    pub height: u32,
}
```

Screenshots write to `<session_folder>/images/browser-<n>.png` (same session images dir as Imagine).

### Events (host → client, no `id`)

```json
{"jsonrpc":"2.0","method":"browser.event","params":{"kind":"loaded","url":"…","title":"…"}}
{"jsonrpc":"2.0","method":"browser.event","params":{"kind":"crashed","message":"…"}}
{"jsonrpc":"2.0","method":"browser.event","params":{"kind":"closed"}}
```

### URL / eval policy (shared, tested in the crate)

- Allow: `https:`, `http:` to localhost / RFC1918 / `*.localhost`, `about:blank`.
- Deny: `file:` except under the session folder; `javascript:`; `data:` navigations.
- `browser_eval`: the JS must be a function expression that returns a JSON-serializable value; host wraps with `JSON.stringify`. Cap result at 20_000 bytes (same order as MCP output cap).
- `browser_fill`: reject if `value` matches password/OTP heuristics (length + digit-only 6–8, or field name from snapshot role `textbox` named password). Fail closed with a tool error.

---

## Host process

```
turbo.exe  (TUI / agent)
    │ named pipe
    ▼
turbo.exe browser-host --session-id <id> --user-data-dir <profile>
    │
    ▼
WebView2 (Edge Chromium)  user-data-dir = ~/.grok/agent-browser
```

Launch (shell/session):

```text
<same turbo.exe> browser-host --session-id <sid> --pipe turbo-browser-<sid>
```

- Do **not** initialize crossterm / alternate screen on this argv.
- Assign the child into the parent Job Object when one exists (`docs/windows-process-tree.md`).
- On parent exit, send `browser.shutdown`; if the pipe dies, the host exits.
- Window title: `Turbo Agent Browser` (never “Chrome” / “Edge”).
- First paint: `about:blank` until a navigate.

WebView2 settings (set at create):

- `AreDefaultContextMenusEnabled` = true (human can inspect)
- `AreDevToolsEnabled` = true (F12 in the agent window is OK)
- `IsZoomControlEnabled` = true
- `AreHostObjectsAllowed` = false (no COM host objects)
- User data folder = profile path (not `%LOCALAPPDATA%\Microsoft\Edge`)

CDP methods the host must call:

| Tool | CDP / WebView2 |
|------|----------------|
| navigate | `ICoreWebView2::Navigate` + wait `NavigationCompleted` |
| snapshot | `Accessibility.enable` + `Accessibility.getFullAXTree` → compact `AxNode` list with stable `uid` |
| click | injected script `document.querySelector('[data-turbo-uid="…"]').click()` after tagging nodes, **or** `Input.dispatchMouseEvent` at box |
| fill | focus + `Input.insertText` / set `value` + `input`/`change` events |
| screenshot | `Page.captureScreenshot` → PNG file |
| eval | `Runtime.evaluate` with `returnByValue: true` |
| tabs | WebView2 has one controller per tab; v1 may be **single tab**. If single-tab, `browser.tabs` returns one entry; `new_tab` is deferred to Task 6. |

**v1 tab policy:** single tab is acceptable for the first green host. Multi-tab is Task 6, not a blocker for “full control of the page.”

---

## Agent tools

Register next to `web_fetch` in `registry/types.rs`. Names (stable, no MCP prefix):

| Tool | Maps to |
|------|---------|
| `browser_navigate` | `browser.navigate` |
| `browser_snapshot` | `browser.snapshot` |
| `browser_click` | `browser.click` |
| `browser_fill` | `browser.fill` |
| `browser_eval` | `browser.eval` |
| `browser_screenshot` | `browser.screenshot` |
| `browser_tabs` | `browser.tabs` (+ new/select when Task 6 lands) |

Lazy start: first tool call ensures the host is running (spawn + wait for pipe, timeout 15s). If WebView2 runtime is missing, tool error tells the user to install the Evergreen WebView2 Runtime.

Resource: `BrowserHandle` in tool `Resources` (same pattern as `ImageGenClient`).

---

## Tasks

### Task 1: Crate + protocol + unit tests (no WebView2 yet)

**Files:**
- Create: `crates/codegen/xai-grok-browser/Cargo.toml`
- Create: `crates/codegen/xai-grok-browser/src/{lib,protocol,profile,client}.rs`
- Modify: workspace `Cargo.toml` members list (insert `crates/codegen/xai-grok-browser` alphabetically after `xai-grok-auth`)

- [ ] **Add the crate** with `serde`, `serde_json`, `thiserror`, `tokio` (net/io/sync/rt/macros), `xai-grok-config` (for `grok_home`). No `webview2-com` in this task.

`profile.rs`:

```rust
pub fn agent_browser_user_data_dir() -> PathBuf {
    xai_grok_config::grok_home().join("agent-browser")
}

pub fn pipe_name(session_id: &str) -> String {
    format!(r"\\.\pipe\turbo-browser-{session_id}")
}
```

- [ ] **Protocol tests** in `protocol.rs`:

```rust
#[test]
fn navigate_roundtrip() {
    let v = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "browser.navigate",
        "params": { "url": "https://example.com/" }
    });
    let env: JsonRpcRequest = serde_json::from_value(v).unwrap();
    assert_eq!(env.method, "browser.navigate");
}

#[test]
fn url_policy_allows_https_and_local_http() { /* … */ }

#[test]
fn url_policy_denies_file_and_javascript() { /* … */ }

#[test]
fn fill_rejects_otp_and_password_shaped_values() { /* … */ }
```

- [ ] **Run**

```powershell
cargo test -p xai-grok-browser --lib -- --test-threads=4
```

Expected: pass.

- [ ] **Commit** `feat(browser): protocol crate and URL policy tests`

---

### Task 2: In-process mock host + client

**Files:**
- Create: `crates/codegen/xai-grok-browser/src/mock.rs`
- Modify: `client.rs` to talk JSON-RPC over `async_fn` transport (trait), with a named-pipe impl and a mock impl.

- [ ] **`BrowserTransport` trait** with `call(method, params) -> Result<Value>`.
- [ ] **Mock** stores current URL/title and a canned AX tree. `navigate` updates URL. `snapshot` returns two nodes (`uid=1` link, `uid=2` textbox). `click` / `fill` record last action.
- [ ] **Tests:** navigate → snapshot → click uid 1; fill rejected for `"123456"` OTP; `file:///C:/Windows/notepad.exe` denied **in the client** before send.

- [ ] **Run** `cargo test -p xai-grok-browser --lib -- --test-threads=4`

- [ ] **Commit** `feat(browser): mock host and JSON-RPC client`

---

### Task 3: `turbo browser-host` CLI (Windows stub first)

**Files:**
- Modify: `crates/codegen/xai-grok-pager/src/app/cli.rs` — add:

```rust
    /// Sidecar: Agent WebView2 window (do not run interactively).
    #[command(name = "browser-host", hide = true)]
    BrowserHost(crate::browser_host_cmd::BrowserHostArgs),
```

- Create: `crates/codegen/xai-grok-pager/src/browser_host_cmd.rs` with `BrowserHostArgs { session_id, pipe, user_data_dir }` and `run()` that calls `xai_grok_browser::host::run(...)`.
- Modify: `xai-grok-pager/src/lib.rs` or `app/mod.rs` to `pub mod browser_host_cmd`.
- Modify: `xai-grok-pager-bin/src/main.rs` — match `Command::BrowserHost` **before** TUI / rustls / update. Exit after host returns.
- Modify: `xai-grok-pager-bin/Cargo.toml` + pager `Cargo.toml` to depend on `xai-grok-browser`.

Non-Windows `host::run`: log and exit 2 with “Windows-only”.

- [ ] **Compile** `cargo check -p xai-grok-pager-bin --bin turbo` (package-scoped).
- [ ] **Commit** `feat(browser): turbo browser-host subcommand`

---

### Task 4: Real WebView2 window + navigate + screenshot

**Files:** `host/window.rs`, `host/webview.rs`, `host/rpc.rs` under `cfg(windows)`.

Dependencies (Windows only): `webview2-com`, `windows` (Win32_UI_WindowsAndMessaging, Win32_Foundation, System_Com). Pin versions in the crate; do not add them workspace-wide unless already present.

- [ ] Create a WS_OVERLAPPEDWINDOW titled `Turbo Agent Browser`, 1280×800, not topmost.
- [ ] `CreateCoreWebView2EnvironmentWithOptions` with `user_data_folder` = `--user-data-dir`.
- [ ] Handle `browser.navigate` and `browser.screenshot` (CDP `Page.captureScreenshot`).
- [ ] Manual smoke (not CI):

```powershell
# After a debug turbo is on PATH or target\debug\turbo.exe
$sid = "test-webview-1"
Start-Process .\target\debug\turbo.exe -ArgumentList "browser-host","--session-id",$sid
# From another terminal: a tiny rust example or `echo` JSON to the pipe
```

- [ ] **Integration test** `#[ignore]` `host_navigates_example_com` gated on `TURBO_WEBVIEW_IT=1`.
- [ ] **Commit** `feat(browser): WebView2 host navigate and screenshot`

---

### Task 5: Snapshot / click / fill / eval (full page control)

**Files:** `host/ax.rs`, inject `assets/turbo_ax.js` (keep it tiny).

- [ ] After each load, tag interactive nodes with `data-turbo-uid`.
- [ ] `browser.snapshot` returns compact list (uid, role, name, value, focused). Cap 200 nodes; `verbose=true` raises to 800.
- [ ] `click` / `fill` resolve uid or return `unknown_uid`.
- [ ] `eval` via `Runtime.evaluate`, JSON only.
- [ ] Unit-test AX compaction with a fixture JSON from a saved CDP dump (check in `crates/codegen/xai-grok-browser/tests/fixtures/ax_example.json`).
- [ ] **Commit** `feat(browser): a11y snapshot click fill eval`

---

### Task 6: Multi-tab (optional if Task 4–5 already useful)

- [ ] New tab = new `ICoreWebView2Controller` or `CreateCoreWebView2Controller`.
- [ ] `browser.tabs` / `new_tab` / `select_tab` / `close_tab`. Last tab close does **not** exit the host (goes to `about:blank`).
- [ ] **Commit** `feat(browser): multi-tab host`

Skip this task if timeboxed; tools can stay single-tab.

---

### Task 7: Agent tools + session lifecycle

**Files:**
- Create: `crates/codegen/xai-grok-tools/src/implementations/grok_build/browser/{mod,navigate,snapshot,click,fill,eval,screenshot,tabs}.rs` (or one `mod.rs` if small).
- Modify: `grok_build/mod.rs`, `registry/types.rs`.
- Create: `crates/codegen/xai-grok-shell/src/session/agent_browser.rs` — `ensure_browser_host(session_id)`, `shutdown_browser_host`.
- Wire `ensure` into tool `BrowserHandle` so the first tool call starts the sidecar.
- On session teardown / pager exit: `shutdown`.

Tool tests use the **mock** transport (no WebView2). Follow `web_fetch` / `image_gen` `NewTool` + `ToolMetadata` patterns.

```rust
// navigate input
pub struct BrowserNavigateInput {
    pub url: String,
}
```

- [ ] `cargo test -p xai-grok-tools --lib browser_ -- --test-threads=4`
- [ ] `cargo test -p xai-grok-shell --lib agent_browser -- --test-threads=4`
- [ ] **Commit** `feat(browser): first-class browser_* tools and host lifecycle`

---

### Task 8: TUI pane + shortcut (mirror, not renderer)

**Files:** `views/agent.rs` (`ActivePane::Browser`), layout split (right 40% when open), `actions/defaults.rs` `ToggleBrowser` / `Ctrl+Shift+B`, draw URL + last snapshot lines.

- [ ] Opening the pane **raises** the WebView window (`browser.raise`) if the host is up; it does not create a second engine.
- [ ] Closing the pane does **not** kill the host (agent may still drive it).
- [ ] **Commit** `feat(browser): Ctrl+Shift+B pane mirrors agent WebView`

---

### Task 9: Skill, docs, CHANGELOG, FR

**Files:**
- Create: `bundled/skills/agent-browser/SKILL.md` (and copy to `~/.grok/skills/agent-browser/` only if that is how chrome-mcp was installed — follow existing skill install).
- Modify: `07-mcp-servers.md` — section “Agent WebView vs chrome-devtools MCP”.
- Modify: `CHANGELOG.md` under Added.
- Ship `fr_01a00c24` only when Task 7 is green (tools + host). If only Tasks 1–5: leave FR open.

Skill rules:

- Prefer `browser_*` tools for pages that need JS.
- `web_fetch` for static docs.
- `chrome-devtools` MCP only when the user explicitly wants **their** Chrome.
- Never automate passwords / 2FA.
- Imagine web: agent window, human logs in once in that window if needed.

- [ ] **Commit** `docs(browser): agent WebView skill and user-guide`

---

### Task 10: Safety + doctor

- [ ] `turbo doctor` probe: WebView2 runtime present? profile dir writable?
- [ ] Domain allowlist env `GROK_BROWSER_ALLOW` (optional comma list). Empty = all https + local http. Non-empty = fail closed outside list (except yolo / `permission_mode=always-approve` still **prompts** on form submit if we can detect it — v1: prompt on `browser_click` when the node name matches `/submit|buy|pay|delete|post|send/i`).
- [ ] **Commit** `feat(browser): doctor probe and click confirm heuristics`

---

## Acceptance (MVP done)

1. `turbo browser-host --session-id demo` opens a window titled **Turbo Agent Browser**, not the user’s Chrome.
2. Agent `browser_navigate` to `https://example.com` → snapshot lists the heading / link with uids → `browser_click` works.
3. `browser_eval` returns JSON; oversized / non-JSON fails closed.
4. Daily Chrome tabs and cookies are unchanged (`~/.grok/agent-browser` is the only profile).
5. Killing Turbo (or closing the job) exits the host. No orphan `msedgewebview2.exe` trees after a few seconds.
6. `file:` and `javascript:` navigations are refused.
7. Package tests for protocol + mock tools are green without WebView2.

---

## Out of scope (this plan)

- Rendering HTML inside ratatui / Servo.
- Attaching to the user’s daily Chrome (`--autoConnect`).
- Password / 2FA / cookie import from daily Chrome.
- Production `release-dist` rebuild (ask first).
- macOS WKWebView / Linux WebKitGTK (protocol crate is portable; host is not).
- Replacing `web_fetch` or deleting chrome-devtools MCP.

---

## Suggested first execution cut

Do **Tasks 1–5 and 7**. That is a controllable WebView window + agent tools. Task 8 (TUI pane) and Task 6 (tabs) can follow in the same week if 1–5/7 are green.

**Do not mix this work with the dirty RC2/open-item tree in the same commit.** Implement on a clean branch from `sync/1.0.0-rc1` or commit the plan alone first (this file).

---

## How to start

```powershell
# After this plan is committed:
cargo test -p xai-grok-browser --lib -- --test-threads=4   # Task 1 once the crate exists
```

Two execution options once you say go:

1. **Subagent-driven** — one isolated child per task, review between tasks.
2. **Inline** — implement Tasks 1–2 in this session, stop for a host smoke before WebView2 deps.
