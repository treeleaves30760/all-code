param(
    [Parameter(Mandatory = $true)][string]$Target,
    [Parameter(Mandatory = $true)][ValidateSet('linux', 'darwin', 'windows')][string]$Os,
    [Parameter(Mandatory = $true)][string]$ArchiveName
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempDir = Join-Path $tempRoot ("alc-package-" + [guid]::NewGuid().ToString('N'))
$stage = Join-Path $tempDir 'stage'
$dist = Join-Path $workspace 'dist'

New-Item -ItemType Directory -Path $stage -Force | Out-Null
New-Item -ItemType Directory -Path $dist -Force | Out-Null

try {
    $binaryExtension = if ($Os -eq 'windows') { '.exe' } else { '' }
    $alcBinary = Join-Path $workspace "target/$Target/release/alc$binaryExtension"
    if (-not (Test-Path -LiteralPath $alcBinary -PathType Leaf)) {
        throw "Missing built alc binary: $alcBinary"
    }

    # One binary. The Codex bridge is a Cargo dependency compiled into alc,
    # so there is no second artefact to fetch, verify, or keep in step.
    Copy-Item -LiteralPath $alcBinary -Destination (Join-Path $stage "alc$binaryExtension")
    Copy-Item -LiteralPath (Join-Path $workspace 'LICENSE') -Destination $stage
    Copy-Item -LiteralPath (Join-Path $workspace 'THIRD_PARTY.md') -Destination $stage
    # The whole directory, not one named file: a license added for a new
    # bundled dependency has to reach the archive without anyone remembering
    # to edit this script.
    Copy-Item -LiteralPath (Join-Path $workspace 'THIRD_PARTY_LICENSES') `
        -Destination $stage -Recurse

    if ($Os -ne 'windows') {
        & chmod 0755 (Join-Path $stage 'alc')
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
