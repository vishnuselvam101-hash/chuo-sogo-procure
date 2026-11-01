; One-click Windows trial installer: installs the Procure and Shop apps (their own Tauri
; setups, run silently for the current user, so no administrator rights are needed) and
; puts the trial guide on the desktop. Saved as UTF-8 with BOM so makensis reads the Japanese.
; Run it as the person who will use the apps (not "as administrator": the apps install per user).
; Both apps already point at the demo server.
; Built by scripts/demo/make-trial-package.sh:
;   makensis -DVERSION=1.0.0 -DPKG=<package dir> -DOUTFILE=<exe> -DICON=<ico> trial-installer.nsi
Unicode true
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\ChuoSogo-Trial"
!include "MUI2.nsh"
!include "LogicLib.nsh"

Name "中央総合 調達プラットフォーム 試用版 ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\ChuoSogo-Trial"
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=0 "ProductName" "Chuo-Sogo Procure Trial"
VIAddVersionKey /LANG=0 "CompanyName" "Chuo-Sogo Inc."
VIAddVersionKey /LANG=0 "FileDescription" "Chuo-Sogo Procure trial installer"
VIAddVersionKey /LANG=0 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=0 "LegalCopyright" "© 2026 Chuo-Sogo Inc."

!define MUI_ICON "${ICON}"
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "$(OpenGuide)"
!define MUI_FINISHPAGE_RUN_FUNCTION OpenGuide
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "Japanese"
!insertmacro MUI_LANGUAGE "English"

LangString OpenGuide ${LANG_JAPANESE} "試用ガイドを開く"
LangString OpenGuide ${LANG_ENGLISH} "Open the trial guide"
LangString GuideName ${LANG_JAPANESE} "試用ガイド - 中央総合"
LangString GuideName ${LANG_ENGLISH} "Trial guide - Chuo-Sogo"
LangString Installing ${LANG_JAPANESE} "インストールしています: "
LangString Installing ${LANG_ENGLISH} "Installing: "
LangString Failed ${LANG_JAPANESE} "インストールに失敗しました: "
LangString Failed ${LANG_ENGLISH} "Installation failed: "

Function OpenGuide
  ExecShell "open" "$INSTDIR\START-HERE.html"
FunctionEnd

!macro InstallApp FILE LABEL
  DetailPrint "$(Installing)${LABEL}"
  File "/oname=$PLUGINSDIR\${FILE}" "${PKG}/Windows/${FILE}"
  ExecWait '"$PLUGINSDIR\${FILE}" /S' $0
  ${If} $0 != 0
    MessageBox MB_ICONSTOP "$(Failed)${LABEL} ($0)" /SD IDOK
    Abort
  ${EndIf}
!macroend

Section "Install"
  InitPluginsDir
  SetOutPath "$INSTDIR"
  File "/oname=START-HERE.html" "${PKG}/START-HERE.html"
  !insertmacro InstallApp "ChuoSogo-Procure-Setup.exe" "Chuo-Sogo Procure"
  !insertmacro InstallApp "ChuoSogo-Shop-Setup.exe" "Chuo-Sogo Shop"
  CreateShortCut "$DESKTOP\$(GuideName).lnk" "$INSTDIR\START-HERE.html"
  ; Removes only the guide and its shortcut; each app has its own entry in Apps & features.
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "$(GuideName)"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Chuo-Sogo Inc."
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  Delete "$DESKTOP\$(GuideName).lnk"
  Delete "$INSTDIR\START-HERE.html"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINST_KEY}"
SectionEnd
