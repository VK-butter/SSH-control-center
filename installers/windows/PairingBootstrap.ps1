#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$InstallerPath,
    [string]$PackageDirectory = $PSScriptRoot,
    [switch]$Console,
    [switch]$InspectOnly,
    [switch]$RequirePairing
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$utf8 = New-Object Text.UTF8Encoding($false,$true)
[Console]::OutputEncoding = $utf8
$exe = Join-Path $PackageDirectory 'sshcc.exe'
$work = Join-Path ([IO.Path]::GetTempPath()) ('sshcc-setup-'+[guid]::NewGuid())
New-Item -ItemType Directory -Path $work | Out-Null
$requestFile = Join-Path $work 'pairing-request.json'
function Read-PairingTrailer([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    try {
        if ($stream.Length -lt 24) { return $null }
        $stream.Seek(-24,[IO.SeekOrigin]::End) | Out-Null
        $footer = New-Object byte[] 24
        if ($stream.Read($footer,0,24) -ne 24) { throw 'อ่านข้อมูลชุดติดตั้งไม่สำเร็จ' }
        $magic = [Text.Encoding]::ASCII.GetString($footer,8,16)
        if ($magic -ne 'SSHCCPAIRING0001') { return $null }
        $length = [BitConverter]::ToUInt64($footer,0)
        if ($length -eq 0 -or $length -gt 65536 -or $length -gt ($stream.Length-24)) { throw 'ข้อมูลจับคู่ในชุดติดตั้งไม่ถูกต้อง กรุณาสร้างชุดติดตั้งใหม่' }
        $stream.Seek(-24-[long]$length,[IO.SeekOrigin]::End) | Out-Null
        $bytes = New-Object byte[] ([int]$length)
        $offset = 0
        while ($offset -lt $bytes.Length) {
            $count = $stream.Read($bytes,$offset,$bytes.Length-$offset)
            if ($count -eq 0) { throw 'ข้อมูลจับคู่ในชุดติดตั้งไม่ครบ' }
            $offset += $count
        }
        return $utf8.GetString($bytes)
    } finally { $stream.Dispose() }
}
$inspection = $null
try {
    $payload = Read-PairingTrailer ([IO.Path]::GetFullPath($InstallerPath))
    if ($RequirePairing -and $null -eq $payload) { throw 'ไม่พบข้อมูลจับคู่ในชุดติดตั้ง กรุณาสร้างชุดติดตั้งใหม่จากเครื่องหลัก' }
    if ($null -ne $payload) {
        [IO.File]::WriteAllText($requestFile,$payload,$utf8)
        $output = @(& $exe pairing inspect --request $requestFile 2>&1)
        if ($LASTEXITCODE -ne 0) { throw ('ชุดติดตั้งจับคู่ไม่ถูกต้อง: '+($output -join [Environment]::NewLine)) }
        $inspection = ($output -join [Environment]::NewLine) | ConvertFrom-Json
        if ($inspection.bundle.os -ne 'windows') { throw 'ชุดติดตั้งนี้ไม่ใช่สำหรับ Windows' }
    }
    if ($InspectOnly) {
        @{paired=($null -ne $inspection); inspection=$inspection} | ConvertTo-Json -Depth 30
        exit 0
    }
    # Capture the actual session owner before asking for UAC/alternate admin.
    $owner = [Security.Principal.WindowsIdentity]::GetCurrent().Name
    $ownerProfile = [Environment]::GetFolderPath('UserProfile')
    $executionMode='preserve'
    $existingInstall=Test-Path -LiteralPath (Join-Path $env:ProgramFiles 'SSH Control Center\sshcc.exe')
    $savedDir = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'SSHControlCenterSetup'
    $savedReceipt = if ($inspection) { Join-Path $savedDir ('pairing-'+$inspection.envelope.requestId+'.txt') } else { $null }
    $runId=[guid]::NewGuid().ToString()
    $publicStage=Join-Path (Join-Path $env:ProgramData 'SSHControlCenterSetup') $runId
    $receiptFile = Join-Path $publicStage 'pairing-code.txt'
    $progressFile = Join-Path $publicStage 'progress.json'
    $resultFile = Join-Path $publicStage 'result.json'
    $installType=(Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').InstallationType
    $requestedInstallTray=(-not $Console -and $installType -ne 'Server Core' -and [Environment]::UserInteractive -and (Test-Path -LiteralPath (Join-Path $PackageDirectory 'sshcc-desktop.exe')))
    $installing = $false
    $successful = $false
    $code = ''
    $statusMessage = ''
    function Start-Setup {
        $settings = @{
            Owner=$owner; OwnerProfile=$ownerProfile; Role='master'; Label=$env:COMPUTERNAME
            InstallTray=$false; SetupRunId=$runId
            ExecutionMode=$executionMode
        }
        if ($inspection) {
            $settings.Role='target'
            $settings.PairingRequest=$requestFile
            # Public output paths are chosen by the elevated worker from SetupRunId.
            $settings.Label=$inspection.envelope.targetLabel
            $settings.TargetUser=$inspection.bundle.user
        } else {
            $settings.InstallTray=$requestedInstallTray
        }
        $data = [Convert]::ToBase64String($utf8.GetBytes(($settings | ConvertTo-Json -Compress)))
        $setupPath = (Join-Path $PackageDirectory 'Setup.ps1').Replace("'","''")
        $source = '$a=ConvertFrom-Json ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('''+$data+'''))); $p=@{}; $a.PSObject.Properties | ForEach-Object {$p[$_.Name]=$_.Value}; & '''+$setupPath+''' @p'
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($source))
        return Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand',$encoded) -PassThru -WindowStyle Hidden
    }
    function Finish-Setup([Diagnostics.Process]$Child) {
        $Child.WaitForExit()
        if ($Child.ExitCode -ne 0) {
            $message = 'ติดตั้งไม่สำเร็จ กรุณาตรวจการเชื่อมต่ออินเทอร์เน็ตและสิทธิ์ administrator แล้วลองใหม่'
            if (Test-Path -LiteralPath $resultFile) {
                $result = Get-Content -LiteralPath $resultFile -Raw -Encoding UTF8 | ConvertFrom-Json
                if ($result.error) { $message += [Environment]::NewLine+$result.error }
            }
            throw $message
        }
        if ($inspection) {
            if (-not (Test-Path -LiteralPath $receiptFile)) { throw 'ติดตั้งแล้ว แต่ยังสร้างรหัสยืนยันไม่ได้ กรุณาลองใหม่' }
            $script:code = [IO.File]::ReadAllText($receiptFile).Trim()
            if (-not $script:code.StartsWith('SSHCC1.')) { throw 'รหัสยืนยันไม่ถูกต้อง' }
            New-Item -ItemType Directory -Path $savedDir -Force | Out-Null
            [IO.File]::WriteAllText($savedReceipt,$script:code,$utf8)
            $script:statusMessage = 'ติดตั้งและตรวจ service สำเร็จแล้ว นำรหัสนี้กลับไปวางที่เครื่องหลัก'
        } else {
            $configuredOwner=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
            if (Test-Path -LiteralPath $resultFile) {
                $installationResult=Get-Content -LiteralPath $resultFile -Raw -Encoding UTF8 | ConvertFrom-Json
                if ($installationResult.owner) { $configuredOwner=$installationResult.owner }
            }
            if ($configuredOwner -ne [Security.Principal.WindowsIdentity]::GetCurrent().User.Value) {
                $script:requestedInstallTray=$false
                $script:statusMessage='service เครื่องหลักพร้อมแล้ว กรุณาเปิด SSH Control Center ด้วยบัญชีเจ้าของเดิมของเครื่องนี้'
                $script:successful=$true
                return
            }
            $codexStatus = 'ไม่พบ Codex สามารถตั้งค่า MCP ภายหลังได้'
            if (Test-Path -LiteralPath (Join-Path $PackageDirectory 'Configure-Codex.ps1')) {
                try { $codexStatus = & (Join-Path $PackageDirectory 'Configure-Codex.ps1') } catch { $codexStatus='ติดตั้งสำเร็จ แต่ตั้งค่า Codex ไม่สำเร็จ: '+$_.Exception.Message }
            }
            $script:statusMessage = 'เครื่องหลักพร้อมใช้งานแล้ว'+[Environment]::NewLine+$codexStatus
            $desktop = Join-Path $env:ProgramFiles 'SSH Control Center\sshcc-desktop.exe'
            if ($requestedInstallTray -and (Test-Path -LiteralPath $desktop)) {
                # User-profile links are written by the original unelevated owner.
                foreach ($location in @('Startup','Programs')) {
                    $folder=[Environment]::GetFolderPath($location)
                    New-Item -ItemType Directory -Path $folder -Force | Out-Null
                    $shortcut=(New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $folder 'SSH Control Center.lnk'))
                    $shortcut.TargetPath=$desktop
                    $shortcut.WorkingDirectory=Split-Path -Parent $desktop
                    if ($location -eq 'Programs') { $shortcut.Arguments='--show' }
                    $shortcut.Save()
                }
            }
            if ($requestedInstallTray -and (Test-Path -LiteralPath $desktop)) { Start-Process -FilePath $desktop -ArgumentList '--show' -WindowStyle Hidden }
        }
        $script:successful=$true
    }
    # Previously completed receipts are public data; only reuse while the local
    # service still attests this exact node and its SYSTEM execution principal.
    if ($inspection -and (Test-Path -LiteralPath $savedReceipt)) {
        try {
            $savedCode = [IO.File]::ReadAllText($savedReceipt).Trim()
            $verified=@(& $exe pairing inspect-code --code-file $savedReceipt --request $requestFile 2>$null)
            if ($LASTEXITCODE -ne 0) { throw 'invalid saved receipt' }
            $receipt=($verified -join [Environment]::NewLine) | ConvertFrom-Json
            if ($receipt.requestId -ne $inspection.envelope.requestId -or $receipt.masterId -ne $inspection.envelope.masterId -or $receipt.bundleDigest -ne $inspection.envelope.bundleDigest) { throw 'different saved request' }
            $local=@(& (Join-Path $env:ProgramFiles 'SSH Control Center\sshcc.exe') service status 2>$null)
            if ($LASTEXITCODE -ne 0) { throw 'local service unavailable' }
            $attestation=($local -join [Environment]::NewLine) | ConvertFrom-Json
            if ($attestation.nodeId -ne $receipt.registration.nodeId -or $attestation.executionPrincipal -ne 'S-1-5-18') { throw 'local service identity changed' }
            $masterOutput=@(& (Join-Path $env:ProgramFiles 'SSH Control Center\sshcc.exe') request masters.list 2>$null)
            if ($LASTEXITCODE -ne 0) { throw 'local owner must approve again' }
            $masters=($masterOutput -join [Environment]::NewLine) | ConvertFrom-Json
            $active=@($masters | Where-Object {$_.id -eq $inspection.envelope.masterId -and $_.publicKey -eq $inspection.bundle.master.publicKey -and -not $_.revoked})
            if ($active.Count -ne 1) { throw 'Master key is revoked or changed' }
            $code=$savedCode; $successful=$true
            $statusMessage='เครื่องนี้ติดตั้งแล้ว คัดลอกรหัสกลับไปที่เครื่องหลักได้'
        } catch { $successful=$false }
    }
    if ($Console) {
        Write-Host 'SSH Control Center Setup'
        if($existingInstall){
            if((Read-Host 'เปลี่ยน jobs/files/terminal ไปใช้ Owner เดิมพร้อมสิทธิ์ administrator? พิมพ์ OWNER หรือ Enter เพื่อรักษาโหมดเดิม') -ceq 'OWNER'){$executionMode='owner-session'}
        }else{$executionMode='owner-session'}
        Write-Host ('บัญชี Owner: '+$owner+' | '+$executionMode+' | Owner session ใช้ administrator และต้อง login อยู่')
        if ($inspection) {
            Write-Host ('เครื่องหลัก: '+$inspection.bundle.master.label)
            Write-Host ('เครื่องใหม่: '+$inspection.envelope.targetLabel+' | กลุ่ม: '+$inspection.bundle.tenant)
            Write-Host 'อนุญาตให้ agent บนเครื่องหลักสั่งงาน จัดการไฟล์ และเปิด terminal ตามโหมดบัญชีที่เลือก'
            if (-not $successful -and (Read-Host 'พิมพ์ APPROVE เพื่ออนุมัติ หรือ Enter เพื่อยกเลิก') -cne 'APPROVE') { exit 0 }
        } elseif ((Read-Host 'ใช้เครื่องนี้เป็นเครื่องหลัก? พิมพ์ INSTALL เพื่อติดตั้ง') -cne 'INSTALL') { exit 0 }
        if (-not $successful) {
            $child=Start-Setup
            $lastMessage=''
            while (-not $child.HasExited) {
                if (Test-Path -LiteralPath $progressFile) {
                    try {
                        $progress=Get-Content -LiteralPath $progressFile -Raw -Encoding UTF8 | ConvertFrom-Json
                        if ($progress.message -ne $lastMessage) { Write-Host $progress.message; $lastMessage=$progress.message }
                    } catch {}
                }
                Start-Sleep -Milliseconds 200
                $child.Refresh()
            }
            Finish-Setup $child
        }
        Write-Host $statusMessage
        if ($code) { Write-Output $code }
        exit 0
    }
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -AssemblyName System.Drawing
    [Windows.Forms.Application]::EnableVisualStyles()
    $form=New-Object Windows.Forms.Form
    $form.Text='SSH Control Center — ติดตั้งและจับคู่'
    $form.ClientSize=New-Object Drawing.Size(640,490)
    $form.StartPosition='CenterScreen'; $form.MaximizeBox=$false
    $form.Font=New-Object Drawing.Font('Segoe UI',11)
    $heading=New-Object Windows.Forms.Label
    $heading.SetBounds(24,20,590,36); $heading.Font=New-Object Drawing.Font('Segoe UI',16,[Drawing.FontStyle]::Bold)
    $heading.Text=if($inspection){'อนุญาตการเชื่อมต่อเครื่องนี้'}else{'ใช้เครื่องนี้เป็นเครื่องหลัก'}
    $form.Controls.Add($heading)
    $details=New-Object Windows.Forms.Label
    $details.SetBounds(24,65,590,100)
    $details.Text=if($inspection){
        'เครื่องหลัก: '+$inspection.bundle.master.label+[Environment]::NewLine+
        'เครื่องใหม่: '+$inspection.envelope.targetLabel+'   กลุ่ม: '+$inspection.bundle.tenant+[Environment]::NewLine+
        'บัญชี Owner: '+$owner+' | Owner session ใช้ administrator และต้อง login อยู่'
    }else{'ติดตั้ง service เพื่อให้ agent จัดการเครื่องอื่นได้'+[Environment]::NewLine+'บัญชีเจ้าของ: '+$owner+[Environment]::NewLine+'ระบบจะตั้งค่า service และเชื่อม Codex ให้อัตโนมัติ'}
    $form.Controls.Add($details)
    $ownerMode=New-Object Windows.Forms.CheckBox
    $ownerMode.SetBounds(24,160,590,28); $ownerMode.Checked=(-not $existingInstall)
    $ownerMode.Text='ใช้ Owner session (administrator) สำหรับ jobs, files และ terminal'
    $form.Controls.Add($ownerMode)
    $status=New-Object Windows.Forms.TextBox
    $status.SetBounds(24,190,590,100); $status.Multiline=$true; $status.ReadOnly=$true; $status.ScrollBars='Vertical'
    $status.Text=if($successful){$statusMessage}else{'พร้อมติดตั้ง — เมื่อกดปุ่ม Windows จะขอสิทธิ์ administrator'}
    $form.Controls.Add($status)
    $bar=New-Object Windows.Forms.ProgressBar
    $bar.SetBounds(24,303,590,12); $bar.Minimum=0; $bar.Maximum=5
    $form.Controls.Add($bar)
    $codeBox=New-Object Windows.Forms.TextBox
    $codeBox.SetBounds(24,335,590,70); $codeBox.Multiline=$true; $codeBox.ReadOnly=$true; $codeBox.ScrollBars='Vertical'; $codeBox.Text=$code
    $codeBox.Visible=($null -ne $inspection)
    $form.Controls.Add($codeBox)
    $action=New-Object Windows.Forms.Button
    $action.SetBounds(24,428,245,38)
    $action.Text=if($successful -and $inspection){'คัดลอกรหัสจับคู่'}elseif($successful -and $requestedInstallTray){'เปิด SSH Control Center'}elseif($successful){'เสร็จแล้ว'}elseif($inspection){'Approve — อนุมัติและติดตั้ง'}else{'ติดตั้งเครื่องหลัก'}
    $form.Controls.Add($action)
    $close=New-Object Windows.Forms.Button
    $close.SetBounds(480,428,134,38); $close.Text='ปิด'; $close.Add_Click({$form.Close()})
    $form.Controls.Add($close)
    $form.Add_FormClosing({param($sender,$eventArgs) if($script:installing){$eventArgs.Cancel=$true}})
    $action.Add_Click({
        if ($script:successful) {
            if ($inspection) { [Windows.Forms.Clipboard]::SetText($script:code); $status.Text='คัดลอกรหัสแล้ว กลับไปเครื่องหลักแล้ววางในช่อง “รหัสจับคู่”' }
            elseif ($requestedInstallTray) { Start-Process -FilePath (Join-Path $env:ProgramFiles 'SSH Control Center\sshcc-desktop.exe') -ArgumentList '--show' -WindowStyle Hidden }
            else { $form.Close() }
            return
        }
        $script:installing=$true; $action.Enabled=$false; $close.Enabled=$false
        $script:executionMode=if($ownerMode.Checked){'owner-session'}elseif($existingInstall){'preserve'}else{'service'}
        $status.Text='กำลังขอสิทธิ์ administrator และตรวจ prerequisites...'
        try {
            $child=Start-Setup
            while (-not $child.HasExited) {
                if (Test-Path -LiteralPath $progressFile) {
                    try {
                        $progress=Get-Content -LiteralPath $progressFile -Raw -Encoding UTF8 | ConvertFrom-Json
                        $status.Text=$progress.message
                        $bar.Value=[Math]::Min(5,[Math]::Max(0,[int]$progress.step))
                    } catch {}
                }
                [Windows.Forms.Application]::DoEvents()
                Start-Sleep -Milliseconds 180
                $child.Refresh()
            }
            Finish-Setup $child
            $status.Text=$script:statusMessage; $codeBox.Text=$script:code; $bar.Value=5
            $action.Text=if($inspection){'คัดลอกรหัสจับคู่'}elseif($requestedInstallTray){'เปิด SSH Control Center'}else{'เสร็จแล้ว'}
        } catch {
            $status.Text=$_.Exception.Message
            $action.Text='ลองติดตั้งอีกครั้ง'
        } finally {
            $script:installing=$false; $action.Enabled=$true; $close.Enabled=$true
        }
    })
    [void]$form.ShowDialog()
} catch {
    if ($Console -or $InspectOnly) { Write-Error $_.Exception.Message; exit 1 }
    Add-Type -AssemblyName System.Windows.Forms
    [Windows.Forms.MessageBox]::Show($_.Exception.Message,'SSH Control Center','OK','Error') | Out-Null
    exit 1
} finally {
    # Only remove the unique, verified temporary directory created by this process.
    if ([IO.Path]::GetFullPath($work).StartsWith([IO.Path]::GetFullPath([IO.Path]::GetTempPath()),[StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
