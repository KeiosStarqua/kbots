<#
.SYNOPSIS
    Install kbots natively on Windows: CLI (kbots/kb), TUI, GUI and a local daemon.
.DESCRIPTION
    No administrator rights and no WSL required. Builds from this checkout with
    Bun + Rust, copies the binaries to %LOCALAPPDATA%\kbots\bin by default,
    adds that directory to the user PATH, records the repo root so `kbots daemon
    start` works from any directory, and optionally starts the daemon.
.PARAMETER InstallDir
    Destination for the binaries (default: %LOCALAPPDATA%\kbots\bin).
.PARAMETER NoPath
    Do not add InstallDir to the per-user PATH.
.PARAMETER Autostart
    Also add a per-user Startup shortcut that starts the daemon at logon.
.PARAMETER NoDaemon
    Do not start the daemon after a successful install.
.PARAMETER RemoteHost
    Save HOST:PORT for a remote daemon instead of starting a local one.
#>
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\kbots\bin",
    [switch]$NoPath,
    [switch]$Autostart,
    [switch]$NoDaemon,
    [string]$RemoteHost
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repoRoot = $PSScriptRoot

function Get-Tool([string]$name) {
    $found = Get-Command $name -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    # Scoop installs are per-user; cargo lives outside PATH on fresh shells.
    $scoopCargo = "$env:USERPROFILE\scoop\apps\rustup\current\.cargo\bin\cargo.exe"
    if ($name -eq 'cargo' -and (Test-Path $scoopCargo)) { return $scoopCargo }
    return $null
}

# MinGW's dlltool is needed by the windows-gnu Rust target for slint/skia.
$scoopMingw = "$env:USERPROFILE\scoop\apps\mingw\current\bin"
if (Test-Path $scoopMingw) { $env:PATH = "$scoopMingw;$env:PATH" }

$bun = Get-Tool 'bun'
$cargo = Get-Tool 'cargo'
$git = Get-Tool 'git'
if (-not $bun -or -not $cargo) {
    throw "Bun and Rust are required. Without admin rights: scoop install bun rustup mingw (or see https://bun.sh and https://rustup.rs)."
}

Push-Location $repoRoot
try {
    Write-Host 'Installing Bun dependencies from the frozen lockfile...'
    & $bun install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "bun install failed ($LASTEXITCODE)" }

    Write-Host 'Type-checking...'
    & $bun run tsc --noEmit
    if ($LASTEXITCODE -ne 0) { throw "TypeScript check failed ($LASTEXITCODE)" }

    foreach ($manifest in @('gui\Cargo.toml', 'tui\control-center\Cargo.toml', 'cli\Cargo.toml')) {
        Write-Host "Building $manifest (release)..."
        & $cargo build --release --manifest-path (Join-Path $repoRoot $manifest)
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed for $manifest ($LASTEXITCODE)" }
    }
}
finally { Pop-Location }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Force (Join-Path $repoRoot 'cli\target\release\kbots.exe') (Join-Path $InstallDir 'kbots.exe')
Copy-Item -Force (Join-Path $repoRoot 'tui\control-center\target\release\kbots-tui.exe') (Join-Path $InstallDir 'kbots-tui.exe')
Copy-Item -Force (Join-Path $repoRoot 'gui\target\release\kbots-control-center.exe') (Join-Path $InstallDir 'kbots-gui.exe')
$kbShim = Join-Path $InstallDir 'kb.cmd'
Set-Content -Path $kbShim -Encoding ASCII -Value "@echo off`r`n`"$InstallDir\kbots.exe`" %*`r`n"

if (-not $NoPath) {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ($userPath -notlike "*$InstallDir*") {
        $separator = if ($userPath) { ';' } else { '' }
        [Environment]::SetEnvironmentVariable('Path', "$userPath$separator$InstallDir", 'User')
    }
    $env:PATH = "$InstallDir;$env:PATH"
}

# Same config file the Rust clients read. `kbots daemon start` uses repoRoot
# when invoked from any directory; host switches the clients to remote mode.
$stateDir = Join-Path $env:USERPROFILE '.local\state\kbots'
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
$configPath = Join-Path $stateDir 'client.json'
$config = if (Test-Path $configPath) { Get-Content $configPath -Raw | ConvertFrom-Json } else { [PSCustomObject]@{} }
$config | Add-Member -NotePropertyName repoRoot -NotePropertyValue $repoRoot -Force
if ($RemoteHost) { $config | Add-Member -NotePropertyName host -NotePropertyValue $RemoteHost -Force }
Set-Content -Path $configPath -Encoding UTF8 -Value ($config | ConvertTo-Json -Compress)

$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
if (Test-Path $startMenu) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path $startMenu 'kbots Control Center.lnk'))
    $shortcut.TargetPath = Join-Path $InstallDir 'kbots-gui.exe'
    $shortcut.WorkingDirectory = $repoRoot
    $shortcut.Save()
}
if ($Autostart) {
    $startup = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Startup'
    New-Item -ItemType Directory -Force -Path $startup | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut((Join-Path $startup 'kbots daemon.lnk'))
    $shortcut.TargetPath = Join-Path $InstallDir 'kbots.exe'
    $shortcut.Arguments = 'daemon start'
    $shortcut.WorkingDirectory = $repoRoot
    $shortcut.WindowStyle = 7
    $shortcut.Save()
}

# CalVer deploy tag, same scheme as install.sh on Unix hosts. Check for .git
# directly: redirecting native git stderr with 2>$null becomes a terminating
# NativeCommandError under $ErrorActionPreference='Stop' when this is a source
# archive (e.g. a Scoop install) rather than a checkout.
if ($git -and (Test-Path (Join-Path $repoRoot '.git'))) {
    $date = Get-Date -Format 'yyyy.MM.dd'
    $n = 1
    while (& $git -C $repoRoot rev-parse -q --verify "refs/tags/deploy/$date.$n" 2>$null) { $n++ }
    $version = "$date.$n"
    $dirty = & $git -C $repoRoot status --porcelain
    & $git -C $repoRoot tag -a "deploy/$version" -m "Deploy deploy/$version"
    if ($LASTEXITCODE -eq 0) {
        $record = if ($dirty) { "$version+dirty" } else { $version }
        Set-Content -Path (Join-Path $repoRoot '.deploy-version') -Encoding ASCII -Value $record
        if ($dirty) { Write-Warning "Working tree is dirty: deployed code is not exactly tag deploy/$version." }
        Write-Host "Tagged deploy/$version"
    }
}

Write-Host "`nInstalled:`n  $InstallDir\kbots.exe`n  $InstallDir\kb.cmd (alias)`n  $InstallDir\kbots-tui.exe`n  $InstallDir\kbots-gui.exe"
if (-not $NoPath -and ($env:PATH -notlike "*$InstallDir*")) {
    Write-Host "Open a new terminal so the updated PATH takes effect."
}
if (-not $NoDaemon -and -not $RemoteHost) {
    & (Join-Path $InstallDir 'kbots.exe') daemon start
    if ($LASTEXITCODE -ne 0) { Write-Warning 'Daemon did not start; run `kbots daemon start` after opening a new terminal.' }
}
if ($RemoteHost) {
    Write-Host "Clients are configured for remote daemon $RemoteHost. Run `kbots connect --local` to switch back."
}
Write-Host 'Done. `kb status` checks the daemon; `kb tui` opens the control center.'
