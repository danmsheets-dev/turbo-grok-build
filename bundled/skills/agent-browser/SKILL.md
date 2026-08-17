---
name: agent-browser
description: >
  Drive Turbo's isolated Agent WebView (browser_* tools). Use when pages need
  JS, a headed window, or a dedicated profile — not the user's daily Chrome.
metadata:
  short-description: "Operate the Turbo Agent WebView"
---

# Agent WebView

Turbo owns a **WebView2** window titled **Turbo Agent Browser**. The sidecar is
`turbo browser-host`. The profile is always `~/.grok/agent-browser`. This is
**not** chrome-devtools MCP and never attaches to the user's daily Chrome.

v1 is **Windows-only**. On other OS the `browser_*` tools fail closed with a
clear Windows-only error.

## When to use which surface

| Need | Use |
|------|-----|
| JS, headed window, login UI, interactive page | `browser_*` (this skill) |
| Static docs / article markdown | `web_fetch` |
| The user's **daily Chrome** tabs or cookies | chrome-devtools MCP — **only** if they asked |

Prefer `browser_*` whenever the page needs JavaScript. Do not reach for
chrome-devtools just because a page is "the web."

## Tools

First-class GrokBuild tools (no `search_tool` / `use_tool`):

| Tool | Role |
|------|------|
| `browser_navigate` | Load `https:`, local `http:`, or `about:blank`. `file:` and `javascript:` are refused. |
| `browser_snapshot` | Compact a11y tree (`uid` / role / name / value / focused). Always before click/fill. |
| `browser_click` | Click by **latest** snapshot `uid`. Unknown uids fail closed. |
| `browser_fill` | Type into a `uid`. Refuses OTP/PIN (6–8 digits) and password-shaped values. |
| `browser_eval` | Last resort. Pass a function expression (`() => document.title`). JSON only; no DOM dumps. |
| `browser_screenshot` | PNG of the Agent window. Tell the user the short path (`images/browser-1.png`). |
| `browser_tabs` | List tabs (`url`, `title`, `tab_id`, `active`). v1 is typically one tab. |

## Loop

1. `browser_navigate` to the URL (or `browser_tabs` if a session is already up).
2. `browser_snapshot` — **always** before click/fill. Uids die after navigation.
3. `browser_click` / `browser_fill` by the **latest** uid.
4. Snapshot again after the page changes. Prefer snapshot over screenshot for structure.
5. `browser_eval` only when a11y is not enough.

Do not invent CSS selectors. Do not reuse stale uids.

## Host, profile, TUI

- Profile: **`~/.grok/agent-browser`** (`$GROK_HOME/agent-browser`). Never the
  user's Chrome user-data-dir. Daily tabs and cookies stay untouched.
- Sidecar: **`turbo browser-host`**. The first `browser_*` call lazy-starts it
  for this pager session. Do not run `browser-host` interactively unless the
  user asked to smoke the window.
- **Ctrl+Shift+B** toggles a TUI **mirror** of the current URL and last
  snapshot. It does **not** render HTML. Opening the pane raises the existing
  WebView window if the host is running; closing the pane does not stop the
  host. `Ctrl+B` still backgrounds the running task.

WebView2 runtime missing: tell the user to install the Evergreen WebView2
Runtime (https://developer.microsoft.com/microsoft-edge/webview2/).

## Login (human only)

- Never put a password, 2FA code, OTP, or recovery key in a tool argument.
  `browser_fill` rejects those shapes; do not work around it with `browser_eval`.
- Login wall / SSO / Cloudflare / challenge: **stop**, ask the human to finish
  **in the Agent WebView window**, wait until they say they are in.
- **Imagine web:** stay in the Agent window. The human logs in **once** in that
  window if needed. Do not switch to chrome-devtools for Imagine unless they
  explicitly want their daily Chrome.
- `turbo login` is the **API** session. It does not log this WebView in.

## Writes (X, LinkedIn, email, …)

Draft in the composer. **Do not Send/Post** unless the user explicitly said
to send. Prefer: fill the reply, snapshot, ask the user to click Send.

Stay on the site they named. No lead-list blasting, no credential stuffing.

## Fail closed

- Unknown domain: do not wander unless the user asked.
- Missing session / host down: report the tool error. Do not invent page contents.
- Non-Windows: report Windows-only in v1. Do not fall back to chrome-devtools
  unless they asked for **their** Chrome.
