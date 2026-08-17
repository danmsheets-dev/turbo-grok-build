---
name: chrome-mcp
description: >
  Drive a real Chrome via the chrome-devtools MCP server (navigate, snapshot,
  click, fill, screenshot). Use when browsing the web, logging into sites,
  automating grok.com / X / LinkedIn in the user's Chrome, or when
  web_fetch headless is not enough. Slash command: /chrome-mcp
metadata:
  short-description: "Operate Chrome through chrome-devtools MCP"
---

# Chrome MCP

Turbo talks to **chrome-devtools MCP**, not a built-in browser. Discover tools
with `search_tool` (`chrome-devtools`) and use the returned schemas only.

## Loop

1. `mcp_server_health` / `search_tool` — confirm chrome-devtools is ready.
2. `list_pages` — pick an existing tab or `new_page`.
3. `navigate_page` if the tab is wrong.
4. `take_snapshot` — **always** before click/fill. Uids die after navigation.
5. `fill` / `fill_form` / `click` / `press_key` by **latest** uid.
6. `wait_for` text you expect. Prefer snapshot over screenshot for structure.

`evaluate_script` is last resort. Do not invent CSS selectors.

## How Chrome is launched

Pinned in `~/.grok/config.toml` as `[mcp_servers.chrome-devtools]`.

**`--autoConnect` (current pin):** attach to the user's **daily Chrome**
(Chrome 144+). Requires remote debugging:

1. Open Chrome.
2. Visit `chrome://inspect/#remote-debugging` and enable it.
3. Keep that Chrome running. Then `list_pages` should show real tabs
   (gmail, x.com, grok.com) — not a fresh `about:blank`.

If `list_pages` is only `about:blank`, remote debugging is off or the MCP
child still has old argv. Run `turbo mcp restart chrome-devtools` or start
a new Turbo session. `/mcps` → `r` also reloads.

**Dedicated profile** (`--userDataDir ~/.grok/browser-profile`): isolated
from daily Chrome. grok.com **Google SSO is Cloudflare-blocked** in that
automation profile. Do not use it for Gmail login. Prefer `--autoConnect`.

`--allow-unrestricted-paths` lets `take_screenshot filePath` write into the
session `images/` folder.

## Login

- Never put a password, 2FA code, or recovery key in a tool argument.
- Login wall / Cloudflare / challenge: **stop**, ask the human to finish in
  the headed Chrome window, wait until they say they are in.
- `turbo login` is the **API** session. It does not log this Chrome in.

## Writes (X, LinkedIn, email, …)

Draft in the composer. **Do not Send/Post** unless the user explicitly said
to send. Prefer: fill the reply, snapshot, ask the user to click Send.

Stay on the site they named. No lead-list blasting, no credential stuffing.

## Imagine web

If the task is grok.com Imagine, load the `imagine-web` skill and follow it.
Do not call `image_gen` unless they asked for the API.
