Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define ROOT "..\..\.."
!define PRODUCT_REGISTRY_KEY "Software\Lumio\Lumio Codex"
!define UNINSTALL_REGISTRY_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Lumio Codex"

Name "BestCodex"
OutFile "${ROOT}\dist\windows\LumioCodex-${VERSION}-windows-x64-setup-internal-unsigned.exe"
InstallDir "$LOCALAPPDATA\Programs\Lumio Codex"
InstallDirRegKey HKCU "${PRODUCT_REGISTRY_KEY}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!define MUI_ICON "${ROOT}\apps\codex-plus-manager\src-tauri\icons\icon.ico"
!define MUI_UNICON "${ROOT}\apps\codex-plus-manager\src-tauri\icons\icon.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  nsExec::ExecToLog 'taskkill /IM lumio-codex.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM lumio-codex-launcher.exe /F'
  Pop $0

  SetOutPath "$INSTDIR"
  File "${ROOT}\dist\windows\app\lumio-codex.exe"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  SetOutPath "$INSTDIR\Helpers"
  File "${ROOT}\dist\windows\app\lumio-codex-launcher.exe"

  SetOutPath "$INSTDIR"
  CreateShortcut "$DESKTOP\BestCodex.lnk" "$INSTDIR\lumio-codex.exe" "" "$INSTDIR\lumio-codex.exe"
  CreateDirectory "$SMPROGRAMS\Lumio Codex"
  CreateShortcut "$SMPROGRAMS\Lumio Codex\BestCodex.lnk" "$INSTDIR\lumio-codex.exe" "" "$INSTDIR\lumio-codex.exe"

  WriteRegStr HKCU "${PRODUCT_REGISTRY_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "DisplayName" "BestCodex"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "Publisher" "Lumio"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "DisplayIcon" "$INSTDIR\lumio-codex.exe"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_REGISTRY_KEY}" "UninstallString" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /IM lumio-codex.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM lumio-codex-launcher.exe /F'
  Pop $0

  Delete "$DESKTOP\BestCodex.lnk"
  Delete "$SMPROGRAMS\Lumio Codex\BestCodex.lnk"
  RMDir "$SMPROGRAMS\Lumio Codex"

  Delete "$INSTDIR\Helpers\lumio-codex-launcher.exe"
  RMDir "$INSTDIR\Helpers"
  Delete "$INSTDIR\lumio-codex.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINSTALL_REGISTRY_KEY}"
  DeleteRegKey HKCU "${PRODUCT_REGISTRY_KEY}"
SectionEnd
