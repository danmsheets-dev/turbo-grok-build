# RC15 Install Instructions (Install Agent Only)

**Do not use these steps during smoke testing of the path-qualified build.**  
Smoke agents must use `target\release-dist\turbo.exe` without overwriting the user install.

---

## Prerequisites

- Windows 10/11 x64  
- Optional: existing Turbo install at `%USERPROFILE%\.turbo\bin` (will be replaced)  
- Close all running `turbo.exe` / TUI sessions before replacing the binary  

---

## Option A — Install **local** release-dist build (recommended after smoke green)

```powershell
$ErrorActionPreference = "Stop"
$Src = "H:\Apps\grok build\turbo-grok-build\target\release-dist\turbo.exe"
if (-not (Test-Path -LiteralPath $Src)) {
  throw "Build missing: $Src — run release-dist build first"
}
$DstDir = Join-Path $env:USERPROFILE ".turbo\bin"
New-Item -ItemType Directory -Path $DstDir -Force | Out-Null
$Dst = Join-Path $DstDir "turbo.exe"

# Windows cannot overwrite a running image — rename-aside if locked
if (Test-Path -LiteralPath $Dst) {
  $Aside = Join-Path $DstDir ("turbo.exe.prev.{0}" -f (Get-Date -Format "yyyyMMddHHmmss"))
  try {
    Move-Item -LiteralPath $Dst -Destination $Aside -Force
  } catch {
    throw "Could not move existing turbo.exe (is it running?): $_"
  }
}
Copy-Item -LiteralPath $Src -Destination $Dst -Force

# PATH check
$Bin = $DstDir
if ($env:Path -notlike "*$Bin*") {
  Write-Host "Add to user PATH if needed: $Bin"
  # Optional permanent user PATH:
  # [Environment]::SetEnvironmentVariable("Path", $env:Path + ";$Bin", "User")
}

& $Dst --version
Write-Host "Installed: $Dst"
```

---

## Option B — Official GitHub installer (published release only)

When a GitHub Release tag exists for this version:

```powershell
# Latest
irm https://github.com/danmsheets-dev/turbo-grok-build/releases/latest/download/install.ps1 | iex

# Pin (example)
# irm https://github.com/danmsheets-dev/turbo-grok-build/releases/latest/download/install.ps1 | iex
# or: powershell -ExecutionPolicy Bypass -File install.ps1 -Version 0.2.119-r1
```

From a clone:

```powershell
cd "H:\Apps\grok build\turbo-grok-build"
powershell -ExecutionPolicy Bypass -File .\install.ps1
# powershell -ExecutionPolicy Bypass -File .\install.ps1 -Version 0.2.119-r1
```

Installer:

- Downloads x86_64-pc-windows-msvc artifact  
- Verifies SHA-256 via release `SHA256SUMS`  
- Installs to `%USERPROFILE%\.turbo\bin\turbo.exe`  

---

## Option C — Build then install (full pipeline)

```powershell
cd "H:\Apps\grok build\turbo-grok-build"
$env:RUST_MIN_STACK = "16777216"
$env:GROK_VERSION = (Get-Content VERSION -Raw).Trim()
cargo build -p xai-grok-pager-bin --profile release-dist --bin turbo
# Then Option A copy steps
```

---

## Verify install

```powershell
where.exe turbo
turbo --version
# Expect VERSION content, e.g. 0.2.119-r1
```

## Rollback

```powershell
$Bin = Join-Path $env:USERPROFILE ".turbo\bin"
# Restore newest turbo.exe.prev.* if present
Get-ChildItem $Bin -Filter "turbo.exe.prev.*" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
```

---

## Safety

- Never install over a running `turbo.exe` without rename-aside  
- Do not delete the monorepo `target\release-dist\turbo.exe` after install (keep as rebuild source)  
- Community updater uses `~/.turbo` — do not point it at `~/.grok`  
