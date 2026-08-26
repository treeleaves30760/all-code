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

$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($architecture) {
    'X64' { $arch = 'x86_64' }
    'Arm64' { $arch = 'aarch64' }
    default { throw "Unsupported CPU architecture: $architecture" }
}

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
    $helperSource = Join-Path $extractDir 'claude-codex.exe'
    if (-not (Test-Path -LiteralPath $alcSource -PathType Leaf)) {
        throw 'Release archive does not contain alc.exe'
    }
    if (-not (Test-Path -LiteralPath $helperSource -PathType Leaf)) {
        throw 'Release archive does not contain claude-codex.exe'
    }

    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    Copy-Item -Force -LiteralPath $alcSource -Destination (Join-Path $installDir 'alc.exe')
    Copy-Item -Force -LiteralPath $helperSource -Destination (Join-Path $installDir 'claude-codex.exe')

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
        Write-Host 'Run: alc config'
    } elseif ($inCurrentPath) {
        Write-Host 'alc is already available on PATH. Run: alc config'
    } elseif ($inUserPath -or $inMachinePath) {
        Write-Host 'The install directory is already in your persistent PATH.'
        Write-Host 'Restart PowerShell, then run: alc config'
    } else {
        if ($pathUpdateError) {
            Write-Warning "Could not update your User PATH automatically: $pathUpdateError"
        } elseif ($env:ALC_NO_PATH_UPDATE -eq '1') {
            Write-Host 'Automatic PATH updates were disabled by ALC_NO_PATH_UPDATE=1.'
        }
        Write-Host 'alc is installed, but its directory is not on PATH.'
        Write-Host 'Add this directory to Settings > Environment Variables > User variables > Path:'
        Write-Host "  $normalizedInstallDir"
        Write-Host 'Then restart PowerShell and run: alc config'
    }
} finally {
    $resolvedTemp = [IO.Path]::GetFullPath($tempDir)
    if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemp).StartsWith('alc-install-')) {
        Remove-Item -Recurse -Force -LiteralPath $resolvedTemp -ErrorAction SilentlyContinue
    }
}
