param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][ValidateSet('linux', 'darwin', 'windows')][string]$Os,
    [Parameter(Mandatory = $true)][ValidateSet('amd64', 'arm64')][string]$HelperArch,
    [Parameter(Mandatory = $true)][string]$ArchiveName
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$helperVersion = 'v0.3.1'
$helperRepo = 'fcakyon/claude-code-with-codex'
$extension = if ($Os -eq 'windows') { 'zip' } else { 'tar.gz' }
$helperAsset = "claude-codex-$Os-$HelperArch.$extension"
$helperChecksumAsset = "claude-codex-$Os-$HelperArch.sha256"
$helperBaseUrl = "https://github.com/$helperRepo/releases/download/$helperVersion"
$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempDir = Join-Path $tempRoot ("alc-package-" + [guid]::NewGuid().ToString('N'))
$stage = Join-Path $tempDir 'stage'
$helperExtract = Join-Path $tempDir 'helper'
$dist = Join-Path $workspace 'dist'

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

New-Item -ItemType Directory -Path $stage, $helperExtract -Force | Out-Null
New-Item -ItemType Directory -Path $dist -Force | Out-Null

try {
    $helperArchive = Join-Path $tempDir $helperAsset
    $helperChecksum = Join-Path $tempDir $helperChecksumAsset
    Invoke-Download -Uri "$helperBaseUrl/$helperAsset" -OutFile $helperArchive
    Invoke-Download -Uri "$helperBaseUrl/$helperChecksumAsset" -OutFile $helperChecksum

    $checksumText = (Get-Content -Raw -LiteralPath $helperChecksum).Trim()
    $expected = ([regex]::Match($checksumText, '[0-9a-fA-F]{64}')).Value.ToLowerInvariant()
    if (-not $expected) {
        throw "Could not parse the checksum for $helperAsset"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $helperArchive).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw "Checksum mismatch for $helperAsset"
    }

    if ($extension -eq 'zip') {
        Expand-Archive -LiteralPath $helperArchive -DestinationPath $helperExtract
    } else {
        & tar -xzf $helperArchive -C $helperExtract
        if ($LASTEXITCODE -ne 0) { throw "Failed to extract $helperAsset" }
    }

    $binaryExtension = if ($Os -eq 'windows') { '.exe' } else { '' }
    $alcBinary = Join-Path $workspace "target/$Target/release/alc$binaryExtension"
    $helperBinary = Join-Path $helperExtract "claude-codex$binaryExtension"
    if (-not (Test-Path -LiteralPath $alcBinary -PathType Leaf)) {
        throw "Missing built alc binary: $alcBinary"
    }
    if (-not (Test-Path -LiteralPath $helperBinary -PathType Leaf)) {
        throw "Missing helper binary in $helperAsset"
    }

    Copy-Item -LiteralPath $alcBinary -Destination (Join-Path $stage "alc$binaryExtension")
    Copy-Item -LiteralPath $helperBinary -Destination (Join-Path $stage "claude-codex$binaryExtension")
    Copy-Item -LiteralPath (Join-Path $workspace 'LICENSE') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $workspace 'THIRD_PARTY.md') -Destination $stage
    New-Item -ItemType Directory -Path (Join-Path $stage 'THIRD_PARTY_LICENSES') | Out-Null
    Copy-Item -LiteralPath (Join-Path $workspace 'THIRD_PARTY_LICENSES/claude-codex-LICENSE') `
        -Destination (Join-Path $stage 'THIRD_PARTY_LICENSES')

    if ($Os -ne 'windows') {
        & chmod 0755 (Join-Path $stage 'alc') (Join-Path $stage 'claude-codex')
        if ($LASTEXITCODE -ne 0) { throw 'Failed to mark release binaries executable' }
    }

    $archivePath = Join-Path $dist $ArchiveName
    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -Force -LiteralPath $archivePath
    }
    if ($ArchiveName.EndsWith('.zip')) {
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $archivePath
    } else {
        Push-Location $stage
        try {
            & tar -czf $archivePath .
            if ($LASTEXITCODE -ne 0) { throw "Failed to create $ArchiveName" }
        } finally {
            Pop-Location
        }
    }
    Write-Host "Created $archivePath"
} finally {
    $resolvedTemp = [IO.Path]::GetFullPath($tempDir)
    if ($resolvedTemp.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemp).StartsWith('alc-package-')) {
        Remove-Item -Recurse -Force -LiteralPath $resolvedTemp -ErrorAction SilentlyContinue
    }
}
