$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][bool]$Condition,
        [Parameter(Mandatory = $true)][string]$Message
    )
    if (-not $Condition) {
        throw $Message
    }
}

$environmentNames = @(
    'ALC_INSTALL_DIR',
    'ALC_NO_PATH_UPDATE',
    'PROCESSOR_ARCHITECTURE',
    'PROCESSOR_ARCHITEW6432'
)
$originalEnvironment = @{}
foreach ($name in $environmentNames) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testDir = Join-Path $tempRoot ("alc-installer-test-" + [guid]::NewGuid().ToString('N'))

try {
    # Simulate 32-bit Windows PowerShell on 64-bit Windows. The installer must
    # use PROCESSOR_ARCHITEW6432 and select the x64 release archive.
    $env:PROCESSOR_ARCHITECTURE = 'x86'
    $env:PROCESSOR_ARCHITEW6432 = 'AMD64'
    $env:ALC_INSTALL_DIR = $testDir
    $env:ALC_NO_PATH_UPDATE = '1'

    $installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
    $installerSource = Get-Content -Raw -LiteralPath $installer
    Assert-True -Condition ($installerSource -notmatch 'RuntimeInformation\]::OSArchitecture') `
        -Message 'The installer must not depend on RuntimeInformation.OSArchitecture, which is missing from older .NET Framework versions.'
    $output = (& $installer *>&1 | Out-String)

    Assert-True -Condition ($output -match 'Downloading alc-windows-x86_64\.zip') `
        -Message "The installer did not select the x64 Windows archive. Output:`n$output"
    Assert-True -Condition ($output -match 'Automatic PATH updates were disabled') `
        -Message "The installer did not print the expected PATH guidance. Output:`n$output"

    $alc = Join-Path $testDir 'alc.exe'
    $helper = Join-Path $testDir 'claude-codex.exe'
    Assert-True -Condition (Test-Path -LiteralPath $alc -PathType Leaf) `
        -Message 'The installer did not install alc.exe.'
    Assert-True -Condition (Test-Path -LiteralPath $helper -PathType Leaf) `
        -Message 'The installer did not install claude-codex.exe.'

    $versionOutput = (& $alc --version 2>&1 | Out-String).Trim()
    Assert-True -Condition ($LASTEXITCODE -eq 0) `
        -Message "Installed alc.exe exited with code $LASTEXITCODE."
    Assert-True -Condition ($versionOutput -match '^alc \d+\.\d+\.\d+') `
        -Message "Installed alc.exe returned an unexpected version: $versionOutput"

    Write-Host "Windows installer smoke test passed on PowerShell $($PSVersionTable.PSVersion): $versionOutput"
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], 'Process')
    }

    $resolvedTestDir = [IO.Path]::GetFullPath($testDir)
    if ($resolvedTestDir.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTestDir).StartsWith('alc-installer-test-')) {
        Remove-Item -Recurse -Force -LiteralPath $resolvedTestDir -ErrorAction SilentlyContinue
    }
}
