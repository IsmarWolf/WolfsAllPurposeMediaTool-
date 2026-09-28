#Requires -Version 5.1
<#
.SYNOPSIS
    Deploys WolfsMedia as a portable unit to an SSD root (PLAN §16.1).

.DESCRIPTION
    The deploy unit is a hand-copied folder, not an installer: `bundle.active`
    is false, so this script *is* the packaging step. It is idempotent and it
    never touches user data - the only things it writes are the seven boot
    directories, the executable, `App/bin/`, the `.vault` attributes and
    `App/version.txt`.

    Two facts drive the order:

    * `tauri::generate_context!` embeds `frontendDist` at COMPILE time, so the
      frontend has to be built before the binary. `cargo build` on its own
      happily produces a binary with no UI inside it, which is why this script
      runs the Vite build first instead of just `cargo build --release`.
    * That build also needs `--features custom-protocol`. Without it Tauri stays
      in dev mode and the shipped binary tries to reach the Vite dev server,
      so a double-clicked copy shows ERR_CONNECTION_REFUSED. `tauri build`
      passes the feature on its own; calling cargo directly does not.
    * The seven directories below are the §5.5 order, and a test
      (`layout::tests::deploy_script_tree_matches_layout`) parses this very
      array and fails if it ever drifts from `layout::ensure`. Do not reorder or
      rename them without touching `core/layout.rs` in the same commit.

.PARAMETER Drive
    Target SSD root, e.g. 'E:\'. Omit to auto-detect (§16.1 step 0).

.PARAMETER SkipBuild
    Do not build; deploy the executable that is already in target\release.

.PARAMETER DryRun
    Print every step that would run and touch nothing. Read-only, and safe to
    run against a real drive.

.EXAMPLE
    .\scripts\deploy-portable.ps1 -Drive 'E:\' -DryRun
    .\scripts\deploy-portable.ps1 -Drive 'E:\'
#>
[CmdletBinding()]
param(
    [string]$Drive,
    [switch]$SkipBuild,
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------- constants

# §4.4: the explicit [[bin]] name. The dev binary lives in
# target\debug\, production in <ROOT>/App/.
$ExeName = 'BackupManager.exe'

# §5.5 order, mirrored from core/layout.rs. A test enforces the match.
# Forward slashes on purpose: they are the separator of the relative-path
# contract of §5.4, so the script and the Rust accessors can be compared as
# plain strings.
$Script:Trees = @(
    'Database'
    'Thumbnails/200'
    'Thumbnails/400'
    'Media/PC'
    '.vault/items'
    '.vault/thumbs'
    'App'
)

# §4.1: where the user drops the static ffmpeg. Not part of the boot tree -
# `layout::ensure` only *verifies* it - so it is created here, by packaging.
$Script:FfmpegPath = 'App/bin/ffmpeg.exe'

$Script:RepoRoot = Split-Path -Parent $PSScriptRoot
$Script:Manifest = Join-Path $Script:RepoRoot 'src-tauri/Cargo.toml'

# ---------------------------------------------------------------- reporting

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Note {
    param([string]$Message)
    Write-Host "    $Message"
}

function Write-Warn2 {
    param([string]$Message)
    Write-Warning $Message
}

function Invoke-External {
    param([string]$File, [string[]]$Arguments, [string]$What)

    if ($DryRun) {
        Write-Note "[dry-run] $File $($Arguments -join ' ')"
        return $null
    }

    # Native tools (cargo, npm, attrib) report progress and warnings on stderr.
    # Under $ErrorActionPreference = 'Stop' PowerShell 5.1 turns those into
    # terminating errors and the script dies on the first "Compiling" line, so
    # the real exit code is the only thing we trust here.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $File @Arguments 2>&1 | ForEach-Object { Write-Host $_ }
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
    }

    if ($code -ne 0) {
        throw "$What failed: $File $($Arguments -join ' ') exited $code"
    }
    return $null
}

# ------------------------------------------------------- step 0: target drive

