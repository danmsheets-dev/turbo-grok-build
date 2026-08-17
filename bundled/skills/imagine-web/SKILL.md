---
name: imagine-web
description: >
  Drive the grok.com Imagine website in Chrome (chrome-devtools MCP) to
  generate images via the web UI, not the image_gen API. Use when the user
  asks to log into xAI/grok.com, use Imagine on the website, or generate
  images through the web interface. Slash command: /imagine-web
metadata:
  short-description: "Generate images via grok.com Imagine in the browser"
---

# Imagine web

Use chrome-devtools MCP to operate the **grok.com Imagine website**. Do **not**
call `image_gen` / `image_edit` / `/imagine` unless the user explicitly wants
the API instead.

## Tools

1. `search_tool` for `chrome-devtools` first. Use the schemas it returns.
   Do not invent tool names or arguments.
2. Primary loop: `list_pages` → `new_page` or `navigate_page` → `take_snapshot`
   → uid `fill` / `click` / `fill_form` → `wait_for`.
3. Prefer `take_snapshot` over `take_screenshot` for finding controls.
4. `evaluate_script` is last resort (read an `<img src>`, trigger a download).

## Login (human only)

- Never put a password, 2FA code, or recovery key in a tool argument.
- If the page is a login / SSO / challenge wall: **stop**, tell the user to
  finish login in the headed Chrome window, then wait for them to say they
  are in.
- Prefer the **daily Chrome** pin (`--autoConnect`). Enable remote
  debugging at `chrome://inspect/#remote-debugging` (Chrome 144+) so
  grok.com is already logged in. The isolated `~/.grok/browser-profile`
  is Cloudflare-blocked on Google SSO (`accounts.x.ai/api/rpc` 403).
- `turbo login` is the API session and does **not** log this browser in.
- Also load `chrome-mcp` if the MCP child looks stale (`about:blank` only).

## Generate

1. Open `https://grok.com/imagine` (title **Grok Imagine**). If that 404s,
   fall back to `https://grok.com` and click Imagine from a snapshot.
2. Home shows templates plus a form: textbox **Ask Grok anything**,
   **Submit**, **Upload**, radios **Image** / **Video** / **Agent**,
   **Speed** / **Quality (v2.0)**, **Aspect Ratio**. Snapshot again after
   load — do not reuse stale uids.
3. Leave **Image** selected unless the user asked for video. `fill` the
   prompt box, then click **Submit** (disabled until the box has text).
4. `wait_for` result text (Download, Share, or similar) or poll snapshots.
   Generation can take more than a minute.
5. Save the image into the **session** `images/` folder (create it if needed):
   - Best: download the result image (`list_network_requests` filtered to
     `image`, or `evaluate_script` for the result `<img src>`), then write
     bytes to `images/<n>.jpg`.
   - Fallback: `take_screenshot` with `filePath` set to that session path
     (`--allow-unrestricted-paths` is on the Turbo MCP pin).
   - If filePath is rejected (temp-only MCP): screenshot to temp, then copy
     into the session folder with a file write.
6. Tell the user the short path (`images/N.jpg`), not the absolute path.

## Fail closed

- Paywall / SuperGrok / quota: stop and report the on-page message. Do not
  retry the API as a silent substitute.
- Bot check / Cloudflare: stop and ask the human to complete it in the window.
- UI changed: take a new snapshot and continue. Do not hard-code last week's
  uids.
- Unknown domain: do not leave grok.com / x.ai / accounts.x.ai unless the
  user asked.
