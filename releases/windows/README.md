# Windows release binaries

Prebuilt **`turbo.exe`** builds for Windows (x86_64) ship as **GitHub Release
assets** (not in the git tree — ~149 MB is over the regular git object limit,
and public-fork LFS uploads are blocked on GitHub).

| Wire version | Release | Asset |
| --- | --- | --- |
| `1.0.0-rc.4` | [v1.0.0-rc.4](https://github.com/danmsheets-dev/turbo-grok-build/releases/tag/v1.0.0-rc.4) | `turbo-1.0.0-rc.4-x86_64-pc-windows-msvc.exe` |
| `1.0.0-rc.1` | [v1.0.0-rc.1](https://github.com/danmsheets-dev/turbo-grok-build/releases/tag/v1.0.0-rc.1) | `turbo-1.0.0-rc.1-x86_64-pc-windows-msvc.exe` |

## Install

```powershell
# Download the latest RC4 Windows asset from GitHub Releases, then:
$dest = Join-Path $env:USERPROFILE ".turbo\bin"
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Copy-Item -Force .\turbo-1.0.0-rc.4-x86_64-pc-windows-msvc.exe (Join-Path $dest "turbo.exe")
turbo version
```

Or point the Grok Build Claude Code plugin at the downloaded artifact:

```powershell
$env:GROK_BINARY = (Resolve-Path .\turbo-1.0.0-rc.4-x86_64-pc-windows-msvc.exe).Path
```

## Verify

```text
turbo version
# → turbo 1.0.0-rc.4 (<commit>)
```

Source tree `VERSION` may be newer than the last GitHub asset. Rebuild with
cargo for the latest wire; release assets are frozen ship builds only.

## Local packaging note

When building a ship binary, place it at:

`releases/windows/turbo-<version>-x86_64-pc-windows-msvc.exe`

and attach it to a GitHub Release (`gh release upload …`). The path is
gitignored for the `.exe`; this README stays tracked.
