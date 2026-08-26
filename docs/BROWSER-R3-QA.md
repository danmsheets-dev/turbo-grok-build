# Agent WebView Round 3 Q&A (rc.8 workstream C)

Live checklist for the product-owned WebView (`browser_*` / `turbo browser-host`).
This is **not** chrome-devtools MCP.

Use a **path-qualified** `turbo.exe` on Windows. Fill Observed / Pass? on a
headed pass. Unit-proven items are already covered by package tests:

```powershell
cargo test -p xai-grok-browser --lib --offline
cargo test -p xai-grok-tools --lib browser --offline -- --test-threads=4
```

| Provenance | Meaning |
|------------|---------|
| **Unit** | Fail-closed in `xai-grok-browser` / `xai-grok-tools` lib tests. |
| **Human** | Needs a headed WebView2 window. Do not skip for ship. |

---

## C.1 Navigate

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.1.1 | `browser_navigate` `https://example.com/` | Loads; title/url returned | Unit + Human | | |
| C.1.2 | `file:///C:/Windows/notepad.exe` | Denied; mock stays `about:blank` | Unit | | |
| C.1.3 | `javascript:` / `data:` / public `http:` | Denied | Unit | | |
| C.1.4 | Redirect (302) to public http / off-allowlist https | Hop cancelled (`NavigationStarting`) | Unit (policy) + Human (live 302) | | |
| C.1.5 | Page iframes `http://example.com/` | Frame hop cancelled | Unit (same hop check) + Human | | |
| C.1.6 | Click a link to `javascript:` or denied host | Click surfaces BlockLog; page does not leave | Unit (policy) + Human | | |
| C.1.7 | Missing URI on `NavigationStarting` | Cancel (fail closed) | Unit | | |
| C.1.8 | HTTP 404 HTML page | Navigate succeeds (document exists) | Unit (commented path) + Human | | |

## C.2 Close hides

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.2.1 | Click the window **X** | Frame **hides**; host keeps serving the pipe | Human (`WM_CLOSE` → `SW_HIDE`) | | |
| C.2.2 | Next `browser_*` (not shutdown) | Window re-shows (`ensure_visible`) | Human | | |
| C.2.3 | `browser.shutdown` / pager teardown | Host actually quits | Human | | |

## C.3 Raise

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.3.1 | `browser_raise` after hide | Window restored to front | Human | | |
| C.3.2 | Ctrl+Shift+B while host is up | TUI pane opens **and** raise is fired | Human | | |
| C.3.3 | Minimized window + raise | Restored (not left iconic) | Human | | |

## C.4 Login HITL

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.4.1 | Password / OTP field | `browser_fill` refuses; human types in the Agent window | Unit (fill policy) + Human | | |
| C.4.2 | Sign-in button | Does **not** require `confirm=true` | Unit | | |
| C.4.3 | Cloudflare / SSO wall | Agent `browser_raise`s and waits; no password in tool args | Human | | |

## C.5 OAuth popup

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.5.1 | `accounts.google.com` / Microsoft login popup | Host-owned popup HWND (`SetNewWindow` + hop policy); not the only tab | Unit (URL detect) + Human | | |
| C.5.2 | `https://evil.test/accounts.google.com/gsi` | **Not** treated as OAuth; policy applies | Unit | | |
| C.5.3 | Ordinary `target=_blank` | Same window (single tab) | Human | | |

## C.6 Snapshot click

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.6.1 | Snapshot then click `4-17`-shaped uid | Clicks tagged DOM node | Unit (mock) + Human | | |
| C.6.2 | Positional `"2"` | Refused with epoch-index hint | Unit | | |
| C.6.3 | Click after **successful** navigate without new snapshot | Fail closed (`call browser_snapshot`) | Unit | | |
| C.6.4 | Click uid from epoch 1 after snapshot epoch 2 | Fail closed (`stale_uid` / unknown uid) | Unit | | |
| C.6.5 | AX-fallback snapshot uids | Read-only; even `confirm=true` refused | Unit | | |
| C.6.6 | Denied navigate | Previous snapshot **kept** | Unit | | |

