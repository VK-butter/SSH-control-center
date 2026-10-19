#requires -Version 5.1
[CmdletBinding()]
param(
    [string]$ConfigPath,
    [string]$CliPath = (Join-Path $env:ProgramFiles 'SSH Control Center\sshcc.exe')
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$explicitPath=[bool]$ConfigPath
if (-not $ConfigPath) {
    $codexDirectory=if($env:CODEX_HOME){$env:CODEX_HOME}else{Join-Path ([Environment]::GetFolderPath('UserProfile')) '.codex'}
    if (-not (Test-Path -LiteralPath $codexDirectory) -and -not (Get-Command codex -ErrorAction SilentlyContinue)) { return 'ไม่พบ Codex สามารถเชื่อม MCP ภายหลังได้' }
    $ConfigPath=Join-Path $codexDirectory 'config.toml'
}
if (-not (Test-Path -LiteralPath $CliPath -PathType Leaf)) { throw 'ไม่พบโปรแกรม SSH Control Center สำหรับ MCP' }
if ($CliPath.Contains("'") -or $CliPath.Contains([char]13) -or $CliPath.Contains([char]10)) { throw 'เส้นทาง MCP ไม่ถูกต้อง' }
$directory=Split-Path -Parent ([IO.Path]::GetFullPath($ConfigPath))
New-Item -ItemType Directory -Path $directory -Force | Out-Null
$before=if(Test-Path -LiteralPath $ConfigPath){[IO.File]::ReadAllText($ConfigPath)}else{''}
$header='[mcp_servers.ssh_control_center]'
$managed=@"
command = '$CliPath'
args = ["mcp"]
startup_timeout_sec = 20
tool_timeout_sec = 150
enabled = true
"@
# Change only root keys owned by this connector. Preserve other MCP servers,
# custom fields and any nested env table, and avoid touching shared settings.
$pattern='(?ms)^[ \t]*\[mcp_servers\.ssh_control_center\][ \t]*(?:#[^\r\n]*)?\r?\n(?<body>.*?)(?=^[ \t]*\[|\z)'
$match=[regex]::Match($before,$pattern)
if($match.Success){
    $rest=[regex]::Replace($match.Groups['body'].Value,'(?ms)^[ \t]*args[ \t]*=[ \t]*\[(?:[^\]"''#]|"(?:\\.|[^"\\])*"|''[^'']*''|#[^\r\n]*)*\][ \t]*(?:#[^\r\n]*)?(?:\r?\n|$)','')
    if ($rest -match '(?m)^[ \t]*args[ \t]*=') { throw 'รูปแบบ args ของ ssh_control_center อ่านไม่ได้ กรุณาตรวจ config.toml ก่อนลองใหม่' }
    $rest=[regex]::Replace($rest,'(?m)^[ \t]*(command|startup_timeout_sec|tool_timeout_sec|enabled)[ \t]*=.*(?:\r?\n|$)','')
    $replacement=$header+[Environment]::NewLine+$managed+[Environment]::NewLine+$rest
    $after=$before.Substring(0,$match.Index)+$replacement+$before.Substring($match.Index+$match.Length)
}else{
    # A malformed/exotic spelling of our table needs explicit manual repair;
    # don't append an ambiguous duplicate table.
    if($before -match '(?m)^[ \t]*\[[^\r\n]*ssh_control_center[^\r\n]*\]'){
        throw 'พบการตั้งค่า ssh_control_center ที่อ่านไม่ได้ กรุณาตรวจ config.toml ก่อนลองใหม่'
    }
    $after=$before.TrimEnd()+[Environment]::NewLine+[Environment]::NewLine+$header+[Environment]::NewLine+$managed+[Environment]::NewLine
}
if($after -ne $before){
    if(Test-Path -LiteralPath $ConfigPath){
        $backup=$ConfigPath+'.sshcc-backup-'+(Get-Date -Format 'yyyyMMddTHHmmssfff')
        Copy-Item -LiteralPath $ConfigPath -Destination $backup
    }
    $temporary=Join-Path $directory ('.sshcc-config-'+[guid]::NewGuid()+'.tmp')
    try{
        [IO.File]::WriteAllText($temporary,$after,(New-Object Text.UTF8Encoding($false)))
        Move-Item -LiteralPath $temporary -Destination $ConfigPath -Force
    }finally{if(Test-Path -LiteralPath $temporary){Remove-Item -LiteralPath $temporary -Force}}
}
return 'เชื่อม Codex แล้ว เปิด session ใหม่เพื่อโหลด tools ของ ssh_control_center'
