#requires -Version 5.1
# Explicit uninstall only. Preserve state, keys, jobs, bridge account and prerequisites for rollback.
[CmdletBinding(SupportsShouldProcess=$true)]
param()
$ErrorActionPreference = 'Stop'
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw 'Run Uninstall.ps1 from an elevated Windows PowerShell.' }
$dataDir = Join-Path $env:ProgramData 'SSHControlCenter'
$manifest = Join-Path $dataDir 'installation.json'
if (-not (Test-Path -LiteralPath $manifest)) { throw 'No owned installation manifest found. Nothing removed.' }
$install = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
$expected = [IO.Path]::GetFullPath((Join-Path $env:ProgramFiles 'SSH Control Center'))
if ([IO.Path]::GetFullPath($install.installDir) -ne $expected) { throw 'Installation directory does not match the owned product directory.' }
if ($PSCmdlet.ShouldProcess('SSHControlCenter','Stop and remove owned service and binaries; preserve data')) {
    $svc = Get-Service 'SSHControlCenter' -ErrorAction SilentlyContinue
    if ($svc) {
        Stop-Service 'SSHControlCenter' -Force
        & sc.exe delete SSHControlCenter
        if ($LASTEXITCODE -ne 0) { throw 'Service deletion failed.' }
    }
    $profile = Get-ItemProperty -LiteralPath ('HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\'+$install.owner) -ErrorAction SilentlyContinue
    if ($profile) {
        $link = Join-Path $profile.ProfileImagePath 'AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\SSH Control Center.lnk'
        if (Test-Path -LiteralPath $link) { Remove-Item -LiteralPath $link }
        $manual = Join-Path $profile.ProfileImagePath 'AppData\Roaming\Microsoft\Windows\Start Menu\Programs\SSH Control Center.lnk'
        if (Test-Path -LiteralPath $manual) { Remove-Item -LiteralPath $manual }
    }
    $sshdConfig = Join-Path $env:ProgramData 'ssh\sshd_config'
    if (Test-Path -LiteralPath $sshdConfig) {
        $before = [IO.File]::ReadAllText($sshdConfig)
        $pattern = '(?ms)^# BEGIN SSHCC '+[regex]::Escape($install.bridgeUser)+'\r?\n.*?^# END SSHCC '+[regex]::Escape($install.bridgeUser)+'\r?\n?'
        $after = [regex]::Replace($before,$pattern,'')
        if ($after -ne $before) {
            [IO.File]::WriteAllText($sshdConfig,$after,(New-Object Text.UTF8Encoding($false)))
            $daemonCommand = (Get-CimInstance Win32_Service -Filter "Name='sshd'").PathName
            if ($daemonCommand -match '^"([^\"]+)"') { $sshd = $Matches[1] }
            elseif ($daemonCommand -match '^(.+?\.exe)(?:\s|$)') { $sshd = $Matches[1] }
            else { [IO.File]::WriteAllText($sshdConfig,$before,(New-Object Text.UTF8Encoding($false))); throw 'Cannot resolve existing sshd executable.' }
            & $sshd -t -f $sshdConfig
            if ($LASTEXITCODE -ne 0) {
                [IO.File]::WriteAllText($sshdConfig,$before,(New-Object Text.UTF8Encoding($false)))
                throw 'sshd rejected config after removing owned block; original restored.'
            }
            Restart-Service sshd
        }
    }
    foreach ($name in @('sshcc-desktop','sshcc-window')) {
        Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $expected ($name+'.exe')) } | Stop-Process -Force
    }
    foreach ($name in @('sshcc.exe','sshcc-desktop.exe','sshcc-window.exe')) {
        $path = Join-Path $expected $name
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path }
    }
    Write-Host "Service removed. Private state and rollback backups retained at $dataDir."
    Write-Host 'Close any remaining tray process. Dedicated SSH key files/account remain inert for reinstall.'
}