## C.7 Confirm submit

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.7.1 | Click "Submit" / "Apply" / "Connect" without confirm | Refused | Unit | | |
| C.7.2 | Same with `confirm=true` after user approval | Allowed | Unit + Human | | |
| C.7.3 | `browser_eval` `el.click()` / `form.submit()` / `location.assign` / password assign without confirm | Refused at **tool, client, host, mock** | Unit | | |
| C.7.4 | `() => document.title` | Allowed without confirm | Unit | | |
| C.7.5 | Confirmed mutating eval | Reaches host | Unit | | |

## C.8 Save PDF

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.8.1 | Inline PDF with empty DOM | Snapshot hints `browser_save` | Unit | | |
| C.8.2 | `browser_save` https PDF | File in session `downloads/` with sanitized name | Unit (save policy) + Human | | |
| C.8.3 | `browser_save` redirect to public http | Blocked | Unit | | |
| C.8.4 | Size cap / reserved names / symlink jail | Fail closed (rc.7, not weakened) | Unit | | |

## C.9 Downloads `wait_ms`

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.9.1 | Attachment → brokered file listed | `report.pdf` in listing, `completed=true` | Unit (host + mock) | | |
| C.9.2 | `wait_ms` > 60s | Clamped to 60s | Unit | | |
| C.9.3 | JS download interstitial | `wait_ms` until completed file appears | Human | | |
| C.9.4 | Traversal / `CON` / `LPT1` names | Rejected; fallback `download.bin` | Unit | | |
| C.9.5 | Broker folder is a file (not a dir) | Listing/broker refused | Unit | | |

## C.10 Denied URL keeps snapshot

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.10.1 | Snapshot, then `data:` navigate | Error; click on previous uid still works | Unit | | |

## C.11 Host Job Object

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.11.1 | Spawn path | `browser-host` attached to `ProcessGroup` / global process scope | Code (`agent_browser.rs`) | | |
| C.11.2 | Kill Turbo (Job Object / `--job-object`) | `turbo browser-host` **and** `msedgewebview2.exe` die | Human | | |
| C.11.3 | Parent exit without shutdown | Pipe drop → host exits | Human | | |

## C.12 Ctrl+Shift+B live mirror

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.12.1 | Snapshot then toggle pane | `last_url` is the page URL, **not** `about:blank` | Unit (`browser_pane_mirror`) | | |
| C.12.2 | Stale `host_running=false` with cached snapshot | Toggle refresh marks host up and shows URL | Unit | | |
| C.12.3 | Headed: navigate + snapshot + Ctrl+Shift+B | TUI shows URL + uid lines; does not render HTML | Human | | |

## C.13 Durable profile / single tab (should)

| # | Q / Action | Expected | Provenance | Observed | Pass? |
|---|------------|----------|------------|----------|-------|
| C.13.1 | `GROK_BROWSER_PROFILE=durable` | `$GROK_HOME/agent-browser` (same as default; shared cookies) | Unit | | |
| C.13.2 | Default | `$GROK_HOME/agent-browser` (shared cookies) | Unit | | |
| C.13.3 | `GROK_BROWSER_FRESH_PROFILE=1` | temp dir | Unit | | |
| C.13.4 | `browser.new_tab` / `select_tab` / `close_tab` | `v1 is a single tab` (not sent) | Unit | | |

---

## Headed smoke (minimum)

1. `browser_navigate` https://example.com/ → snapshot → click a link.
2. Close the window with **X** → `browser_raise` (or any `browser_*`) shows it again.
3. Google/Microsoft sign-in popup stays a real window.
4. Inline PDF → `browser_save` → `browser_downloads`.
5. Denied URL (`data:`) after a snapshot → click still uses the old uid.
6. Quit Turbo; confirm no leftover `msedgewebview2.exe`.

## Residual (not unit-provable here)

- Live 302 / iframe / click hops through WebView2 COM (`TURBO_WEBVIEW_IT=1`).
- Close-hides + raise + OAuth popup + Job Object kill tree.
- JS download interstitial timing (`wait_ms`).
- Durable profile actually persisting LinkedIn cookies across pager sessions (path is unit-proven; cookie jar is headed).
