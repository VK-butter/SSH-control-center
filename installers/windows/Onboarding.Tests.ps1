#requires -Version 5.1
# Isolated tests: no approved install, registered tasks or changed services.
[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$CliPath,[string]$NativeTemplate)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$utf8=New-Object Text.UTF8Encoding($false,$true)
$qa=Join-Path ([IO.Path]::GetTempPath()) ('sshcc-onboarding-tests-'+[guid]::NewGuid())
New-Item -ItemType Directory -Path $qa | Out-Null
$checks=New-Object Collections.Generic.List[string]
function Assert([bool]$Condition,[string]$Name){if(-not $Condition){throw "FAIL: $Name"};$checks.Add($Name)}
function Sha([byte[]]$Bytes){$sha=[Security.Cryptography.SHA256]::Create();try{return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-','').ToLowerInvariant()}finally{$sha.Dispose()}}
function Invoke-Child([string]$Script,[string[]]$Arguments,[string]$InputText=''){
    $processInfo=New-Object Diagnostics.ProcessStartInfo
    $processInfo.FileName=Join-Path $PSHOME 'powershell.exe'
    $processInfo.Arguments='-Sta -NoProfile -ExecutionPolicy Bypass -File "'+$Script+'" '+(($Arguments | ForEach-Object {'"'+$_+'"'} ) -join ' ')
    $processInfo.UseShellExecute=$false;$processInfo.CreateNoWindow=$true
    $processInfo.RedirectStandardInput=$true;$processInfo.RedirectStandardOutput=$true;$processInfo.RedirectStandardError=$true
    $processInfo.StandardOutputEncoding=$utf8;$processInfo.StandardErrorEncoding=$utf8
    $child=New-Object Diagnostics.Process;$child.StartInfo=$processInfo
    [void]$child.Start()
    $child.StandardInput.Write($InputText);$child.StandardInput.Close()
    $stdout=$child.StandardOutput.ReadToEndAsync();$stderr=$child.StandardError.ReadToEndAsync()
    if(-not $child.WaitForExit(30000)){ $child.Kill();throw 'Inspector timed out' }
    return @{exit=$child.ExitCode;output=$stdout.Result;error=$stderr.Result}
}
Copy-Item -LiteralPath $CliPath -Destination (Join-Path $qa 'sshcc.exe')
foreach($name in @('PairingBootstrap.ps1','Configure-Codex.ps1')){
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot $name) -Destination $qa
}
$generic=Join-Path $qa 'generic.exe'
[IO.File]::WriteAllBytes($generic,[Text.Encoding]::ASCII.GetBytes('fake-executable-template-without-trailer'))
$genericResult=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$generic,'-PackageDirectory',$qa,'-InspectOnly')
Assert ($genericResult.exit -eq 0 -and ($genericResult.output | ConvertFrom-Json).paired -eq $false) 'Generic template needs no service'
$missing=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$generic,'-PackageDirectory',$qa,'-InspectOnly','-RequirePairing')
Assert ($missing.exit -ne 0) 'Client template rejects missing pairing metadata'
$bundle=[ordered]@{
    master=[ordered]@{id='11111111-1111-4111-8111-111111111111';label='เครื่องหลัก ทดสอบ';publicKey='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH'}
    os='windows';routes=@([ordered]@{address='127.0.0.1';kind='lan';port=22})
    schemaVersion=2;targetId='onboarding-test';tenant='qa';user='sshcc-bridge'
}
$bundleJson=$bundle | ConvertTo-Json -Depth 20 -Compress
$envelope=@{schemaVersion=1;requestId='22222222-2222-4222-8222-222222222222';masterId=$bundle.master.id;bundleDigest=(Sha ($utf8.GetBytes($bundleJson)));targetLabel='เครื่องใหม่ ภาษาไทย';bundle=$bundle}
$payload=$utf8.GetBytes(($envelope | ConvertTo-Json -Depth 20 -Compress))
$paired=Join-Path $qa 'paired.exe'
$stream=[IO.File]::Create($paired)
try{
    $template=[IO.File]::ReadAllBytes($generic);$stream.Write($template,0,$template.Length)
    $stream.Write($payload,0,$payload.Length)
    $length=[BitConverter]::GetBytes([uint64]$payload.Length);$stream.Write($length,0,8)
    $magic=[Text.Encoding]::ASCII.GetBytes('SSHCCPAIRING0001');$stream.Write($magic,0,16)
}finally{$stream.Dispose()}
$inspect=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$paired,'-PackageDirectory',$qa,'-InspectOnly')
Assert ($inspect.exit -eq 0) ('Valid personalized trailer: '+$inspect.error)
$parsed=$inspect.output | ConvertFrom-Json
Assert ($parsed.paired -and $parsed.inspection.envelope.targetLabel -ceq 'เครื่องใหม่ ภาษาไทย') 'UTF-8 Thai label survives trailer'
$wrongMagic=Join-Path $qa 'wrong-magic.exe';Copy-Item -LiteralPath $paired -Destination $wrongMagic
$stream=[IO.File]::OpenWrite($wrongMagic)
try{$stream.Seek(-1,[IO.SeekOrigin]::End)|Out-Null;$stream.WriteByte(88)}finally{$stream.Dispose()}
$invalid=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$wrongMagic,'-PackageDirectory',$qa,'-InspectOnly','-RequirePairing')
Assert ($invalid.exit -ne 0) 'Client template rejects corrupted magic instead of becoming Master'
$truncated=Join-Path $qa 'truncated.exe'
[IO.File]::WriteAllBytes($truncated,[IO.File]::ReadAllBytes($paired)[0..20])
$invalid=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$truncated,'-PackageDirectory',$qa,'-InspectOnly','-RequirePairing')
Assert ($invalid.exit -ne 0) 'Client template rejects truncated trailer'
$before=Get-CimInstance Win32_Service -Filter "Name='SSHControlCenter'" | Select-Object State,PathName,StartMode
$denied=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$paired,'-PackageDirectory',$qa,'-Console') ([Environment]::NewLine)
$after=Get-CimInstance Win32_Service -Filter "Name='SSHControlCenter'" | Select-Object State,PathName,StartMode
Assert ($denied.exit -eq 0 -and (($before | ConvertTo-Json -Compress) -ceq ($after | ConvertTo-Json -Compress))) 'Deny before Approve preserves real service'
$bad=Join-Path $qa 'oversize.exe';Copy-Item -LiteralPath $paired -Destination $bad
$stream=[IO.File]::OpenWrite($bad)
try{$stream.Seek(-24,[IO.SeekOrigin]::End)|Out-Null;$length=[BitConverter]::GetBytes([uint64]65537);$stream.Write($length,0,8)}finally{$stream.Dispose()}
$rejected=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$bad,'-PackageDirectory',$qa,'-InspectOnly')
Assert ($rejected.exit -ne 0) 'Over 64 KiB envelope rejected'
$envelope.bundle.routes[0].address='127.0.0.2'
$changedPayload=$utf8.GetBytes(($envelope | ConvertTo-Json -Depth 20 -Compress))
$stream=[IO.File]::Create($bad)
try{
    $template=[IO.File]::ReadAllBytes($generic);$stream.Write($template,0,$template.Length)
    $stream.Write($changedPayload,0,$changedPayload.Length)
    $length=[BitConverter]::GetBytes([uint64]$changedPayload.Length);$stream.Write($length,0,8)
    $stream.Write($magic,0,16)
}finally{$stream.Dispose()}
$rejected=Invoke-Child (Join-Path $qa 'PairingBootstrap.ps1') @('-InstallerPath',$bad,'-PackageDirectory',$qa,'-InspectOnly')
Assert ($rejected.exit -ne 0) 'Tampered bundle rejected before approval'
$config=Join-Path $qa 'config.toml'
$original=@'
model = "preserve"
[mcp_servers.other]
command = 'C:\other.exe'
[mcp_servers.ssh_control_center]
command = 'C:\old.exe'
args = [
  "old", # old multiline arguments
]
custom = "keep"
[mcp_servers.ssh_control_center.env]
PUBLIC_MODE = "keep"
'@
[IO.File]::WriteAllText($config,$original,$utf8)
$configured=Invoke-Child (Join-Path $qa 'Configure-Codex.ps1') @('-ConfigPath',$config,'-CliPath',$CliPath)
Assert ($configured.exit -eq 0) 'Automatic Codex configuration succeeds'
$first=[IO.File]::ReadAllText($config)
$configured=Invoke-Child (Join-Path $qa 'Configure-Codex.ps1') @('-ConfigPath',$config,'-CliPath',$CliPath)
$second=[IO.File]::ReadAllText($config)
Assert ($configured.exit -eq 0 -and $first -ceq $second) 'Repeated Codex setup is byte-idempotent'
Assert ($second.Contains('[mcp_servers.other]') -and $second.Contains('custom = "keep"') -and $second.Contains('PUBLIC_MODE = "keep"')) 'Other MCP, custom keys and env preserved'
Assert (@(Get-ChildItem -LiteralPath $qa -Filter '*.sshcc-backup-*').Count -eq 1) 'Codex backup retained exactly once'
if($NativeTemplate){
    $native=Join-Path $qa 'native-paired.exe'
    Copy-Item -LiteralPath $NativeTemplate -Destination $native
    $stream=[IO.File]::Open($native,[IO.FileMode]::Append)
    try{$stream.Write($payload,0,$payload.Length);$length=[BitConverter]::GetBytes([uint64]$payload.Length);$stream.Write($length,0,8);$stream.Write($magic,0,16)}finally{$stream.Dispose()}
    $child=Start-Process -FilePath $native -ArgumentList '/INSPECT' -WindowStyle Hidden -PassThru
    if(-not $child.WaitForExit(30000)){Stop-Process -Id $child.Id;throw 'Native inspector timed out'}
    Assert ($child.ExitCode -eq 0) 'Native EXE appended envelope inspection succeeds'
}
@{passed=$checks.Count;checks=$checks.ToArray();testDirectory=$qa} | ConvertTo-Json -Depth 5
