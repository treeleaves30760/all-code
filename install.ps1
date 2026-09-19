$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repo = 'treeleaves30760/all-code'
$installDir = if ($env:ALC_INSTALL_DIR) { $env:ALC_INSTALL_DIR } else { Join-Path $HOME '.local\bin' }
$version = if ($env:ALC_VERSION) { $env:ALC_VERSION } else { 'latest' }

function Invoke-Download {
    param([Parameter(Mandatory = $true)][string]$Uri, [Parameter(Mandatory = $true)][string]$OutFile)
    $curl = Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $curl) {
        $curl = Get-Command curl -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
    }
    if ($curl) {
        & $curl.Source -fsSL --retry 3 --retry-delay 1 $Uri -o $OutFile
        if ($LASTEXITCODE -eq 0) { return }
        Remove-Item -Force -LiteralPath $OutFile -ErrorAction SilentlyContinue
    }
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $Uri -OutFile $OutFile
            return
        } catch {
            Remove-Item -Force -LiteralPath $OutFile -ErrorAction SilentlyContinue
            if ($attempt -eq 3) { throw }
            Start-Sleep -Seconds $attempt
        }
    }
}

function Test-PathContains {
    param(
        [AllowNull()][string]$PathValue,
        [Parameter(Mandatory = $true)][string]$Directory
    )
    foreach ($entry in @($PathValue -split ';' | Where-Object { $_ })) {
        try {
            $expandedEntry = [Environment]::ExpandEnvironmentVariables($entry)
            if ([IO.Path]::GetFullPath($expandedEntry).TrimEnd('\') -ieq $Directory) {
                return $true
            }
        } catch {
            continue
        }
    }
    return $false
}

function Get-AlcWindowsArchitecture {
    # PROCESSOR_ARCHITEW6432 reports the operating-system architecture when a
    # 32-bit PowerShell process is running on 64-bit Windows. It must take
    # precedence over PROCESSOR_ARCHITECTURE, which only describes the process.
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($architecture)) {
        $architecture = $env:PROCESSOR_ARCHITECTURE
    }
    if ([string]::IsNullOrWhiteSpace($architecture)) {
        throw 'Could not detect the Windows CPU architecture from PROCESSOR_ARCHITEW6432 or PROCESSOR_ARCHITECTURE.'
    }

    switch ($architecture.Trim().ToUpperInvariant()) {
        'AMD64' { return 'x86_64' }
        'X64' { return 'x86_64' }
        'X86_64' { return 'x86_64' }
        'ARM64' { return 'aarch64' }
        'AARCH64' { return 'aarch64' }
        default {
            throw "Unsupported Windows CPU architecture: $architecture. alc supports x64 and ARM64 Windows."
        }
    }
}

