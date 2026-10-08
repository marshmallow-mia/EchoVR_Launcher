; The Windows installer: the portable folder's files, installed for this user (no
; administrator rights) into %LOCALAPPDATA%\Programs\EchoVR_Launcher, with Start menu and
; desktop shortcuts and an entry in Windows' Apps list. The folder stays the user's, so the
; launcher's own update (Settings, Update now) can replace its files.
;
;   makensis -DVERSION=0.11.8 -DSTAGE=<folder with the portable files> -DOUT=<setup.exe> windows/installer.nsi
;
; A beta or alpha (VERSION=0.11.10-beta.1) also needs -DVIVERSION=0.11.10: Windows' file
; version is numbers only.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user

!ifndef VERSION
  !error "VERSION is needed: makensis -DVERSION=..."
!endif
!ifndef STAGE
  !error "STAGE is needed: the folder with the portable files"
!endif
!ifndef OUT
  !define OUT "Echo_VR_Launcher-${VERSION}-windows-setup.exe"
!endif
!ifndef VIVERSION
  !define VIVERSION "${VERSION}"
!endif

!define NAME "Echo VR Launcher"
!define EXE "EchoVR_Launcher.exe"
!define UNINSTALLER "Uninstall Echo VR Launcher.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\EchoVR_Launcher"
; What the launcher itself writes (src/core/tray.rs, src/core/links.rs).
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
!define RUN_VALUE "Echo VR Launcher (updates)"
!define SPARK_KEY "Software\Classes\spark"

Name "${NAME}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\EchoVR_Launcher"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
BrandingText "${NAME} ${VERSION}"

VIProductVersion "${VIVERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${NAME} setup"
VIAddVersionKey "LegalCopyright" "GPL-3.0"

!include "MUI2.nsh"
!define MUI_ICON "..\icon.ico"
!define MUI_UNICON "..\icon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A file a running launcher holds can't be overwritten or deleted, but it can be renamed: it
; goes aside as <name>.old, which the launcher removes at its next start (as after its update).
!macro PutFile NAME
  Delete "$INSTDIR\${NAME}.old"
  Delete "$INSTDIR\${NAME}"
  IfFileExists "$INSTDIR\${NAME}" 0 +2
    Rename "$INSTDIR\${NAME}" "$INSTDIR\${NAME}.old"
  File "${STAGE}\${NAME}"
!macroend

Section "${NAME}" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; The tray and the launcher, if they run: one started before an update still holds the
  ; replaced files (<name>.old), and then they couldn't be put in.
  nsExec::Exec 'taskkill /F /IM "${EXE}"'
  Sleep 500
  !insertmacro PutFile "${EXE}"
  !insertmacro PutFile "WebView2Loader.dll"
  File "${STAGE}\LICENSE"
  File "${STAGE}\THIRD_PARTY_NOTICES.txt"
  WriteUninstaller "$INSTDIR\${UNINSTALLER}"

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "marshmallow-mia"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/marshmallow-mia/EchoVR_Launcher"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" 40000
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${EXE}" "" "$INSTDIR\${EXE}" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The launcher itself."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A shortcut on the desktop (the Start menu gets one either way)."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  ; The tray that looks for updates, and the launcher, if they run.
  nsExec::Exec 'taskkill /F /IM "${EXE}"'
  Sleep 500
  Delete /REBOOTOK "$INSTDIR\${EXE}"
  Delete /REBOOTOK "$INSTDIR\${EXE}.old"
  Delete /REBOOTOK "$INSTDIR\WebView2Loader.dll"
  Delete /REBOOTOK "$INSTDIR\WebView2Loader.dll.old"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD_PARTY_NOTICES.txt"
  Delete "$INSTDIR\${UNINSTALLER}"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$DESKTOP\${NAME}.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  ; spark:// links, only while they open this launcher.
  ReadRegStr $0 HKCU "${SPARK_KEY}\shell\open\command" ""
  StrCmp $0 "" spark_done
  Push $0
  Push "$INSTDIR"
  Call un.StrContains
  Pop $1
  StrCmp $1 "" spark_done
  DeleteRegKey HKCU "${SPARK_KEY}"
  spark_done:
  ; The launcher's settings, downloads and logs (not the Echo VR builds you installed).
  IfSilent data_done
  MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION "Also remove the launcher's settings, downloads and logs ($LOCALAPPDATA\EchoVR_Launcher)?$\n$\nYour installed Echo VR versions stay where they are." IDNO data_done
  RMDir /r "$LOCALAPPDATA\EchoVR_Launcher"
  data_done:
SectionEnd

; Pushes "" or the needle when the haystack (pushed first) contains the needle (pushed second).
Function un.StrContains
  Exch $R1 ; needle
  Exch
  Exch $R2 ; haystack
  Push $R3
  Push $R4
  Push $R5
  StrLen $R3 $R1
  StrCpy $R4 0
  loop:
    StrCpy $R5 $R2 $R3 $R4
    StrCmp $R5 $R1 found
    StrCmp $R5 "" notfound
    IntOp $R4 $R4 + 1
    Goto loop
  found:
    StrCpy $R1 $R1
    Goto done
  notfound:
    StrCpy $R1 ""
  done:
  Pop $R5
  Pop $R4
  Pop $R3
  Pop $R2
  Exch $R1
FunctionEnd
