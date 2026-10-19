#requires -Version 5.1
[CmdletBinding()]
param(
    [ValidateSet('master','target','both')][string]$Role = 'master',
    [string]$Owner = [Security.Principal.WindowsIdentity]::GetCurrent().Name,
    [ValidatePattern('^[a-zA-Z0-9_-]{1,20}$')][string]$TargetUser = 'sshcc-bridge',
    [string]$Label = $env:COMPUTERNAME,
    [string]$EnrollmentBundle,
    [string]$RegistrationOut,
    [string]$ServiceEnvironmentFile,
    [switch]$InstallTray,
    [string]$AnswersFile,
    [string]$PairingRequest,
    [string]$ReceiptOut,
    [string]$ProgressFile,
    [string]$ResultFile,
    [string]$OwnerProfile,
    [ValidateSet('preserve','service','owner-session')][string]$ExecutionMode='preserve',
    [ValidatePattern('^[a-fA-F0-9-]{36}$')][string]$SetupRunId
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$setupMutex = $null
$maintenanceActive=$false
$maintenanceProgram=''
$restartExisting=$false
$bridgeCreated=$false
function Write-SetupProgress([int]$Step,[string]$Message) {
    if ($ProgressFile) {
        [IO.File]::WriteAllText($ProgressFile,(@{step=$Step;message=$Message} | ConvertTo-Json -Compress),(New-Object Text.UTF8Encoding($false)))
    }
}
function Write-SetupResult([bool]$Ok,[string]$ErrorText='') {
    if ($ResultFile) {
        [IO.File]::WriteAllText($ResultFile,(@{ok=$Ok;error=$ErrorText;owner=if($Ok){$ownerSid}else{''};role=$Role} | ConvertTo-Json -Compress),(New-Object Text.UTF8Encoding($false)))
    }
}
trap {
    try { Write-SetupProgress 0 ('ติดตั้งไม่สำเร็จ: '+$_.Exception.Message); Write-SetupResult $false $_.Exception.Message } catch {}
    if ($restartExisting) { Start-Service 'SSHControlCenter' -ErrorAction SilentlyContinue }
    if ($maintenanceActive) { try { Invoke-AsSystem @('--data-dir',$dataDir,'configure','installer-end') $maintenanceProgram } catch {} }
    if ($bridgeCreated) { Remove-LocalUser -Name $TargetUser -ErrorAction SilentlyContinue }
    if ($setupMutex) { try { $setupMutex.ReleaseMutex() } catch {}; $setupMutex.Dispose() }
    Write-Error $_.Exception.Message -ErrorAction Continue
    exit 1
}
if ($AnswersFile) {
    $answers = @{}
    foreach ($line in [IO.File]::ReadAllLines($AnswersFile)) {
        if ($line -match '^([A-Za-z]+)=(.*)$') { $answers[$Matches[1]] = $Matches[2] }
    }
    $Role = $answers.Role
    $Owner = $answers.Owner
    $TargetUser = $answers.TargetUser
    $InstallTray = ($answers.InstallTray -eq '1')
    $EnrollmentBundle = $answers.EnrollmentBundle
    if ($Role -notin @('master','target','both') -or $TargetUser -notmatch '^[a-zA-Z0-9_-]{1,20}$') { throw 'Invalid setup answers.' }
}
if (-not [Environment]::Is64BitOperatingSystem -or -not [Environment]::Is64BitProcess) { throw 'Run the x64 installer in 64-bit Windows PowerShell.' }
$ownerSid = ([Security.Principal.NTAccount]$Owner).Translate([Security.Principal.SecurityIdentifier]).Value
$invokingOwnerSid=$ownerSid
if ($EnrollmentBundle) { $EnrollmentBundle = (Resolve-Path -LiteralPath $EnrollmentBundle).Path }
if ($ServiceEnvironmentFile) { $ServiceEnvironmentFile = (Resolve-Path -LiteralPath $ServiceEnvironmentFile).Path }
if (-not $RegistrationOut) { $RegistrationOut = Join-Path $env:USERPROFILE 'sshcc-registration.json' }
$RegistrationOut = [IO.Path]::GetFullPath($RegistrationOut)
# Capture the invoking owner before UAC, including alternate-administrator elevation.
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    $values = @{
        Role=$Role; Owner=$Owner; TargetUser=$TargetUser; Label=$Label; ExecutionMode=$ExecutionMode
        EnrollmentBundle=$EnrollmentBundle; RegistrationOut=$RegistrationOut; ServiceEnvironmentFile=$ServiceEnvironmentFile; InstallTray=[bool]$InstallTray
        PairingRequest=$PairingRequest; ReceiptOut=$ReceiptOut; ProgressFile=$ProgressFile; ResultFile=$ResultFile; OwnerProfile=$OwnerProfile; SetupRunId=$SetupRunId
    } | ConvertTo-Json -Compress
    $encodedValues = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($values))
    $script = $PSCommandPath.Replace("'","''")
    $command = '$a=ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String(''' + $encodedValues + '''))); $p=@{}; $a.PSObject.Properties | ForEach-Object {$p[$_.Name]=$_.Value}; & ''' + $script + ''' @p'
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    $child = Start-Process "$PSHOME\powershell.exe" -Verb RunAs -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand',$encoded) -Wait -PassThru -WindowStyle Hidden
    exit $child.ExitCode
}
# Advanced file-based CLI remains available, but never follows filesystem links
# supplied as public output paths. The new GUI uses protected SetupRunId staging.
function Assert-NoReparseOutput([string]$Path) {
    if (-not $Path) { return }
    $current=[IO.Path]::GetFullPath($Path)
    while ($current) {
        if (Test-Path -LiteralPath $current) {
            $item=Get-Item -LiteralPath $current -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'เส้นทางผลลัพธ์ตัวติดตั้งมี link ที่ไม่ปลอดภัย' }
        }
        $parent=[IO.Directory]::GetParent($current)
        if ($null -eq $parent) { break }
        $current=$parent.FullName
    }
}
if (-not $SetupRunId) {
    foreach ($publicPath in @($RegistrationOut,$ReceiptOut,$ProgressFile,$ResultFile)) { Assert-NoReparseOutput $publicPath }
}
$setupMutex=New-Object Threading.Mutex($false,'Global\SSHControlCenter.Setup')
try { $hasLock=$setupMutex.WaitOne(0) } catch [Threading.AbandonedMutexException] { $hasLock=$true }
if (-not $hasLock) { throw 'มีตัวติดตั้ง SSH Control Center อีกตัวกำลังทำงาน กรุณารอแล้วลองใหม่' }
if ($SetupRunId) {
    # Elevated worker writes only to system-owned directories. The launching
    # owner can read progress/public results, but cannot redirect or replace them.
    $publicRoot=Join-Path $env:ProgramData 'SSHControlCenterSetup'
    $publicStage=Join-Path $publicRoot $SetupRunId
    foreach ($path in @($publicRoot,$publicStage)) {
        if (Test-Path -LiteralPath $path) {
            $item=Get-Item -LiteralPath $path -Force
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'พื้นที่ทำงานตัวติดตั้งไม่ปลอดภัย' }
            $currentAcl=Get-Acl -LiteralPath $path
            if ($currentAcl.GetOwner([Security.Principal.SecurityIdentifier]).Value -notin @('S-1-5-18','S-1-5-32-544')) { throw 'พื้นที่ทำงานตัวติดตั้งไม่ได้เป็นของระบบ' }
            $writeRights=[Security.AccessControl.FileSystemRights]::Write -bor [Security.AccessControl.FileSystemRights]::Delete -bor [Security.AccessControl.FileSystemRights]::ChangePermissions -bor [Security.AccessControl.FileSystemRights]::TakeOwnership
            foreach ($rule in $currentAcl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
                if ($rule.AccessControlType -eq 'Allow' -and ($rule.FileSystemRights -band $writeRights) -and $rule.IdentityReference.Value -notin @('S-1-5-18','S-1-5-32-544')) { throw 'พื้นที่ทำงานตัวติดตั้งมีสิทธิ์เขียนที่ไม่ปลอดภัย' }
            }
        } else { New-Item -ItemType Directory -Path $path | Out-Null }
        $acl=New-Object Security.AccessControl.DirectorySecurity
        $acl.SetAccessRuleProtection($true,$false)
        $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
        foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
            $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)),'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
        }
        if ($path -eq $publicRoot) {
            $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-545')),'ReadAndExecute','None','None','Allow')))
        } else {
            $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($invokingOwnerSid)),'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow')))
        }
        Set-Acl -LiteralPath $path -AclObject $acl
    }
    foreach ($name in @('progress.json','result.json','registration.json','pairing-code.txt')) {
        $path=Join-Path $publicStage $name
        if (Test-Path -LiteralPath $path) {
            $item=Get-Item -LiteralPath $path -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'ไฟล์ผลลัพธ์ตัวติดตั้งเป็น link ที่ไม่ปลอดภัย' }
            Remove-Item -LiteralPath $path -Force
        }
    }
    $ProgressFile=Join-Path $publicStage 'progress.json'
    $ResultFile=Join-Path $publicStage 'result.json'
    $RegistrationOut=Join-Path $publicStage 'registration.json'
    $ReceiptOut=Join-Path $publicStage 'pairing-code.txt'
}
Write-SetupProgress 0 'กำลังตรวจชุดติดตั้งและงานที่กำลังรัน'
if ($PairingRequest) {
    $PairingRequest=(Resolve-Path -LiteralPath $PairingRequest).Path
    $EnrollmentBundle=Join-Path ([IO.Path]::GetTempPath()) ('sshcc-bundle-'+[guid]::NewGuid()+'.json')
    $inspectionOutput=@(& (Join-Path $PSScriptRoot 'sshcc.exe') pairing inspect --request $PairingRequest --bundle-output $EnrollmentBundle 2>&1)
    if ($LASTEXITCODE -ne 0) { throw ('ข้อมูลจับคู่ไม่ถูกต้อง: '+($inspectionOutput -join [Environment]::NewLine)) }
    $inspection=($inspectionOutput -join [Environment]::NewLine) | ConvertFrom-Json
    if ($inspection.bundle.os -ne 'windows') { throw 'ชุดติดตั้งนี้ไม่ใช่สำหรับ Windows' }
    $Role='target'; $Label=$inspection.envelope.targetLabel; $TargetUser=$inspection.bundle.user
}
$installDir = Join-Path $env:ProgramFiles 'SSH Control Center'
$dataDir = Join-Path $env:ProgramData 'SSHControlCenter'
$sshdConfig = Join-Path $env:ProgramData 'ssh\sshd_config'
$serviceName = 'SSHControlCenter'
$exe = Join-Path $installDir 'sshcc.exe'
function Assert-TrustedTree([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $trusted = @('S-1-5-18','S-1-5-32-544',[Security.Principal.WindowsIdentity]::GetCurrent().User.Value)
    $writeRights = [Security.AccessControl.FileSystemRights]::Write -bor [Security.AccessControl.FileSystemRights]::Delete -bor [Security.AccessControl.FileSystemRights]::DeleteSubdirectoriesAndFiles -bor [Security.AccessControl.FileSystemRights]::ChangePermissions -bor [Security.AccessControl.FileSystemRights]::TakeOwnership
    $queue = New-Object 'Collections.Generic.Queue[string]'
    $queue.Enqueue($Path)
    while ($queue.Count) {
        $item = Get-Item -LiteralPath $queue.Dequeue() -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing reparse point in existing product state: $($item.FullName)" }
        $acl = Get-Acl -LiteralPath $item.FullName
        $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
        if ($owner -notin $trusted) { throw "Refusing product state owned by an untrusted account: $($item.FullName)" }
        foreach ($rule in $acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier])) {
            if ($rule.AccessControlType -eq 'Allow' -and ($rule.FileSystemRights -band $writeRights) -and $rule.IdentityReference.Value -notin $trusted -and $rule.IdentityReference.Value -ne 'S-1-3-0') {
                throw "Refusing previously writable product state: $($item.FullName)"
            }
        }
        if ($item.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) { $queue.Enqueue($child.FullName) }
        }
    }
}
# Validate BEFORE reading/adopting config or tightening permissions; otherwise planted grants survive init.
$dataExistedAtValidation = Test-Path -LiteralPath $dataDir
$installExistedAtValidation = Test-Path -LiteralPath $installDir
Assert-TrustedTree $dataDir
Assert-TrustedTree $installDir
$previousManifest = Join-Path $dataDir 'installation.json'
if (Test-Path -LiteralPath $previousManifest) {
    $previous = Get-Content -LiteralPath $previousManifest -Raw -Encoding UTF8 | ConvertFrom-Json
    # A newly added Master grants its own key; it never transfers ownership.
    $ownerSid=$previous.owner
    $Owner=([Security.Principal.SecurityIdentifier]$ownerSid).Translate([Security.Principal.NTAccount]).Value
    if ($previous.bridgeUser -ne $TargetUser) { throw 'บัญชีเชื่อมต่อเดิมไม่ตรงกับชุดติดตั้ง กรุณาใช้บัญชีเดิมในตัวเลือกขั้นสูง' }
    $existingConfig=Get-Content -LiteralPath (Join-Path $dataDir 'config.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($existingConfig.roles -contains 'master' -and $Role -ne 'master') { $Role='both' }
    if ($existingConfig.roles -contains 'target' -and $Role -ne 'target') { $Role='both' }
    $Label=$existingConfig.label
}
if ($ExecutionMode -eq 'preserve') {
    $ExecutionMode=if(Test-Path -LiteralPath (Join-Path $dataDir 'config.json')) {
        $savedPolicy=Get-Content -LiteralPath (Join-Path $dataDir 'config.json') -Raw | ConvertFrom-Json
        if($savedPolicy.PSObject.Properties.Name -contains 'executionMode'){$savedPolicy.executionMode}else{'service'}
    }else{'owner-session'}
}
Write-SetupProgress 0 ('บัญชีสำหรับงาน: '+$Owner+' | '+$ExecutionMode+' | '+ $(if($ExecutionMode -eq 'owner-session'){'administrator; ต้อง login อยู่'}else{'SYSTEM'}))
if($ExecutionMode -eq 'owner-session' -and $ownerSid -ne $invokingOwnerSid){
    Add-Type -AssemblyName System.Windows.Forms
    $approval=[Windows.Forms.MessageBox]::Show(('บัญชี Owner เดิม: '+$Owner+[Environment]::NewLine+'Jobs, files และ terminal จะใช้บัญชีนี้พร้อมสิทธิ์ administrator ขณะ login อยู่'),'ยืนยันบัญชีสำหรับงาน','YesNo','Question')
    if($approval -ne 'Yes'){throw 'ยกเลิกการเปลี่ยนโหมด Owner session'}
}
$bridgeSid = $null
$bridgeCreated=$false
$restartExisting = $false
$backupDir = Join-Path $dataDir ('installer-backups\' + (Get-Date -Format 'yyyyMMddTHHmmssfff'))
function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program failed with exit code $LASTEXITCODE" }
}
function Invoke-AsSystem([string[]]$Arguments,[string]$Program=$exe,[switch]$CaptureOutput) {
    # Keep executable, inputs and completion marker under protected product state.
    # This preserves the IPC owner/SYSTEM boundary with alternate-admin UAC.
    $taskName='SSHCCSetup-'+[guid]::NewGuid()
    $exitFile=Join-Path $dataDir ($taskName+'.exit.json')
    $values=@{exe=$Program;arguments=$Arguments;exitFile=$exitFile;outputLimit=if($CaptureOutput){65536}else{4096}} | ConvertTo-Json -Compress
    $payload=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($values))
    $command='$v=ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('''+$payload+'''))); $code=1; $messages=@(); try { $messages=@(& $v.exe @($v.arguments) 2>&1); $code=$LASTEXITCODE } catch { $messages=@($_.Exception.Message) }; $text=($messages -join [Environment]::NewLine); if($text.Length -gt $v.outputLimit){$code=1; $text=''SYSTEM command output exceeded its limit''}; [IO.File]::WriteAllText(($v.exitFile+''.tmp''),(@{exitCode=$code;error=$text}|ConvertTo-Json -Compress)); Move-Item -LiteralPath ($v.exitFile+''.tmp'') -Destination $v.exitFile -Force; exit $code'
    $encoded=[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
    $action=New-ScheduledTaskAction -Execute (Join-Path $PSHOME 'powershell.exe') -Argument ('-NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand '+$encoded)
    $principal=New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    $settings=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
    try {
        Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Settings $settings -Force | Out-Null
        Start-ScheduledTask -TaskName $taskName
        $deadline=[DateTime]::UtcNow.AddSeconds(60)
        while (-not (Test-Path -LiteralPath $exitFile) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 200 }
        if (-not (Test-Path -LiteralPath $exitFile)) { throw 'ตรวจ service ไม่สำเร็จภายในเวลาที่กำหนด' }
        $taskResult=Get-Content -LiteralPath $exitFile -Raw | ConvertFrom-Json
        if ($taskResult.exitCode -ne 0) {
            if ($CaptureOutput) { throw ('ตรวจการตั้งค่า SSH ด้วย SYSTEM ไม่สำเร็จ: '+$taskResult.error) }
            if ($taskResult.error -match 'E_BUSY') { throw 'มีงานกำลังรันอยู่ กรุณารอให้งานเสร็จหรือยกเลิกงานก่อนติดตั้ง' }
            if ($taskResult.error -match 'E_INSTALLING') { throw 'มีตัวติดตั้งอื่นกำลังเปลี่ยน service กรุณารอแล้วลองใหม่' }
            throw 'service ยังไม่พร้อมสร้างรหัสจับคู่ กรุณาตรวจ log แล้วลองใหม่'
        }
        if ($CaptureOutput) { return [string]$taskResult.error }
    } finally {
        Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $exitFile) { Remove-Item -LiteralPath $exitFile -Force }
    }
}
function Protect-State([string]$Path) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true,$false)
    $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
    foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
        $id = New-Object Security.Principal.SecurityIdentifier($sid)
        $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
}
function Ensure-Capability([string]$Name) {
    $cap = Get-WindowsCapability -Online -Name $Name
    if ($cap.State -ne 'Installed') { Add-WindowsCapability -Online -Name $Name | Out-Null }
}
foreach ($name in @('sshcc.exe','Uninstall.ps1')) {
    if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { throw "Package is missing $name. Build a release package first." }
}
if ($InstallTray) {
    foreach ($name in @('sshcc-desktop.exe','sshcc-window.exe')) {
        if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { throw "Desktop package is missing $name." }
    }
}
$bundle = $null
if ($EnrollmentBundle) {
    if ($Role -eq 'master') { throw 'Enrollment requires target or both role.' }
    $bundle = Get-Content -LiteralPath $EnrollmentBundle -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($bundle.schemaVersion -ne 2 -or $bundle.os -ne 'windows' -or $bundle.user -ne $TargetUser) { throw 'Bundle version, OS or SSH user does not match this installation.' }
    if ($bundle.master.id -notmatch '^[a-fA-F0-9-]{36}$' -or $bundle.master.publicKey -notmatch '^ssh-ed25519 [A-Za-z0-9+/=]+(?: [^\r\n]*)?$') { throw 'Invalid enrollment master key or identity.' }
}
if ($ServiceEnvironmentFile) {
    $serviceEnv = Get-Content -LiteralPath $ServiceEnvironmentFile -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($serviceEnv -isnot [PSCustomObject]) { throw 'Service environment must be a JSON object of strings.' }
    foreach ($entry in $serviceEnv.PSObject.Properties) {
        if (-not $entry.Name -or $entry.Name.Contains('=') -or $entry.Value -isnot [string] -or $entry.Name.Contains([char]0) -or $entry.Value.Contains([char]0)) { throw 'Invalid service environment entry.' }
    }
}
if (Test-Path -LiteralPath (Join-Path $dataDir 'jobs.sqlite')) {
    $preflight=@(& (Join-Path $PSScriptRoot 'sshcc.exe') --data-dir $dataDir configure installer-preflight 2>&1)
    if ($LASTEXITCODE -ne 0) { throw ('ตรวจงานเดิมไม่สำเร็จ: '+($preflight -join [Environment]::NewLine)) }
    $jobCheck=($preflight -join [Environment]::NewLine) | ConvertFrom-Json
    if ($jobCheck.runningJobs -gt 0 -or $jobCheck.runningTerminals -gt 0) { throw 'มี jobs หรือ terminals กำลังทำงาน กรุณาปิดก่อนติดตั้ง' }
}
Write-SetupProgress 1 'กำลังตรวจและติดตั้ง prerequisites'
if ($InstallTray) {
    $installType = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').InstallationType
    if ($installType -eq 'Server Core') { throw 'Server Core uses service/CLI; omit -InstallTray.' }
    $wvRoot = Join-Path ([Environment]::GetFolderPath('ProgramFilesX86')) 'Microsoft\EdgeWebView\Application'
    $webview = @(Get-ChildItem -Path (Join-Path $wvRoot '*\msedgewebview2.exe') -ErrorAction SilentlyContinue)
    if (-not $webview.Count) {
        $bootstrap = Join-Path ([IO.Path]::GetTempPath()) ('sshcc-webview-'+[guid]::NewGuid()+'.exe')
        Invoke-WebRequest -UseBasicParsing -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrap
        $signature = Get-AuthenticodeSignature -LiteralPath $bootstrap
        if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { throw 'WebView2 bootstrap signature validation failed.' }
        $wvSetup = Start-Process -FilePath $bootstrap -ArgumentList @('/silent','/install') -Wait -PassThru -WindowStyle Hidden
        if ($wvSetup.ExitCode -notin @(0,3010)) { throw "WebView2 prerequisite failed ($($wvSetup.ExitCode))." }
    }
}
if (-not (Get-Command ssh.exe -ErrorAction SilentlyContinue)) { Ensure-Capability 'OpenSSH.Client~~~~0.0.1.0' }
if ($Role -ne 'master') {
    if (-not (Get-Service sshd -ErrorAction SilentlyContinue)) { Ensure-Capability 'OpenSSH.Server~~~~0.0.1.0' }
    $account = Get-LocalUser -Name $TargetUser -ErrorAction SilentlyContinue
    if (-not $account) {
        $randomBytes = New-Object byte[] 48
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        try { $rng.GetBytes($randomBytes) } finally { $rng.Dispose() }
        $password = ConvertTo-SecureString ([Convert]::ToBase64String($randomBytes)+'Aa1!') -AsPlainText -Force
        $account = New-LocalUser -Name $TargetUser -Password $password -AccountNeverExpires -PasswordNeverExpires -UserMayNotChangePassword -Description 'SSH Control Center forced-command bridge'
        $bridgeCreated=$true
    }
    if ($account.Description -ne 'SSH Control Center forced-command bridge') { throw 'Refusing to take over an existing account not created by SSH Control Center. Choose a new dedicated bridge name.' }
    $bridgeSid = $account.SID.Value
    if ($bridgeSid -eq $ownerSid) { throw 'Target bridge account must be separate from the owner.' }
    if (Get-LocalGroupMember -SID 'S-1-5-32-544' | Where-Object { $_.SID.Value -eq $bridgeSid }) { throw 'Target bridge account must not be an administrator.' }
    Set-Service sshd -StartupType Automatic
    Start-Service sshd
}
if (-not $installExistedAtValidation) {
    # No -Force: fail if another principal created the product path after preflight.
    New-Item -ItemType Directory -Path $installDir | Out-Null
    Protect-State $installDir
} else { Assert-TrustedTree $installDir }
if (-not $dataExistedAtValidation) {
    New-Item -ItemType Directory -Path $dataDir | Out-Null
    Protect-State $dataDir
    if (@(Get-ChildItem -LiteralPath $dataDir -Force).Count) { throw 'New private state was populated before protection; refusing adoption.' }
} else {
    Assert-TrustedTree $dataDir
    Protect-State $dataDir
}
New-Item -ItemType Directory -Path $backupDir -Force | Out-Null
Write-SetupProgress 2 'กำลังติดตั้ง service และสำรองข้อมูลเดิม'
$existing = Get-Service -Name $serviceName -ErrorAction SilentlyContinue
if ($existing) {
    $restartExisting = $existing.Status -eq 'Running'
    if ($restartExisting) {
        $maintenanceProgram=Join-Path $backupDir 'sshcc-setup.exe'
        Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'sshcc.exe') -Destination $maintenanceProgram
        Invoke-AsSystem @('--data-dir',$dataDir,'configure','installer-begin') $maintenanceProgram
        $maintenanceActive=$true
    }
    Stop-Service $serviceName -Force
}
# Close installed CLI clients before replacing the image, including shell streams.
Get-CimInstance Win32_Process -Filter "Name='sshcc.exe'" | Where-Object {
    $_.ExecutablePath -eq (Join-Path $installDir 'sshcc.exe')
} | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
# A package without desktop images (merge pairing) leaves the running window alone.
foreach ($name in @('sshcc-desktop','sshcc-window')) {
    if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot ($name+'.exe')))) { continue }
    Get-Process -Name $name -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq (Join-Path $installDir ($name+'.exe')) } | Stop-Process -Force -ErrorAction SilentlyContinue
}
foreach ($name in @('sshcc.exe','sshcc-desktop.exe','sshcc-window.exe','Uninstall.ps1','Setup.ps1','PairingBootstrap.ps1','Configure-Codex.ps1')) {
    $old = Join-Path $installDir $name
    if (Test-Path -LiteralPath $old) { Copy-Item -LiteralPath $old -Destination (Join-Path $backupDir $name) }
}
foreach ($name in @('config.json','installation.json','service-env.json','fleet.sqlite','fleet.sqlite-wal','fleet.sqlite-shm','jobs.sqlite','jobs.sqlite-wal','jobs.sqlite-shm','terminals.sqlite','terminals.sqlite-wal','terminals.sqlite-shm')) {
    if (Test-Path -LiteralPath (Join-Path $dataDir $name)) { Copy-Item -LiteralPath (Join-Path $dataDir $name) -Destination (Join-Path $backupDir $name) }
}
$templateNames=@('sshcc-client-windows-template.exe','sshcc-client-ubuntu-template.deb','template-manifest.json')
$templateBackup=Join-Path $backupDir 'templates'
New-Item -ItemType Directory -Path $templateBackup -Force | Out-Null
foreach ($name in $templateNames) {
    $old=Join-Path (Join-Path $installDir 'templates') $name
    if (Test-Path -LiteralPath $old) { Copy-Item -LiteralPath $old -Destination (Join-Path $templateBackup $name) }
}
try {
    foreach ($name in @('sshcc.exe','sshcc-desktop.exe','sshcc-window.exe','Uninstall.ps1','Setup.ps1','PairingBootstrap.ps1','Configure-Codex.ps1')) {
        $source = Join-Path $PSScriptRoot $name
        if (Test-Path -LiteralPath $source) {
            $destination=Join-Path $installDir $name
            # A reconnecting MCP client may still map the prior executable.
            # Rename its image before copying; the running process can finish.
            if($name.EndsWith('.exe') -and (Test-Path -LiteralPath $destination)) {
                Move-Item -LiteralPath $destination -Destination (Join-Path $backupDir ('retired-'+[guid]::NewGuid()+'.exe'))
            }
            Copy-Item -LiteralPath $source -Destination $destination -Force
        }
    }
    if (($Role -eq 'master' -or $Role -eq 'both') -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot 'templates'))) {
        $templates=Join-Path $installDir 'templates'
        New-Item -ItemType Directory -Path $templates -Force | Out-Null
        foreach ($asset in Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot 'templates') -File) {
            if ($asset.Name -notin @('sshcc-client-windows-template.exe','sshcc-client-ubuntu-template.deb','template-manifest.json')) { throw 'ชุดติดตั้งมี release template ที่ไม่รู้จัก' }
            Copy-Item -LiteralPath $asset.FullName -Destination (Join-Path $templates $asset.Name) -Force
        }
    }
    $initArgs = @('--data-dir',$dataDir,'init','--role',$Role,'--owner',$ownerSid,'--label',$Label,'--execution-mode',$ExecutionMode)
    if ($bridgeSid) { $initArgs += @('--allow-principal',$bridgeSid) }
    Invoke-Checked $exe $initArgs
    if($ExecutionMode -eq 'owner-session'){Invoke-AsSystem @('--data-dir',$dataDir,'configure','owner-check')}
    if ($ServiceEnvironmentFile) {
        [IO.File]::WriteAllText((Join-Path $dataDir 'service-env.json'),($serviceEnv | ConvertTo-Json -Depth 5),(New-Object Text.UTF8Encoding($false)))
    }
    if ($bundle) {
        Write-SetupProgress 3 'กำลังตั้ง SSH และเพิ่มสิทธิ์เครื่องหลักที่อนุมัติ'
        $normalizedBundle = Join-Path $dataDir 'enrollment-bundle.json'
        [IO.File]::WriteAllText($normalizedBundle,($bundle | ConvertTo-Json -Depth 20),(New-Object Text.UTF8Encoding($false)))
        Invoke-Checked $exe @('--data-dir',$dataDir,'configure','accept-bundle',$normalizedBundle)
        # Public SSH authorization file is outside the private state directory.
        $keysDir = Join-Path $env:ProgramData 'ssh\sshcc'
        if (Test-Path -LiteralPath $keysDir) { Assert-TrustedTree $keysDir }
        else { New-Item -ItemType Directory -Path $keysDir | Out-Null }
        Protect-State $keysDir
        $keysAcl = Get-Acl -LiteralPath $keysDir
        $id = New-Object Security.Principal.SecurityIdentifier($bridgeSid)
        $keysAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id,'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow')))
        Set-Acl -LiteralPath $keysDir -AclObject $keysAcl
        $keyFile = Join-Path $keysDir ($TargetUser + '.authorized_keys')
        $marker = 'sshcc:master:' + $bundle.master.id
        if (Test-Path -LiteralPath $keyFile) { Copy-Item -LiteralPath $keyFile -Destination (Join-Path $backupDir 'authorized_keys') }
        $lines = @()
        if (Test-Path -LiteralPath $keyFile) { $lines = @([IO.File]::ReadAllLines($keyFile) | Where-Object { -not $_.EndsWith(' '+$marker) }) }
        $publicParts = $bundle.master.publicKey.Split(' ')
        $sha = [Security.Cryptography.SHA256]::Create()
        try { $fingerprint = 'SHA256:' + [Convert]::ToBase64String($sha.ComputeHash([Convert]::FromBase64String($publicParts[1]))).TrimEnd('=') } finally { $sha.Dispose() }
        # Encoded PowerShell invocation behaves the same with cmd or PowerShell as sshd's DefaultShell.
        $bridgeScript = '$utf8=New-Object Text.UTF8Encoding($false); [Console]::InputEncoding=$utf8; [Console]::OutputEncoding=$utf8; $OutputEncoding=$utf8; ' + "& '" + $exe.Replace("'","''") + "' bridge --master-id " + $bundle.master.id + ' --master-key ' + $fingerprint + '; exit $LASTEXITCODE'
        $bridgeEncoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bridgeScript))
        $bridgeCommand = 'powershell.exe -NoProfile -NonInteractive -EncodedCommand ' + $bridgeEncoded
        $forced = $bridgeCommand.Replace('\','\\').Replace('"','\"')
        $publicParts = $bundle.master.publicKey.Split(' ')
        $lines += 'restrict,command="' + $forced + '" ' + $publicParts[0] + ' ' + $publicParts[1] + ' ' + $marker
        [IO.File]::WriteAllLines($keyFile,$lines,(New-Object Text.UTF8Encoding($false)))
        $keyAcl = Get-Acl -LiteralPath $keyFile
        $keyAcl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
        Set-Acl -LiteralPath $keyFile -AclObject $keyAcl
        $configText = [IO.File]::ReadAllText($sshdConfig)
        [IO.File]::WriteAllText((Join-Path $backupDir 'sshd_config'),$configText,(New-Object Text.UTF8Encoding($false)))
        $pattern = '(?ms)^# BEGIN SSHCC '+[regex]::Escape($TargetUser)+'\r?\n.*?^# END SSHCC '+[regex]::Escape($TargetUser)+'\r?\n?'
        $configText = [regex]::Replace($configText,$pattern,'')
        $keySetting = $keyFile.Replace('\','/')
        $block = @"
# BEGIN SSHCC $TargetUser
Match User $TargetUser
    AuthorizedKeysFile "$keySetting"
    PasswordAuthentication no
    AllowTcpForwarding no
    AllowAgentForwarding no
    PermitTTY no
    X11Forwarding no
Match all
# END SSHCC $TargetUser
"@
        # Match settings use the first applicable value. Place the explicit bridge rule
        # before legacy Match Group blocks; preserve every existing policy line.
        # Older Win32 OpenSSH can misclassify local groups intermittently (#1828).
        $firstMatch = [regex]::Match($configText,'(?m)^[ \t]*Match[ \t]+')
        $configured = if ($firstMatch.Success) {
            $configText.Insert($firstMatch.Index,$block+[Environment]::NewLine)
        } else {
            $configText.TrimEnd()+[Environment]::NewLine+$block+[Environment]::NewLine
        }
        [IO.File]::WriteAllText($sshdConfig,$configured,(New-Object Text.UTF8Encoding($false)))
        # Validate with the daemon actually used by the existing SSH service.
        $daemonCommand = (Get-CimInstance Win32_Service -Filter "Name='sshd'").PathName
        if ($daemonCommand -match '^"([^\"]+)"') { $sshd = $Matches[1] }
        elseif ($daemonCommand -match '^(.+?\.exe)(?:\s|$)') { $sshd = $Matches[1] }
        else { throw 'Cannot resolve the existing sshd executable.' }
        if (-not (Test-Path -LiteralPath $sshd)) { throw 'Existing sshd executable is missing.' }
        Invoke-Checked $sshd @('-t','-f',$sshdConfig)
        # Win32 OpenSSH needs SYSTEM to resolve another user's groups for Match Group.
        $effectiveText = Invoke-AsSystem @('-T','-e','-f',$sshdConfig,'-C',('user='+$TargetUser+',host=localhost,addr=127.0.0.1')) -Program $sshd -CaptureOutput
        $effective = @($effectiveText -split '\r?\n')
        $effectiveKeys = @($effective | Where-Object { $_ -match '^authorizedkeysfile ' })
        if (-not $effectiveKeys.Count -or $effectiveKeys[0].Substring(19).Trim().Trim('"') -ne $keySetting) { throw 'Existing SSH policy overrides bridge AuthorizedKeysFile; reconcile manually.' }
        Restart-Service sshd
        $hostKeys = @($effective | Where-Object { $_ -match '^hostkey ' } | ForEach-Object {
            $pub = $_.Substring(8).Trim().Trim('"').Replace('__PROGRAMDATA__',$env:ProgramData) + '.pub'
            if (Test-Path -LiteralPath $pub) { [IO.File]::ReadAllText($pub).Trim() }
        })
        if ($hostKeys.Count -eq 0) { throw 'No public SSH host keys found.' }
        $spec = @{
            targetId=$bundle.targetId; tenant=$bundle.tenant; routes=$bundle.routes; os='windows'
            user=$TargetUser; hostKeys=$hostKeys; bridgePath=$exe.Replace('\','/')
        }
        $specPath = Join-Path $dataDir 'registration-spec.json'
        [IO.File]::WriteAllText($specPath,($spec | ConvertTo-Json -Depth 20),(New-Object Text.UTF8Encoding($false)))
        Invoke-Checked $exe @('--data-dir',$dataDir,'configure','registration','--spec',$specPath,'--output',$RegistrationOut)
    }
    $binaryPath = '"' + $exe + '" --data-dir "' + $dataDir + '" service windows'
    if ($existing) {
        # CIM preserves the quoted binary path under Windows PowerShell 5.1.
        $serviceInstance=Get-CimInstance Win32_Service -Filter ("Name='"+$serviceName+"'")
        $serviceChange=Invoke-CimMethod -InputObject $serviceInstance -MethodName Change -Arguments @{PathName=$binaryPath;StartMode='Automatic';StartName='LocalSystem'}
        if ($serviceChange.ReturnValue -ne 0) { throw ('Service configuration failed: '+$serviceChange.ReturnValue) }
    } else {
        New-Service -Name $serviceName -BinaryPathName $binaryPath -DisplayName 'SSH Control Center' -StartupType Automatic | Out-Null
    }
    Invoke-Checked sc.exe @('failure',$serviceName,'reset=','86400','actions=','restart/5000/restart/15000/restart/60000')
    if ($InstallTray -and -not $SetupRunId) {
        $profile = Get-ItemProperty -LiteralPath ('HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\'+$ownerSid)
        $startup = Join-Path $profile.ProfileImagePath 'AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup'
        New-Item -ItemType Directory -Path $startup -Force | Out-Null
        $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startup 'SSH Control Center.lnk'))
        $shortcut.TargetPath = Join-Path $installDir 'sshcc-desktop.exe'
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Save()
        $startMenu = Join-Path $profile.ProfileImagePath 'AppData\Roaming\Microsoft\Windows\Start Menu\Programs'
        $manual = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startMenu 'SSH Control Center.lnk'))
        $manual.TargetPath = Join-Path $installDir 'sshcc-desktop.exe'
        $manual.Arguments = '--show'
        $manual.WorkingDirectory = $installDir
        $manual.Save()
    }
    @{
        schemaVersion=2; owner=$ownerSid; bridgeUser=$TargetUser; role=$Role; executionMode=$ExecutionMode
        installDir=$installDir; dataDir=$dataDir; backupDir=$backupDir
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dataDir 'installation.json') -Encoding UTF8
    # Future installers can validate ownership independently of the administrator used for this run.
    foreach ($rootPath in @($dataDir,$installDir)) {
        $ownedItems = @((Get-Item -LiteralPath $rootPath)) + @(Get-ChildItem -LiteralPath $rootPath -Recurse -Force)
        foreach ($item in $ownedItems) {
            $acl = Get-Acl -LiteralPath $item.FullName
            $inherit = if ($item.PSIsContainer) { 'OICI' } else { '' }
            $sddl = 'O:BAD:P(A;'+$inherit+';FA;;;SY)(A;'+$inherit+';FA;;;BA)'
            if ($rootPath -eq $installDir) { $sddl += '(A;'+$inherit+';FRFX;;;BU)' }
            $acl.SetSecurityDescriptorSddlForm($sddl,([Security.AccessControl.AccessControlSections]::Owner -bor [Security.AccessControl.AccessControlSections]::Access))
            Set-Acl -LiteralPath $item.FullName -AclObject $acl
        }
    }
    Write-SetupProgress 4 'กำลังเริ่ม service และตรวจสิทธิ์ SYSTEM'
    Start-Service $serviceName
    if ($PairingRequest) {
        $receiptDir=Join-Path $dataDir 'pairing-receipts'
        New-Item -ItemType Directory -Path $receiptDir -Force | Out-Null
        Protect-State $receiptDir
        $trustedRequest=Join-Path $receiptDir ($inspection.envelope.requestId+'.request.json')
        $trustedRegistration=Join-Path $receiptDir ($inspection.envelope.requestId+'.registration.json')
        $trustedReceipt=Join-Path $receiptDir ($inspection.envelope.requestId+'.txt')
        Copy-Item -LiteralPath $PairingRequest -Destination $trustedRequest -Force
        Copy-Item -LiteralPath $RegistrationOut -Destination $trustedRegistration -Force
        foreach ($trustedFile in @($trustedRequest,$trustedRegistration)) {
            $acl=Get-Acl -LiteralPath $trustedFile
            $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')))
            Set-Acl -LiteralPath $trustedFile -AclObject $acl
        }
        Invoke-AsSystem @('--data-dir',$dataDir,'configure','pairing-receipt','--request',$trustedRequest,'--registration',$trustedRegistration,'--output',$trustedReceipt)
        if (-not $ReceiptOut) { $ReceiptOut=Join-Path $env:USERPROFILE 'sshcc-pairing-code.txt' }
        Copy-Item -LiteralPath $trustedReceipt -Destination $ReceiptOut -Force
        # A public receipt is copyable by the launching owner; the trusted copy stays private.
        $receiptAcl=New-Object Security.AccessControl.FileSecurity
        $receiptAcl.SetAccessRuleProtection($true,$false)
        foreach ($sid in @('S-1-5-18','S-1-5-32-544',$invokingOwnerSid)) {
            $id=New-Object Security.Principal.SecurityIdentifier($sid)
            $rights=if($SetupRunId -and $sid -eq $invokingOwnerSid){'Read'}else{'FullControl'}
            $receiptAcl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($id,$rights,'Allow')))
        }
        Set-Acl -LiteralPath $ReceiptOut -AclObject $receiptAcl
    } else {
        Invoke-AsSystem @('--data-dir',$dataDir,'service','status')
    }
    $maintenanceActive=$false
    Write-SetupProgress 5 'ติดตั้งและตรวจ service สำเร็จแล้ว'
    Write-SetupResult $true
    Write-Host "Installed $serviceName ($Role). Binaries: $installDir"
    if ($bundle) { Write-Host "Import this public registration on the Master: $RegistrationOut" }
    Write-Host 'Tailscale login and existing firewall/network policy remain operator-managed.'
} catch {
    Stop-Service $serviceName -Force -ErrorAction SilentlyContinue
    foreach ($name in @('sshcc.exe','sshcc-desktop.exe','sshcc-window.exe','Uninstall.ps1','Setup.ps1','PairingBootstrap.ps1','Configure-Codex.ps1')) {
        $saved = Join-Path $backupDir $name
        if (Test-Path -LiteralPath $saved) {
            $destination=Join-Path $installDir $name
            if($name.EndsWith('.exe') -and (Test-Path -LiteralPath $destination)) {
                Move-Item -LiteralPath $destination -Destination (Join-Path $backupDir ('failed-'+[guid]::NewGuid()+'.exe'))
            }
            Copy-Item -LiteralPath $saved -Destination $destination -Force
        }
        elseif (Test-Path -LiteralPath (Join-Path $installDir $name)) { Remove-Item -LiteralPath (Join-Path $installDir $name) -Force }
    }
    foreach ($name in $templateNames) {
        $saved=Join-Path $templateBackup $name
        $destination=Join-Path (Join-Path $installDir 'templates') $name
        if (Test-Path -LiteralPath $saved) { Copy-Item -LiteralPath $saved -Destination $destination -Force }
        elseif (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination -Force }
    }
    foreach ($name in @('config.json','installation.json','service-env.json','fleet.sqlite','fleet.sqlite-wal','fleet.sqlite-shm','jobs.sqlite','jobs.sqlite-wal','jobs.sqlite-shm','terminals.sqlite','terminals.sqlite-wal','terminals.sqlite-shm')) {
        $saved = Join-Path $backupDir $name
        $destination = Join-Path $dataDir $name
        if (Test-Path -LiteralPath $saved) { Copy-Item -LiteralPath $saved -Destination $destination -Force }
        elseif (Test-Path -LiteralPath $destination) { Remove-Item -LiteralPath $destination }
    }
    $saved = Join-Path $backupDir 'authorized_keys'
    if (Test-Path -LiteralPath $saved) { Copy-Item -LiteralPath $saved -Destination (Join-Path $env:ProgramData ('ssh\sshcc\'+$TargetUser+'.authorized_keys')) -Force }
    $saved = Join-Path $backupDir 'sshd_config'
    if (Test-Path -LiteralPath $saved) { Copy-Item -LiteralPath $saved -Destination $sshdConfig -Force; Restart-Service sshd }
    if (-not $existing) {
        & sc.exe delete $serviceName 2>$null | Out-Null
        $ownedKeyFile=Join-Path $env:ProgramData ('ssh\sshcc\'+$TargetUser+'.authorized_keys')
        if (-not (Test-Path -LiteralPath (Join-Path $backupDir 'authorized_keys')) -and (Test-Path -LiteralPath $ownedKeyFile)) { Remove-Item -LiteralPath $ownedKeyFile -Force }
    }
    if ($bridgeCreated) { Remove-LocalUser -Name $TargetUser -ErrorAction SilentlyContinue }
    if ($restartExisting) { Start-Service $serviceName }
    if ($maintenanceActive) { try { Invoke-AsSystem @('--data-dir',$dataDir,'configure','installer-end') $maintenanceProgram } catch {}; $maintenanceActive=$false }
    if ($ReceiptOut -and (Test-Path -LiteralPath $ReceiptOut)) { Remove-Item -LiteralPath $ReceiptOut -Force -ErrorAction SilentlyContinue }
    Write-SetupResult $false $_.Exception.Message
    throw
} finally {
    if ($PairingRequest -and $EnrollmentBundle -and (Test-Path -LiteralPath $EnrollmentBundle)) { Remove-Item -LiteralPath $EnrollmentBundle -Force -ErrorAction SilentlyContinue }
    if ($setupMutex) { $setupMutex.ReleaseMutex(); $setupMutex.Dispose(); $setupMutex=$null }
}