function Get-AlcTmuxStatus {
    # Match alc's runtime: skip other ports, but stop at the first native port
    # even if its version is old/unparseable. Aliases and functions do not count.
    # Get-Command's PATH search skips quoted directories, unlike which_all.
    # Resolve each entry explicitly, in order, without changing the session PATH.
    # Windows split_paths treats semicolons inside quotes as part of the path,
    # and removes quotes (even when they surround only part of an entry).
    $pathEntries = @(
        $entry = ''
        $quoted = $false
        foreach ($character in ([string]$env:Path).ToCharArray()) {
            if ($character -eq '"') { $quoted = -not $quoted }
            elseif ($character -eq ';' -and -not $quoted) {
                if ($entry) { $entry }
                $entry = ''
            } else { $entry += $character }
        }
        if ($entry) { $entry }
    )
    $candidates = @(foreach ($entry in $pathEntries) {
        try {
            $name = [IO.Path]::Combine($entry, 'tmux')
        } catch { continue }
        Get-Command -Name ([Management.Automation.WildcardPattern]::Escape($name)) `
            -All -CommandType Application -ErrorAction SilentlyContinue
    })
    $reason = 'No native Windows tmux was found on PATH'
    foreach ($candidate in $candidates) {
        $process = New-Object System.Diagnostics.Process
        try {
            $process.StartInfo.FileName = $candidate.Source
            $process.StartInfo.Arguments = '-V'
            $process.StartInfo.UseShellExecute = $false
            $process.StartInfo.CreateNoWindow = $true
            $process.StartInfo.RedirectStandardInput = $true
            $process.StartInfo.RedirectStandardOutput = $true
            $process.StartInfo.RedirectStandardError = $true
            $null = $process.Start()
            $process.StandardInput.Close()
            # Drain stderr concurrently so a diagnostic cannot fill the pipe.
            $stderr = $process.StandardError.ReadToEndAsync()
            $text = $process.StandardOutput.ReadToEnd()
            $process.WaitForExit()
            $null = $stderr.GetAwaiter().GetResult()
        } catch {
            return [pscustomobject]@{ Ready = $false; Reason = "Could not run $($candidate.Source) -V: $($_.Exception.Message)" }
        } finally {
            $process.Dispose()
        }

        # Runtime reads stdout even when -V exits nonzero. psmux identifies
        # itself on its second line; -win32 must occur on the first line.
        $firstLine = ($text -split "`n")[0].Trim()
        if ($text.ToLowerInvariant().Contains('psmux') -or $firstLine -notmatch '-win32') {
            $reason = 'Only psmux or non-native tmux builds were found on PATH'
            continue
        }
        $match = [regex]::Match($firstLine, '^[^0-9]*([0-9]+)\.([0-9]+)')
        [uint32]$major = 0
        [uint32]$minor = 0
        if (-not $match.Success -or
            -not [uint32]::TryParse($match.Groups[1].Value, [ref]$major) -or
            -not [uint32]::TryParse($match.Groups[2].Value, [ref]$minor)) {
            return [pscustomobject]@{ Ready = $false; Reason = "Cannot parse the first native tmux version on PATH ($($candidate.Source))" }
        }
        if ($major -lt 3 -or ($major -eq 3 -and $minor -lt 2)) {
            return [pscustomobject]@{ Ready = $false; Reason = "The first native tmux on PATH ($($candidate.Source)) is older than 3.2" }
        }
        return [pscustomobject]@{ Ready = $true; Reason = '' }
    }
    return [pscustomobject]@{ Ready = $false; Reason = $reason }
}

function Update-AlcSessionPath {
    param(
        [AllowNull()][string]$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User'),
        [AllowNull()][string]$MachinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    )
    if ($env:ALC_NO_PATH_UPDATE -eq '1') { return }
    # Append new package-manager entries, preserving session-only paths and
    # their order. Never replace the current PATH with registry values.
    foreach ($entry in @(($UserPath + ';' + $MachinePath) -split ';' | Where-Object { $_ })) {
        try {
            $directory = [IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($entry))
        } catch { continue }
        if (-not (Test-PathContains -PathValue $env:Path -Directory $directory.TrimEnd('\'))) {
            if ([string]::IsNullOrEmpty($env:Path)) { $env:Path = $directory }
            else { $env:Path += ";$directory" }
        }
    }
}

function Install-AlcTmux {
    if ($env:ALC_NO_TMUX_INSTALL -eq '1') {
        Write-Host 'Skipping tmux dependency setup (ALC_NO_TMUX_INSTALL=1).'
        return
    }
    $previousExitCode = Get-Variable -Name LASTEXITCODE -Scope Global -ValueOnly -ErrorAction SilentlyContinue
    try {
        $status = Get-AlcTmuxStatus
        if ($status.Ready) {
            Write-Host 'Native Windows tmux is ready for --tmux (3.2 or newer).'
            return
        }
        $winget = Get-Command winget.exe -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if (-not $winget) {
            Write-Warning "$($status.Reason); WinGet is not installed (not installed automatically)."
        } else {
            Write-Host 'Installing or upgrading optional native tmux with WinGet (accepting package/source agreements)...'
            # PowerShell 7 can turn nonzero native exits into errors. Record the
            # actual WinGet result before any PATH refresh or version probe.
            $PSNativeCommandUseErrorActionPreference = $false
            try {
                $savedErrorActionPreference = $ErrorActionPreference
                try {
                    # PS5.1 treats redirected native stderr as an error record.
                    $ErrorActionPreference = 'Continue'
                    & $winget.Source install --id arndawg.tmux-windows --exact --source winget --scope user --accept-package-agreements --accept-source-agreements --disable-interactivity
                    $wingetExitCode = $LASTEXITCODE
                } finally {
                    $ErrorActionPreference = $savedErrorActionPreference
                }
                if ($null -eq $wingetExitCode) {
                    Write-Warning 'WinGet did not complete.'
                } elseif ($wingetExitCode -ne 0) {
                    Write-Warning "WinGet exited with code $wingetExitCode; installation/upgrade may have failed or this architecture may be unsupported."
                }
            } catch {
                Write-Warning "Could not run WinGet: $($_.Exception.Message)"
            }
            try { Update-AlcSessionPath } catch {
                Write-Warning "Could not refresh session PATH: $($_.Exception.Message)"
            }
            if ($env:ALC_NO_PATH_UPDATE -eq '1') {
                Write-Host 'ALC_NO_PATH_UPDATE=1: session PATH was not refreshed. Restart your terminal to pick up any WinGet PATH changes.'
            }
            $status = Get-AlcTmuxStatus
            if ($status.Ready) {
                Write-Host 'Native Windows tmux is ready for --tmux (3.2 or newer).'
                return
            }
            Write-Warning "$($status.Reason) after WinGet; an older PATH entry may be hiding the installed port."
        }
    } catch {
        Write-Warning "Optional tmux setup failed: $($_.Exception.Message)"
    } finally {
        # A best-effort dependency failure must not become the installer exit
        # status in callers (including CI) that forward LASTEXITCODE.
        $global:LASTEXITCODE = $previousExitCode
    }
    Write-Host 'alc is installed; only --tmux needs native Windows tmux 3.2+. Install/upgrade manually:'
    Write-Host '  winget install --id arndawg.tmux-windows --exact'
    Write-Host 'Then restart your terminal and check PATH and tmux -V. Ordinary alc and --share work without tmux.'
}

$arch = Get-AlcWindowsArchitecture
$asset = "alc-windows-$arch.zip"
if ($version -eq 'latest') {
    $releaseUrl = "https://github.com/$repo/releases/latest/download"
} else {
    $tag = if ($version.StartsWith('v')) { $version } else { "v$version" }
    $releaseUrl = "https://github.com/$repo/releases/download/$tag"
}

$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempDir = Join-Path $tempRoot ("alc-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $archive = Join-Path $tempDir $asset
    $checksums = Join-Path $tempDir 'checksums.txt'
    Write-Host "Downloading $asset..."
    Invoke-Download -Uri "$releaseUrl/$asset" -OutFile $archive
    Invoke-Download -Uri "$releaseUrl/checksums.txt" -OutFile $checksums

    $escapedAsset = [regex]::Escape($asset)
    $checksumLine = Get-Content -LiteralPath $checksums | Where-Object {
        $_ -match "^([0-9a-fA-F]{64})\s+\*?$escapedAsset$"
    } | Select-Object -First 1
    if (-not $checksumLine) {
        throw "No checksum was published for $asset"
    }
    $expected = ([regex]::Match($checksumLine, '^[0-9a-fA-F]{64}')).Value.ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum mismatch for $asset"
    }

    $extractDir = Join-Path $tempDir 'extract'
    Expand-Archive -LiteralPath $archive -DestinationPath $extractDir
    $alcSource = Join-Path $extractDir 'alc.exe'
    if (-not (Test-Path -LiteralPath $alcSource -PathType Leaf)) {
        throw 'Release archive does not contain alc.exe'
    }

    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -Force -LiteralPath $alcSource -Destination (Join-Path $installDir 'alc.exe')

    # The Codex bridge is built into alc from 1.4.0 on. An older install left
    # a separate claude-codex.exe here; removing it keeps a stale copy from
    # answering for anyone who still calls it directly.
    Remove-Item -Force -LiteralPath (Join-Path $installDir 'claude-codex.exe') -ErrorAction SilentlyContinue

    $normalizedInstallDir = [IO.Path]::GetFullPath($installDir).TrimEnd('\')
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $inUserPath = Test-PathContains -PathValue $userPath -Directory $normalizedInstallDir
    $inMachinePath = Test-PathContains -PathValue $machinePath -Directory $normalizedInstallDir
    $inCurrentPath = Test-PathContains -PathValue $env:Path -Directory $normalizedInstallDir
    $pathUpdated = $false
    $pathUpdateError = $null
    if (-not $inUserPath -and -not $inMachinePath -and $env:ALC_NO_PATH_UPDATE -ne '1') {
        try {
            $entries = @($userPath -split ';' | Where-Object { $_ })
            $newUserPath = (@($entries) + $normalizedInstallDir) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $newUserPath, 'User')
            $inUserPath = $true
            $pathUpdated = $true
        } catch {
            $pathUpdateError = $_.Exception.Message
        }
    }
    if (($inUserPath -or $inMachinePath) -and -not $inCurrentPath -and $env:ALC_NO_PATH_UPDATE -ne '1') {
        $env:Path = "$normalizedInstallDir;$env:Path"
        $inCurrentPath = $true
    }

    Write-Host "`nInstalled alc to $(Join-Path $installDir 'alc.exe')"
    if ($pathUpdated) {
        Write-Host 'Added the install directory to your User PATH.'
        Write-Host 'alc is ready in this PowerShell. New terminals will pick it up automatically.'
        Write-Host 'Next: codex login, then: alc --codex claude'
        Write-Host 'Another provider instead: alc config'
    } elseif ($inCurrentPath) {
        Write-Host 'alc is already available on PATH.'
        Write-Host 'Next: codex login, then: alc --codex claude'
        Write-Host 'Another provider instead: alc config'
    } elseif ($inUserPath -or $inMachinePath) {
        Write-Host 'The install directory is already in your persistent PATH.'
        Write-Host 'Restart PowerShell, then: codex login, then: alc --codex claude'
    } else {
        if ($pathUpdateError) {
            Write-Warning "Could not update your User PATH automatically: $pathUpdateError"
        } elseif ($env:ALC_NO_PATH_UPDATE -eq '1') {
            Write-Host 'Automatic PATH updates were disabled by ALC_NO_PATH_UPDATE=1.'
        }
        Write-Host 'alc is installed, but its directory is not on PATH.'
        Write-Host 'Add this directory to Settings > Environment Variables > User variables > Path:'
        Write-Host "  $normalizedInstallDir"
        Write-Host 'Then restart PowerShell, run codex login, and: alc --codex claude'
    }

    Install-AlcTmux
} finally {
    $resolvedTemp = [IO.Path]::GetFullPath($tempDir)
    if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemp).StartsWith('alc-install-')) {
        Remove-Item -Recurse -Force -LiteralPath $resolvedTemp -ErrorAction SilentlyContinue
    }
}
