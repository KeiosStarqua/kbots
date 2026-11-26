# Scoop bucket

## Purpose

- Personal Scoop bucket that installs kbots natively on Windows. The manifest downloads a pinned source commit and builds the CLI, TUI and GUI with the repo's own `install.ps1`. No binary release or git tag is needed.

## Ownership

- `bucket/kbots.json` is the only published artifact in this folder. Other JSON files at the repo root are not Scoop manifests.

## Local Contracts

- The manifest pins a commit that is pushed to `origin`. `version`, `url`, `hash` and `extract_dir` always move together. The commit must contain `install.ps1` and native Windows support.
- `url` is the GitHub archive of that commit; `hash` is the SHA-256 of the downloaded zip; `extract_dir` is `kbots-<full-sha>` (GitHub's archive root).
- Installation delegates to `install.ps1` with `-InstallDir $dir -NoPath -NoDaemon`. Do not duplicate the Bun/Rust build logic here, and do not let the Scoop install start a daemon.
- Local installs never create git tags. `install.ps1` only writes a local CalVer to `.deploy-version` in the app directory; `kbdeploy` remains the only tag source.
- `install.ps1` must tolerate a source archive with no `.git`: with `$ErrorActionPreference='Stop'`, redirecting native git stderr to `$null` throws in Windows PowerShell 5.1, so the dirty check uses `Test-Path .git` instead.
- `depends` lists `bun`, `rustup`, `mingw` and `git`. MinGW's `bin` is added to `PATH` for `dlltool` before the build.
- The installer forces `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu` (installing that toolchain if missing) without changing the user's global default. An MSVC-only toolchain cannot link the GUI/Slint build because `link.exe` is not installed.
- Shims: `kbots`, `kb` (alias of `kbots.exe`), `kbots-tui`, `kbots-gui`; Start Menu shortcut `kbots Control Center`.

## Work Guidance

- To bump the installed version, pick a pushed commit, download its archive, recompute the SHA-256, update all four fields, then verify a fresh install before committing. Do not create a deploy tag just to bump the bucket.
- The manifest is referenced by the `kbots` bucket entry in the `skoopub` repo's `scoop.json`; keep the version in sync when that repo is updated.
- Keep the daemon's repo root in `%USERPROFILE%\.local\state\kbots\client.json` — `install.ps1` writes it, and `kbots daemon start` needs it. It must be UTF-8 without BOM; Rust's `serde_json` rejects a BOM and would fall back to the caller's cwd.

## Verification

- `scoop install kbots/kbots` from a clean shell, then `kbots daemon start` and `kb status`.
- `Get-Content bucket/kbots.json -Raw | ConvertFrom-Json` to validate the manifest syntax.

## Child DOX Index

- None.
