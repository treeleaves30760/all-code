# Offline tests. Load only the installer helpers, never the release download or
# persistent PATH writes. Real fixture executables exercise discovery and exit codes.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$installer = Join-Path (Split-Path -Parent $PSScriptRoot) 'install.ps1'
$tokens = $null
$errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($installer, [ref]$tokens, [ref]$errors)
Assert-True ($errors.Count -eq 0) 'Installer does not parse.'
foreach ($name in @('Test-PathContains', 'Get-AlcTmuxStatus', 'Update-AlcSessionPath', 'Install-AlcTmux')) {
    $definition = $ast.Find({ param($node)
        $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name
    }, $true)
    Assert-True ($null -ne $definition) "Installer is missing the tmux dependency helper $name."
    . ([scriptblock]::Create($definition.Extent.Text))
}
$realUpdatePath = ${function:Update-AlcSessionPath}
# Supply registry-like values without modifying the real User/Machine PATH.
function Update-AlcSessionPath {
    & $script:realUpdatePath -UserPath $script:userPath -MachinePath $script:machinePath
}

$environmentNames = @('Path', 'ALC_NO_TMUX_INSTALL', 'ALC_NO_PATH_UPDATE', 'ALC_TMUX_FIXTURE')
$originalEnvironment = @{}
foreach ($name in $environmentNames) {
    $originalEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ('alc-tmux-test-' + [guid]::NewGuid().ToString('N'))
$passed = 0
try {
    New-Item -ItemType Directory -Path $tempDir | Out-Null
    $fixtureExe = Join-Path $tempDir 'fixture.exe'
    # Framework csc emits an executable runnable from both Windows PowerShell
    # and PowerShell 7 (Add-Type in PS7 cannot emit ConsoleApplication).
    $compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319\csc.exe'
    $fixtureSource = Join-Path $tempDir 'fixture.cs'
    [IO.File]::WriteAllText($fixtureSource, @'
using System;
using System.IO;
class InstallerFixture {
    static int Main(string[] args) {
        string root = Environment.GetEnvironmentVariable("ALC_TMUX_FIXTURE");
        string exe = System.Diagnostics.Process.GetCurrentProcess().MainModule.FileName;
        string dir = Path.GetDirectoryName(exe);
        if (Path.GetFileName(exe).Equals("tmux.exe", StringComparison.OrdinalIgnoreCase)) {
            File.AppendAllText(Path.Combine(root, "probes"), dir + Environment.NewLine);
            if (args.Length != 1 || args[0] != "-V") return 90;
            // Runtime probes supply EOF on stdin and use stdout, not stderr.
            if (!Console.IsInputRedirected) return 91;
            if (Console.ReadLine() != null) return 92;
            Console.Error.WriteLine("diagnostic (not version output)");
            Console.Write(File.ReadAllText(Path.Combine(dir, "version")));
            return Int32.Parse(File.ReadAllText(Path.Combine(dir, "exit")));
        }
        File.AppendAllText(Path.Combine(root, "calls"), String.Join(" ", args) + Environment.NewLine);
        if (File.Exists(Path.Combine(root, "manager-stderr"))) Console.Error.WriteLine("package manager diagnostic");
        string destination = File.ReadAllText(Path.Combine(root, "destination"));
        if (destination.Length > 0) {
            Directory.CreateDirectory(destination);
            File.Copy(Path.Combine(root, "source.exe"), Path.Combine(destination, "tmux.exe"), true);
            File.Copy(Path.Combine(root, "after-version"), Path.Combine(destination, "version"), true);
            File.WriteAllText(Path.Combine(destination, "exit"), "0");
        }
        return Int32.Parse(File.ReadAllText(Path.Combine(root, "manager-exit")));
    }
}
'@)
    & $compiler /nologo /target:exe "/out:$fixtureExe" $fixtureSource
    Assert-True ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $fixtureExe)) 'Fixture compilation failed.'

    function New-Case {
        param([string]$Name)
        $script:caseName = $Name
        $script:fixture = Join-Path $tempDir $Name
        New-Item -ItemType Directory -Path $script:fixture | Out-Null
        $env:ALC_TMUX_FIXTURE = $script:fixture
        $env:ALC_NO_TMUX_INSTALL = $null
        $env:ALC_NO_PATH_UPDATE = $null
        $env:Path = $script:fixture
        $script:userPath = ''
        $script:machinePath = ''
        $global:LASTEXITCODE = 0
        foreach ($file in @('calls', 'probes', 'destination')) {
            [IO.File]::WriteAllText((Join-Path $script:fixture $file), '')
        }
        [IO.File]::WriteAllText((Join-Path $script:fixture 'manager-exit'), '0')
        [IO.File]::WriteAllText((Join-Path $script:fixture 'after-version'), 'tmux 3.6a-win32')
        Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $script:fixture 'source.exe')
    }
    function Add-Tmux {
        param([string]$Directory, [string]$Version, [int]$ExitCode = 0)
        $dir = Join-Path $script:fixture $Directory
        New-Item -ItemType Directory -Path $dir | Out-Null
        Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $dir 'tmux.exe')
        [IO.File]::WriteAllText((Join-Path $dir 'version'), $Version)
        [IO.File]::WriteAllText((Join-Path $dir 'exit'), [string]$ExitCode)
        $env:Path += ";$dir"
        return $dir
    }
    function Add-Winget {
        param([string]$Destination = '', [int]$ExitCode = 0)
        Copy-Item -LiteralPath $fixtureExe -Destination (Join-Path $script:fixture 'winget.exe')
        [IO.File]::WriteAllText((Join-Path $script:fixture 'destination'), $Destination)
        [IO.File]::WriteAllText((Join-Path $script:fixture 'manager-exit'), [string]$ExitCode)
    }
    function Assert-Calls {
        param([string]$Expected)
        $actual = [IO.File]::ReadAllText((Join-Path $script:fixture 'calls')).Trim()
        Assert-True ($actual -eq $Expected) "$script:caseName expected calls [$Expected], got [$actual]"
    }
    function Test-Install {
        $script:output = Install-AlcTmux *>&1 | Out-String
    }
    function Pass {
        $script:passed++
        Write-Host "PASS: $script:caseName"
    }
    $installArgs = 'install --id arndawg.tmux-windows --exact --source winget --scope user --accept-package-agreements --accept-source-agreements --disable-interactivity'

    # A missing dependency phase or accepting a non-native port fails these
    # assertions about real discovery and the installer's package invocation.
    New-Case 'missing-native-port'
    $destination = Join-Path $fixture 'installed'
    Add-Winget -Destination $destination
    $userPath = $destination
    Test-Install
    Assert-Calls $installArgs
    Assert-True (Get-AlcTmuxStatus).Ready "New native port was not discovered. $output"
    Assert-True ($output -match 'tmux.*ready.*--tmux') "No verified success message. $output"
    Pass

    foreach ($version in @('tmux 3.2-win32', 'tmux 3.6a-win32', 'tmux 3.10-win32', 'tmux 4.0-win32')) {
        New-Case "compatible-$version"
        $null = Add-Tmux 'native' $version
        Add-Winget
        Test-Install
        Assert-Calls ''
        Assert-True (Get-AlcTmuxStatus).Ready "Compatible version was refused: $version"
        Pass
    }

    New-Case 'compatible-native-in-quoted-path'
    $native = Add-Tmux 'quoted native' 'tmux 3.6a-win32'
    $env:Path = "$fixture;`"$native`""
    $before = $env:Path
    Assert-True (Get-AlcTmuxStatus).Ready 'A compatible native port in quoted PATH was not found.'
    Add-Winget
    Test-Install
    Assert-Calls ''
    Assert-True ($env:Path -eq $before) 'Discovery changed the session PATH.'
    Pass

    New-Case 'old-quoted-native-before-new-plain-native'
    $old = Add-Tmux 'quoted old native' 'tmux 3.1c-win32'
    $later = Add-Tmux 'later' 'tmux 3.6a-win32'
    $env:Path = "$fixture;`"$old`";$later"
    $before = $env:Path
    Assert-True (-not (Get-AlcTmuxStatus).Ready) 'An old native port in quoted PATH was ignored.'
    Assert-True (-not ([IO.File]::ReadAllText((Join-Path $fixture 'probes')).Contains($later))) 'Search continued past the quoted native blocker.'
    Assert-True ($env:Path -eq $before) 'Discovery changed the session PATH.'
    Pass

    New-Case 'compatible-native-in-quoted-semicolon-path'
    $native = Add-Tmux 'tools;archived' 'tmux 3.6a-win32'
    $env:Path = "$fixture;`"$native`""
    $before = $env:Path
    Assert-True (Get-AlcTmuxStatus).Ready 'A native port in a quoted semicolon PATH entry was not found.'
    Add-Winget
    Test-Install
    Assert-Calls ''
    Assert-True ($env:Path -eq $before) 'Discovery changed the session PATH.'
    Pass

    New-Case 'false-quoted-prefix-must-not-be-probed'
    $prefix = Add-Tmux 'tools' 'tmux 3.1c-win32'
    $later = Add-Tmux 'later' 'tmux 3.6a-win32'
    $env:Path = "$fixture;`"$prefix;archived`";$later"
    $before = $env:Path
    $status = Get-AlcTmuxStatus
    $probed = [IO.File]::ReadAllLines((Join-Path $fixture 'probes'))
    Assert-True ($probed -notcontains $prefix) 'A false prefix of a quoted PATH entry was executed.'
    Assert-True $status.Ready 'A false prefix native blocked the later valid native.'
    Assert-True ($probed -contains $later) 'The later valid native was not probed.'
    Assert-True ($env:Path -eq $before) 'Discovery changed the session PATH.'
    Pass

    New-Case 'psmux-and-other-builds-before-native'
    $null = Add-Tmux 'psmux' "tmux 3.6-win32`nPSMUX compatibility alias"
    $null = Add-Tmux 'msys' 'tmux 3.6a'
    $native = Add-Tmux 'native' 'tmux 3.2-win32'
    Add-Winget
    Test-Install
    Assert-Calls ''
    Assert-True (Get-AlcTmuxStatus).Ready 'psmux or MSYS2 hid the native port.'
    Assert-True ([IO.File]::ReadAllText((Join-Path $fixture 'probes')).Contains($native)) 'Native port was not probed.'
    Pass

    foreach ($version in @('tmux 3.1c-win32', 'tmux unknown-win32', 'tmux 4294967296.0-win32')) {
        New-Case "blocked-$version"
        $null = Add-Tmux 'first' $version
        $later = Add-Tmux 'later' 'tmux 3.6a-win32'
        Assert-True (-not (Get-AlcTmuxStatus).Ready) "Bad first native port was ignored: $version"
        Assert-True (-not ([IO.File]::ReadAllText((Join-Path $fixture 'probes')).Contains($later))) 'Search incorrectly continued past a native blocker.'
        Pass
    }

    New-Case 'spawn-failure-before-native'
    $bad = Add-Tmux 'broken' 'tmux 3.6-win32'
    [IO.File]::WriteAllText((Join-Path $bad 'tmux.exe'), 'not an executable')
    $later = Add-Tmux 'later' 'tmux 3.6a-win32'
    Assert-True (-not (Get-AlcTmuxStatus).Ready) 'A spawn failure was ignored.'
    Assert-True (-not ([IO.File]::ReadAllText((Join-Path $fixture 'probes')).Contains($later))) 'Search continued after a spawn failure.'
    Pass

    New-Case 'first-line-determines-port'
    $null = Add-Tmux 'other' "tmux 3.6a`n-win32"
    Assert-True (-not (Get-AlcTmuxStatus).Ready) 'A non-native first line was accepted.'
    Pass

    New-Case 'runtime-version-stdout-despite-exit-code'
    $null = Add-Tmux 'native' 'tmux 3.6a-win32' 7
    Assert-True (Get-AlcTmuxStatus).Ready 'Runtime reads version stdout even with a nonzero exit code.'
    Pass

    New-Case 'functions-and-aliases-are-not-native'
    function tmux { 'tmux 9.9-win32' }
    try {
        Assert-True (-not (Get-AlcTmuxStatus).Ready) 'A shell function was accepted as native tmux.'
        Set-Alias -Name tmux -Value Write-Output
        try {
            Assert-True (-not (Get-AlcTmuxStatus).Ready) 'A shell alias was accepted as native tmux.'
        } finally { Remove-Item Alias:tmux }
    } finally { Remove-Item Function:tmux }
    Pass

    New-Case 'upgrade-existing-native'
    $old = Add-Tmux 'native' 'tmux 3.1c-win32'
    Add-Winget -Destination $old
    Test-Install
    Assert-Calls $installArgs
    Assert-True (Get-AlcTmuxStatus).Ready 'Existing old native port was not upgraded.'
    Pass

    New-Case 'old-path-still-shadows-installed-native'
    $null = Add-Tmux 'old' 'tmux 3.1-win32'
    $destination = Join-Path $fixture 'installed'
    Add-Winget -Destination $destination
    $userPath = $destination
    Test-Install
    Assert-Calls $installArgs
    Assert-True (-not (Get-AlcTmuxStatus).Ready) 'Later installation was reported usable despite an older native PATH entry.'
    Assert-True ($output -match 'PATH' -and $output -notmatch 'tmux.*ready.*--tmux') "Missing shadowing warning: $output"
    Pass

    New-Case 'opt-out'
    $env:ALC_NO_TMUX_INSTALL = '1'
    Add-Winget
    Test-Install
    Assert-Calls ''
    Assert-True ($output -match 'ALC_NO_TMUX_INSTALL=1') "Missing opt-out message. $output"
    Pass

    New-Case 'no-winget'
    Test-Install
    Assert-Calls ''
    Assert-True ($output -match '[Ww]in[Gg]et' -and $output -match 'winget install --id arndawg.tmux-windows --exact') "Missing manual fallback. $output"
    Pass

    New-Case 'failed-winget-exit-code'
    Add-Winget -ExitCode 42
    Test-Install
    Assert-Calls $installArgs
    Assert-True ($output -match '42') "External exit code was lost. $output"
    Assert-True ($output -notmatch 'tmux.*ready.*--tmux') "A failed install was reported successful. $output"
    Assert-True ($global:LASTEXITCODE -eq 0) 'Optional WinGet failure leaked into the caller exit status.'
    Pass

    New-Case 'nonzero-winget-but-usable-after-reprobe'
    $destination = Join-Path $fixture 'installed'
    Add-Winget -Destination $destination -ExitCode 42
    $userPath = $destination
    Test-Install
    Assert-True ($output -match '42') "WinGet failure was hidden. $output"
    Assert-True ($output -match 'tmux.*ready.*--tmux') "Usability should be based on re-probing. $output"
    Pass

    New-Case 'zero-winget-but-still-missing'
    Add-Winget
    Test-Install
    Assert-Calls $installArgs
    Assert-True ($output -notmatch 'tmux.*ready.*--tmux') "Unverified success. $output"
    Assert-True ($output -match 'winget install --id arndawg.tmux-windows --exact') "Missing recovery command. $output"
    Pass

    New-Case 'path-merge-preserves-session-and-deduplicates'
    $session = Join-Path $fixture 'session-only'
    $user = Join-Path $fixture 'user'
    $machine = Join-Path $fixture 'machine'
    $env:Path = "$session;$user"
    $userPath = "$($user.ToUpperInvariant())\;;$machine"
    $machinePath = "$session;$machine"
    Update-AlcSessionPath
    Assert-True ($env:Path -eq "$session;$user;$machine") "PATH was replaced or duplicated: $env:Path"
    Pass

    New-Case 'path-root-entry-preserved'
    $userPath = 'C:\'
    Update-AlcSessionPath
    Assert-True ($env:Path -eq "$fixture;C:\") "An absolute drive root became drive-relative: $env:Path"
    Pass

    New-Case 'path-opt-out'
    $env:ALC_NO_PATH_UPDATE = '1'
    $before = $env:Path
    $destination = Join-Path $fixture 'installed'
    Add-Winget -Destination $destination
    $userPath = $destination
    Test-Install
    Assert-Calls $installArgs
    Assert-True ($env:Path -eq $before) 'ALC_NO_PATH_UPDATE did not prevent session PATH refresh.'
    Assert-True ($output -match '(new terminal|[Rr]estart)' -and $output -notmatch 'tmux.*ready.*--tmux') "Missing restart guidance. $output"
    Update-AlcSessionPath
    Assert-True ($env:Path -eq $before) 'Direct PATH merge ignored opt-out.'
    Pass

    New-Case 'winget-stderr-does-not-hide-exit-code'
    Add-Winget -ExitCode 42
    [IO.File]::WriteAllText((Join-Path $fixture 'manager-stderr'), '')
    Test-Install
    Assert-True ($output -match '42') "Native stderr hid the WinGet exit code. $output"
    Pass

    New-Case 'winget-spawn-failure'
    Add-Winget
    [IO.File]::WriteAllText((Join-Path $fixture 'winget.exe'), 'not an executable')
    Test-Install
    Assert-True ($output -match '[Ww]in[Gg]et' -and $output -match 'winget install --id arndawg.tmux-windows --exact') "Missing spawn-failure guidance. $output"
    Assert-True ($output -notmatch 'tmux.*ready.*--tmux') "Spawn failure was reported successful. $output"
    Pass

    New-Case 'path-refresh-failure-is-optional'
    $normalUpdatePath = ${function:Update-AlcSessionPath}
    function Update-AlcSessionPath { throw 'PATH refresh denied' }
    try {
        Add-Winget
        Test-Install
        Assert-True ($output -match 'PATH refresh denied' -and $output -match 'winget install --id arndawg.tmux-windows --exact') "Missing refresh-failure guidance. $output"
    } finally { Set-Item Function:Update-AlcSessionPath $normalUpdatePath }
    Pass

    New-Case 'native-error-preference-does-not-lose-winget-exit'
    $PSNativeCommandUseErrorActionPreference = $true
    Add-Winget -ExitCode 42
    Test-Install
    Assert-True ($output -match '42') "Native error preference hid the WinGet exit code. $output"
    Pass

    Write-Host "Windows tmux offline tests passed on PowerShell $($PSVersionTable.PSVersion): $passed"
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $originalEnvironment[$name], 'Process')
    }
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}
