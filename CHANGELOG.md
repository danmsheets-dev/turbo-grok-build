# Changelog

All notable changes to **Turbo Grok Build** (`turbo` binary).

Format: [Keep a Changelog](https://keepachangelog.com/).  
Wire versions: [`VERSION`](./VERSION) (`0.2.114-rN` community RC line).

English-only product surface (UI and public docs) as of RC14.

---

## Pedigree (community line)

Turbo Grok Build evolved from the Hyper community fork of
[xAI Grok Build](https://github.com/xai-org/grok-build). Multi-agent work
accelerated at r6; product rebrand to **Turbo** at r10.

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
| **r14** | **`0.2.114-r14`** | **web_fetch**, **workflow routing**, English-only |

Older release notes (r1–r13 detail) are archived under
[`docs/archive/`](./docs/archive/).

---

## [Unreleased]

### Fixed (post-RC14 brand polish)
- Finish **hyper → turbo** leftovers: community updater + install URLs point at
  `danmsheets-dev/turbo-grok-build`, test harness looks up `turbo` binary, nested
  confine allowlist includes `turbo`, recovery text / docs use `turbo` CLI names,
  flake homepage + archive-entry tests aligned.

### Planned (RC15 candidates — feature request log)
Rust `target/` disk hygiene product surface (Windows monorepo often 100–300 GB):
- `turbo disk report` / `turbo disk clean --safe`
- Doctor free-space probe + agent preflight before heavy cargo / release-dist
- Windows PDB / light-agent cargo policy; post-ship cache trim (keep `turbo.exe`)
- Stale absolute-path target self-heal after renames; optional `turbo build ship`
- Auto developer_log on disk pressure

Review: `turbo features list` / `turbo features export`.

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
