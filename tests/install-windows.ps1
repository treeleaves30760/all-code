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
    'ALC_NO_TMUX_INSTALL',
    'ALC_VERSION',
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
    $env:ALC_NO_TMUX_INSTALL = '1'

    $installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
    $installerSource = Get-Content -Raw -LiteralPath $installer
    Assert-True -Condition ($installerSource -notmatch 'RuntimeInformation\]::OSArchitecture') `
        -Message 'The installer must not depend on RuntimeInformation.OSArchitecture, which is missing from older .NET Framework versions.'
    if ($env:ALC_TEST_RELEASE_DIR) {
        # Pre-tag CI tests the just-built payload instead of GitHub's older
        # latest release. Replace exactly the downloader AST extent; all real
        # checksum/extraction/version/Rust-publication logic runs unchanged.
        $fixtureReleaseDirectory = [IO.Path]::GetFullPath($env:ALC_TEST_RELEASE_DIR)
        Assert-True -Condition (Test-Path -LiteralPath $fixtureReleaseDirectory -PathType Container) `
            -Message 'ALC_TEST_RELEASE_DIR must name a local release-fixture directory.'
        $fixtureDownloads = New-Object 'System.Collections.Generic.List[string]'
        $tokens = $null
        $parseErrors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseInput($installerSource, [ref]$tokens, [ref]$parseErrors)
        Assert-True -Condition ($parseErrors.Count -eq 0) -Message 'Could not parse the production Windows installer.'
        $downloadFunctions = @($ast.FindAll({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -ceq 'Invoke-Download'
        }, $true))
        Assert-True -Condition ($downloadFunctions.Count -eq 1) `
            -Message 'Expected exactly one production Invoke-Download function.'
        $fixtureDownloader = @'
function Invoke-Download {
    param([Parameter(Mandatory = $true)][string]$Uri, [Parameter(Mandatory = $true)][string]$OutFile)
    $leaf = [IO.Path]::GetFileName(([Uri]$Uri).AbsolutePath)
    switch -CaseSensitive ($leaf) {
        'alc-windows-x86_64.zip' { $limit = 268435456 }
        'checksums.txt' { $limit = 1048576 }
        default { throw "Unexpected installer fixture download: $Uri" }
    }
    $source = Join-Path $fixtureReleaseDirectory $leaf
    $item = Get-Item -LiteralPath $source
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $item.Length -gt $limit) {
        throw "Installer fixture is not a bounded regular file: $source"
    }
    if (Test-Path -LiteralPath $OutFile) { throw "Fixture download destination already exists: $OutFile" }
    [IO.File]::Copy($source, $OutFile, $false)
    $fixtureDownloads.Add($leaf)
}
'@
        $extent = $downloadFunctions[0].Extent
        $fixtureInstaller = $installerSource.Substring(0, $extent.StartOffset) + $fixtureDownloader + $installerSource.Substring($extent.EndOffset)
        # Fixture payload version is verified by its own --version probe; ignore
        # an inherited release selector, then restore it in the common finally.
        [Environment]::SetEnvironmentVariable('ALC_VERSION', $null, 'Process')
        $output = (& ([scriptblock]::Create($fixtureInstaller)) *>&1 | Out-String)
        Assert-True -Condition ($fixtureDownloads.Count -eq 2 -and
            $fixtureDownloads[0] -ceq 'alc-windows-x86_64.zip' -and $fixtureDownloads[1] -ceq 'checksums.txt') `
            -Message 'The installer did not fetch exactly the archive/checksum fixture pair.'
    } else {
        $output = (& $installer *>&1 | Out-String)
    }

    Assert-True -Condition ($output -match 'Downloading alc-windows-x86_64\.zip') `
        -Message "The installer did not select the x64 Windows archive. Output:`n$output"
    Assert-True -Condition ($output -match 'Automatic PATH updates were disabled') `
        -Message "The installer did not print the expected PATH guidance. Output:`n$output"

    $alc = Join-Path $testDir 'alc.exe'
    $helper = Join-Path $testDir 'claude-codex.exe'
    Assert-True -Condition (Test-Path -LiteralPath $alc -PathType Leaf) `
        -Message 'The installer did not install alc.exe.'
    # The bridge is linked into alc from 1.4.0 on: a second binary here would
    # be a stale copy answering for anyone who still calls it directly.
    Assert-True -Condition (-not (Test-Path -LiteralPath $helper)) `
        -Message 'The installer left a claude-codex.exe behind.'

    $versionOutput = (& $alc --version 2>&1 | Out-String).Trim()
    Assert-True -Condition ($LASTEXITCODE -eq 0) `
        -Message "Installed alc.exe exited with code $LASTEXITCODE."
    Assert-True -Condition ($versionOutput -match '^alc \d+\.\d+\.\d+') `
        -Message "Installed alc.exe returned an unexpected version: $versionOutput"

    $activePath = Join-Path $testDir '.alc\active.json'
    $frontPath = Join-Path $testDir '.alc\front.json'
    Assert-True -Condition (Test-Path -LiteralPath $activePath -PathType Leaf) `
        -Message 'The verified installer did not publish the immutable runtime active manifest.'
    Assert-True -Condition (Test-Path -LiteralPath $frontPath -PathType Leaf) `
        -Message 'The installer did not publish a stable front manifest.'
    $active = Get-Content -Raw -LiteralPath $activePath | ConvertFrom-Json
    Assert-True -Condition ($active.schema -eq 1 -and $active.current.digest -cmatch '^[0-9a-f]{64}$') `
        -Message 'The active runtime manifest has an invalid schema or digest.'
    $generation = Join-Path $testDir ('.alc\generations\' + $active.current.digest + '\alc.exe')
    Assert-True -Condition (Test-Path -LiteralPath $generation -PathType Leaf) `
        -Message 'The active immutable generation payload is missing.'
    $generationDigest = (Get-FileHash -Algorithm SHA256 -LiteralPath $generation).Hash.ToLowerInvariant()
    Assert-True -Condition ($generationDigest -ceq $active.current.digest) `
        -Message 'The active generation digest does not match its executable.'
    Assert-True -Condition ($installerSource -notmatch 'Copy-Item -Force -LiteralPath \$alcSource' -and $installerSource -match '__install --install-to') `
        -Message 'The installer must use the Rust publication transaction, not overwrite alc.exe directly.'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $testDir '.alc\pending.json'))) `
        -Message 'A successful fresh Windows install incorrectly left a pending bootstrap.'

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