function Resolve-TargetRoot {
    if ($Drive) {
        # Accept 'E:', 'E:\' or 'E:\somewhere' and normalize to a rooted path.
        if (-not (Test-Path -LiteralPath $Drive)) {
            throw "Target root does not exist: $Drive (create it, or point -Drive elsewhere)"
        }
        $resolved = (Resolve-Path -LiteralPath $Drive).Path
        return $resolved
    }

    # §16.1 step 0: E:, then D:, then ask.
    foreach ($candidate in @('E:\', 'D:\')) {
        if (Test-Path -LiteralPath $candidate) {
            Write-Note "auto-detected target: $candidate"
            return $candidate
        }
    }

    $answer = Read-Host 'Target SSD root (e.g. E:\)'
    if (-not $answer) {
        throw 'No target root given.'
    }
    return $answer
}

# ------------------------------------------------------------ step 1: build

function Invoke-Build {
    Write-Step 'Building the frontend and the executable'

    if ($SkipBuild) {
        Write-Note '-SkipBuild: deploying whatever is already in target\release'
        return
    }

    # The order matters: generate_context! embeds dist/ into the binary. The
    # feature matters just as much - without `custom-protocol` Tauri keeps dev
    # mode, and the binary then tries to load http://localhost:1420 instead of
    # the assets, which fails with ERR_CONNECTION_REFUSED the moment Vite is not
    # running. `tauri build` passes it implicitly; calling cargo directly means
    # we have to.
    Invoke-External -File 'npm.cmd' -Arguments @('run', 'build') -What 'frontend build'
    Invoke-External -File 'cargo' -Arguments @('build', '--release', '--features', 'custom-protocol', '--manifest-path', $Script:Manifest) -What 'cargo build'
}

function Get-BuiltExe {
    $release = Join-Path $Script:RepoRoot "src-tauri/target/release/$ExeName"
    if (-not (Test-Path -LiteralPath $release)) {
        # A dry run reports this instead of throwing: "no release build yet" is
        # a finding to print, not a failure of the script.
        if ($DryRun) {
            Write-Warn2 "No release build at $release - a real run would stop here. Build it first (or drop -SkipBuild)."
            return $null
        }
        throw "Built executable not found: $release (run without -SkipBuild, or from src-tauri\)"
    }
    return (Resolve-Path -LiteralPath $release).Path
}

# ------------------------------------------------------- step 2/3: App unit

function Install-AppUnit {
    param([string]$Root, [string]$ExePath)

    Write-Step "Installing the portable unit into $Root\App"

    $appDir = Join-Path $Root 'App'
    if (-not (Test-Path -LiteralPath $appDir)) {
        if ($DryRun) {
            Write-Note "[dry-run] create $appDir"
        } else {
            New-Item -ItemType Directory -Path $appDir -Force | Out-Null
        }
    }

    $target = Join-Path $appDir $ExeName
    if ($DryRun) {
        if ($ExePath) {
            Write-Note "[dry-run] copy $ExePath -> $target"
        } else {
            Write-Note "[dry-run] copy <no release build> -> $target"
        }
    } else {
        try {
            Copy-Item -LiteralPath $ExePath -Destination $target -Force
        } catch {
            # The classic cause: the app is still running from the target folder.
            throw "Could not copy the executable to $target . Close a running WolfsMedia first. ($($_.Exception.Message))"
        }
    }

    return $target
}

function Install-BinDir {
    param([string]$Root)

    $binDir = Join-Path $Root 'App/bin'
    if (-not (Test-Path -LiteralPath $binDir)) {
        if ($DryRun) {
            Write-Note "[dry-run] create $binDir"
        } else {
            New-Item -ItemType Directory -Path $binDir -Force | Out-Null
        }
    }
}

function Test-Ffmpeg {
    param([string]$Root)

    Write-Step 'Verifying ffmpeg (PLAN 16.2)'
    Install-BinDir -Root $Root

    $ffmpeg = Join-Path $Root $Script:FfmpegPath
    if (Test-Path -LiteralPath $ffmpeg) {
        Write-Note "found: $ffmpeg"
        return $true
    }

    # Graceful, per C4: the app boots and says so; only the HEIC/video
    # thumbnail paths and the ffmpeg transcode are unavailable.
    Write-Warn2 "ffmpeg is missing at $ffmpeg . The app still starts, but video and HEIC thumbnails will not be generated."
    Write-Note 'Drop a static win64 ffmpeg.exe there (mov/mp4/heic in, webp out). The app re-checks it on every boot.'
    return $false
}

# ------------------------------------------------------------ step 4: tree

function Initialize-Tree {
    param([string]$Root)

    Write-Step 'Ensuring the portable tree (PLAN 5.5)'

    $created = 0
    foreach ($relative in $Script:Trees) {
        $target = Join-Path $Root ($relative -replace '/', '\')
        if (Test-Path -LiteralPath $target) {
            Write-Note "exists: $relative"
            continue
        }
        if ($DryRun) {
            Write-Note "[dry-run] create $relative"
        } else {
            New-Item -ItemType Directory -Path $target -Force | Out-Null
            Write-Note "create: $relative"
        }
        $created++
    }

    return $created
}

# --------------------------------------------------------- step 5: .vault

function Set-VaultAttributes {
    param([string]$Root)

    Write-Step 'Marking .vault hidden + system (PLAN 5.5, step 5)'

    $vault = Join-Path $Root '.vault'
    if (-not (Test-Path -LiteralPath $vault)) {
        Write-Warn2 ".vault not found at $vault - run the tree step first."
        return $false
    }

    # Same tool as core/layout.rs, so runtime and packaging agree.
    Invoke-External -File 'attrib' -Arguments @('+h', '+s', $vault) -What 'attrib +h +s'
    return $true
}

# --------------------------------------------------------- step 6: marker

# §4.3: `Database/app.db` is what identifies the root. A freshly deployed drive
# has an empty `Database/` and no marker yet, and the boot would then fall back
# to the *exe folder* (§5.2 step 3) and build the whole tree one level too deep,
# inside `App/`. Touching the marker here makes the first boot find the root it
# was deployed to. An existing app.db is never opened, truncated or rewritten -
# that file is the user's database, and re-deploying must not touch it.
function Initialize-Marker {
    param([string]$Root)

    Write-Step 'Seeding the root marker (PLAN 4.3)'

    $marker = Join-Path $Root 'Database/app.db'
    if (Test-Path -LiteralPath $marker) {
        Write-Note "exists (left untouched): $marker"
        return $false
    }

    if ($DryRun) {
        Write-Note "[dry-run] create empty $marker"
        return $true
    }

    [System.IO.File]::Open($marker, [System.IO.FileMode]::CreateNew).Dispose()
    Write-Note "created: $marker"
    return $true
}

# --------------------------------------------------------- step 7: version
function Get-AppVersion {
    $tauriConf = Join-Path $Script:RepoRoot 'src-tauri/tauri.conf.json'
    if (Test-Path -LiteralPath $tauriConf) {
        $conf = Get-Content -LiteralPath $tauriConf -Raw | ConvertFrom-Json
        if ($conf.version) {
            return [string]$conf.version
        }
    }

    $cargoToml = Get-Content -LiteralPath $Script:Manifest -Raw
    if ($cargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }

    return '0.0.0'
}

function Write-VersionStamp {
    param([string]$Root, [string]$ExePath)

    Write-Step 'Stamping App/version.txt (PLAN 4.1)'

    $version = Get-AppVersion
    $stamp = Join-Path $Root 'App/version.txt'

    if ($DryRun) {
        Write-Note "[dry-run] write $stamp with version $version"
        return $version
    }

    # Locale-free, so it is not a translation surface: version, build time, and
    # the commit it came from (§14 - diagnostics, never user data).
    $commit = 'unknown'
    try {
        $commit = (& git -C $Script:RepoRoot rev-parse --short HEAD 2>$null).Trim()
        if (-not $commit) {
            $commit = 'unknown'
        }
    } catch {
        $commit = 'unknown'
    }

    $lines = @(
        $version
        "built: $((Get-Date).ToString('yyyy-MM-ddTHH:mm:ss'))"
        "commit: $commit"
        "exe: $ExeName"
    )
    Set-Content -LiteralPath $stamp -Value $lines -Encoding UTF8
    Write-Note "wrote $stamp (v$version, $commit)"
    return $version
}

# ---------------------------------------------------------- step 8: summary

function Write-Summary {
    param(
        [string]$Root,
        [string]$ExePath,
        [bool]$HasFfmpeg,
        [int]$CreatedDirs,
        [bool]$SeededMarker,
        [string]$Version
    )

    Write-Host ''
    Write-Step 'Summary'
    Write-Host "    root        : $Root"
    Write-Host "    executable  : $ExePath"
    Write-Host "    version     : $Version"
    Write-Host "    new folders : $CreatedDirs"
    Write-Host "    root marker : $(if ($SeededMarker) { 'created' } else { 'already present' })"
    Write-Host "    ffmpeg      : $(if ($HasFfmpeg) { 'present' } else { 'MISSING (see warning above)' })"
    Write-Host ''

    if (-not $HasFfmpeg) {
        Write-Note 'The app runs either way; the Settings > Origem screen will show the ffmpeg badge as missing.'
    }
    Write-Note 'The first boot fills Database/app.db with the schema; the empty file is already there as the root marker.'
    if ($DryRun) {
        Write-Note 'This was a dry run: nothing was written.'
    }
}

# ------------------------------------------------------------------- main

$root = Resolve-TargetRoot
Invoke-Build
$exePath = Get-BuiltExe
$installed = Install-AppUnit -Root $root -ExePath $exePath
$hasFfmpeg = Test-Ffmpeg -Root $root
$createdDirs = Initialize-Tree -Root $root
$vaultHidden = Set-VaultAttributes -Root $root
$seededMarker = Initialize-Marker -Root $root
$version = Write-VersionStamp -Root $root -ExePath $installed
Write-Summary -Root $root -ExePath $installed -HasFfmpeg $hasFfmpeg -CreatedDirs $createdDirs -SeededMarker $seededMarker -Version $version
