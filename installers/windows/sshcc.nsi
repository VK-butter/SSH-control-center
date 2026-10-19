Unicode True
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!ifndef VERSION
!error "VERSION must come from the Cargo workspace release"
!endif
Name "SSH Control Center ${VERSION}"
OutFile "${OUTPUT_FILE}"
RequestExecutionLevel user
SilentInstall silent
CRCCheck off
ShowInstDetails nevershow
Var Parameters
Var BootstrapFlags
Var ConsoleMode

; The native EXE is a single-file bootstrap. Its small Thai approval window is
; created by the embedded PowerShell helper before any privileged setup starts.
; Appended pairing data is data-only and validated by the embedded CLI.
Function .onInit
  InitPluginsDir
  SetOutPath "$PLUGINSDIR\sshcc"
  File "${PACKAGE_DIR}\sshcc.exe"
  File "${PACKAGE_DIR}\Setup.ps1"
  File "${PACKAGE_DIR}\Uninstall.ps1"
  File "${PACKAGE_DIR}\PairingBootstrap.ps1"
  File "${PACKAGE_DIR}\Configure-Codex.ps1"
!if /FileExists "${PACKAGE_DIR}\sshcc-desktop.exe"
  File "${PACKAGE_DIR}\sshcc-desktop.exe"
  File "${PACKAGE_DIR}\sshcc-window.exe"
!endif
!ifdef EMBED_TEMPLATES
  SetOutPath "$PLUGINSDIR\sshcc\templates"
  File /r "${PACKAGE_DIR}\templates\*"
!endif
  ${GetParameters} $Parameters
  StrCpy $ConsoleMode "0"
!ifdef REQUIRE_PAIRING
  StrCpy $BootstrapFlags "-RequirePairing"
!else
  StrCpy $BootstrapFlags ""
!endif
  ClearErrors
  ${GetOptions} $Parameters "/CONSOLE" $0
  ${IfNot} ${Errors}
    StrCpy $BootstrapFlags "$BootstrapFlags -Console"
    StrCpy $ConsoleMode "1"
  ${EndIf}
  SetRegView 64
  ReadRegStr $1 HKLM "SOFTWARE\Microsoft\Windows NT\CurrentVersion" "InstallationType"
  ReadEnvStr $2 SSH_CONNECTION
  ${If} $ConsoleMode != "1"
  ${AndIf} $1 == "Server Core"
    StrCpy $BootstrapFlags "$BootstrapFlags -Console"
    StrCpy $ConsoleMode "1"
  ${ElseIf} $ConsoleMode != "1"
  ${AndIf} $2 != ""
    StrCpy $BootstrapFlags "$BootstrapFlags -Console"
    StrCpy $ConsoleMode "1"
  ${EndIf}
  ClearErrors
  ${GetOptions} $Parameters "/INSPECT" $0
  ${IfNot} ${Errors}
    StrCpy $BootstrapFlags "$BootstrapFlags -InspectOnly"
  ${EndIf}
  ${If} $ConsoleMode == "1"
    ExecWait '"$WINDIR\SysNative\WindowsPowerShell\v1.0\powershell.exe" -Sta -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\sshcc\PairingBootstrap.ps1" -InstallerPath "$EXEPATH" -PackageDirectory "$PLUGINSDIR\sshcc" $BootstrapFlags' $0
  ${Else}
    ExecWait '"$WINDIR\SysNative\WindowsPowerShell\v1.0\powershell.exe" -Sta -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "$PLUGINSDIR\sshcc\PairingBootstrap.ps1" -InstallerPath "$EXEPATH" -PackageDirectory "$PLUGINSDIR\sshcc" $BootstrapFlags' $0
  ${EndIf}
  SetErrorLevel $0
  Quit
FunctionEnd

Section
SectionEnd
