# Windows release binaries

Prebuilt Windows (x86_64) builds ship as **GitHub Release** zip archives plus
`SHA256SUMS` (not in the git tree — ~150 MB is over the regular git object
limit, and public-fork LFS uploads are blocked on GitHub).

`install.ps1` and `turbo update` require **exactly** these names:

| Wire version | Release | Asset |
| --- | --- | --- |
| `1.0.0-rc.5` | [v1.0.0-rc.5](https://github.com/danmsheets-dev/turbo-grok-build/releases/tag/v1.0.0-rc.5) | `turbo-1.0.0-rc.5-x86_64-pc-windows-msvc.zip` |
| `1.0.0-rc.4` | [v1.0.0-rc.4](https://github.com/danmsheets-dev/turbo-grok-build/releases/tag/v1.0.0-rc.4) | `turbo-1.0.0-rc.4-x86_64-pc-windows-msvc.zip` |

A raw `.exe` on the release is a convenience copy only; auto-update ignores it.

## Install

```powershell
irm https://raw.githubusercontent.com/danmsheets-dev/turbo-grok-build/dev/install.ps1 | iex
turbo update --check
# → turbo 1.0.0-rc.5 (latest: 1.0.0-rc.5) [stable]
```

Or pin: `.\install.ps1 -Version v1.0.0-rc.5`

The zip must contain a root-level `turbo.exe` plus `bundled/`. The installer
activates the binary at `%USERPROFILE%\.turbo\bin\turbo.exe` and the bundle at
`%USERPROFILE%\.grok\bundled`.

## Verify

```text
turbo version
# → turbo 1.0.0-rc.5 (<commit>)
```

## Local packaging note

`release.yml` on tag `v*` builds `turbo-<version>-x86_64-pc-windows-msvc.zip`.
Do not pass `/DEBUG:LongSymbolTruncate` (GitHub `link.exe` 14.44 → LNK1117).
The path `releases/windows/*.exe` is gitignored; this README stays tracked.
