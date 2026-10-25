; SPDX-License-Identifier: EPL-2.0
;
; The Windows installer (NSIS 3), built by tools/release/package.py (`win`), which
; generates the file lists and passes the /D definitions:
;   VERSION       0.1.0           VIVERSION   0.1.0.0
;   SRC           the folder of the Windows archive (programs, lib, robots, documents)
;   OUT           the setup program to write
;   INSTALL_LIST  / UNINSTALL_LIST   the File and Delete/RMDir lines for SRC
;   ICON          robocode-s.ico
;
; Per user, no administrator: the folder is %LOCALAPPDATA%\Programs\robocode-s, which the
; user owns, so the `os` sandbox can grant its AppContainer access by ACL entries at run
; time (an installation in Program Files would need these grants made by the installer).
; Everything else is HKCU. The uninstaller removes exactly the files installed (a robot
; the user put in robots\ stays, with its folder) and does not touch the data folder
; (%LOCALAPPDATA%\robocode-s: the Java runtime) or the settings (%APPDATA%\robocode-s).
;
; Command line: /S installs silently with the default components (the Start menu
; shortcut, PATH and the file types, no desktop shortcut); /NOPATH, /NOASSOC,
; /NOSTARTMENU and /DESKTOP change them; /D=DIR (last, no quotes) names the folder.
; /UPDATE is the programs' own update (crates/update, ADR 0022): over an installation it
; keeps the components that installation has, and waits up to two minutes for its programs
; to exit (giving up without changing anything); /RELAUNCH starts the window when done.

Unicode true
SetCompressor /SOLID lzma
RequestExecutionLevel user
ManifestDPIAware true

!define NAME "robocode-s"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\robocode-s"
!define APP_KEY "Software\robocode-s"
!define ENV_KEY "Environment"

Name "${NAME} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\robocode-s"
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"
BrandingText "robocode-s ${VERSION} (unsigned)"

VIProductVersion "${VIVERSION}"
VIAddVersionKey "ProductName" "robocode-s"
VIAddVersionKey "FileDescription" "robocode-s installer"
VIAddVersionKey "CompanyName" "Nick Hofstede"
VIAddVersionKey "LegalCopyright" "EPL-2.0"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"

!include MUI2.nsh
!include LogicLib.nsh
!include FileFunc.nsh
!include StrFunc.nsh
${StrStr}
${UnStrRep}

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\robocode-s-gui.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start robocode-s"
!define MUI_COMPONENTSPAGE_SMALLDESC

!insertmacro MUI_PAGE_LICENSE "${SRC}\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Var Upgrade
Var Relaunch

Section "robocode-s" SecMain
  SectionIn RO
  ${If} $Upgrade == 1
    ReadRegStr $0 HKCU "${APP_KEY}" "InstallDir"
    DetailPrint "Removing the previous version in $0"
    ; _?= keeps the uninstaller from copying itself away, so that ExecWait waits for it.
    ExecWait '"$0\uninstall.exe" /S _?=$0'
    Delete "$0\uninstall.exe"
  ${EndIf}
  SetOutPath "$INSTDIR"
  !include "${INSTALL_LIST}"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${APP_KEY}" "Version" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "robocode-s"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Nick Hofstede"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\robocode-s-gui.exe,0"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKCU "${UNINST_KEY}" "URLInfoAbout" "https://github.com/nhofsted/robocode-s"
  WriteRegStr HKCU "${UNINST_KEY}" "HelpLink" "https://github.com/nhofsted/robocode-s"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" $0
SectionEnd

Section "Start menu shortcut" SecStart
  SetOutPath "$INSTDIR"
  CreateShortCut "$SMPROGRAMS\robocode-s.lnk" "$INSTDIR\robocode-s-gui.exe" "" "$INSTDIR\robocode-s-gui.exe" 0
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  SetOutPath "$INSTDIR"
  CreateShortCut "$DESKTOP\robocode-s.lnk" "$INSTDIR\robocode-s-gui.exe" "" "$INSTDIR\robocode-s-gui.exe" 0
SectionEnd

Section "Add to PATH (robocode-s in Command Prompt and PowerShell)" SecPath
  ClearErrors
  ReadRegStr $0 HKCU "${ENV_KEY}" "Path"
  StrLen $1 $0
  ; NSIS strings hold 1024 characters: a longer PATH would be cut, so it is left alone.
  ; A PATH too long to read at all reads as "" with the error flag, as does a missing
  ; one: either way it is left alone rather than replaced.
  ${If} ${Errors}
    DetailPrint "The user's PATH couldn't be read; robocode-s was not added to it."
  ${ElseIf} $1 > 900
    DetailPrint "The user's PATH is too long to be changed safely; robocode-s was not added to it."
  ${Else}
    ${StrStr} $2 ";$0;" ";$INSTDIR;"
    ${If} $2 == ""
      ${If} $0 == ""
        StrCpy $0 "$INSTDIR"
      ${Else}
        StrCpy $0 "$0;$INSTDIR"
      ${EndIf}
      ; REG_EXPAND_SZ, and read without expanding, so the entries with %VARIABLES% stay.
      WriteRegExpandStr HKCU "${ENV_KEY}" "Path" "$0"
      WriteRegDWORD HKCU "${APP_KEY}" "PathAdded" 1
      SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
    ${EndIf}
  ${EndIf}
SectionEnd

Section "Open recordings (.rsr, .track) with robocode-s" SecAssoc
  WriteRegStr HKCU "Software\Classes\robocode-s.recording" "" "robocode-s battle recording"
  WriteRegStr HKCU "Software\Classes\robocode-s.recording\DefaultIcon" "" "$INSTDIR\robocode-s-gui.exe,0"
  WriteRegStr HKCU "Software\Classes\robocode-s.recording\shell\open\command" "" '"$INSTDIR\robocode-s-gui.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\robocode-s.track" "" "robocode-s website replay track"
  WriteRegStr HKCU "Software\Classes\robocode-s.track\DefaultIcon" "" "$INSTDIR\robocode-s-gui.exe,0"
  WriteRegStr HKCU "Software\Classes\robocode-s.track\shell\open\command" "" '"$INSTDIR\robocode-s-gui.exe" "%1"'
  WriteRegStr HKCU "Software\Classes\.rsr" "" "robocode-s.recording"
  WriteRegStr HKCU "Software\Classes\.track" "" "robocode-s.track"
  ; .rec is a common extension: offered under Open with, not taken over.
  WriteRegStr HKCU "Software\Classes\.rec\OpenWithProgids" "robocode-s.recording" ""
  WriteRegDWORD HKCU "${APP_KEY}" "Associated" 1
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd

Function .onInit
  ; An installation to replace: the old uninstaller removes its files, the shortcuts, the
  ; PATH entry and the file types, which this installer then makes again as chosen.
  StrCpy $Upgrade 0
  ReadRegStr $0 HKCU "${APP_KEY}" "InstallDir"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\uninstall.exe"
    StrCpy $Upgrade 1
  ${EndIf}
  ; The programs' own update: the installation's components, once its programs exited.
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/UPDATE" $1
  ${IfNot} ${Errors}
  ${AndIf} $Upgrade == 1
    ReadRegStr $2 HKCU "${APP_KEY}" "InstallDir"
    Call WaitForPrograms
    ${IfNot} ${FileExists} "$SMPROGRAMS\robocode-s.lnk"
      SectionSetFlags ${SecStart} 0
    ${EndIf}
    ${If} ${FileExists} "$DESKTOP\robocode-s.lnk"
      SectionSetFlags ${SecDesktop} ${SF_SELECTED}
    ${EndIf}
    ReadRegDWORD $3 HKCU "${APP_KEY}" "PathAdded"
    ${If} $3 != 1
      SectionSetFlags ${SecPath} 0
    ${EndIf}
    ReadRegDWORD $3 HKCU "${APP_KEY}" "Associated"
    ${If} $3 != 1
      SectionSetFlags ${SecAssoc} 0
    ${EndIf}
  ${EndIf}
  StrCpy $Relaunch 0
  ClearErrors
  ${GetOptions} $0 "/RELAUNCH" $1
  ${IfNot} ${Errors}
    StrCpy $Relaunch 1
  ${EndIf}
  ; Command-line choices of components.
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/NOPATH" $1
  ${IfNot} ${Errors}
    SectionSetFlags ${SecPath} 0
  ${EndIf}
  ClearErrors
  ${GetOptions} $0 "/DESKTOP" $1
  ${IfNot} ${Errors}
    SectionSetFlags ${SecDesktop} ${SF_SELECTED}
  ${EndIf}
  ClearErrors
  ${GetOptions} $0 "/NOASSOC" $1
  ${IfNot} ${Errors}
    SectionSetFlags ${SecAssoc} 0
  ${EndIf}
  ClearErrors
  ${GetOptions} $0 "/NOSTARTMENU" $1
  ${IfNot} ${Errors}
    SectionSetFlags ${SecStart} 0
  ${EndIf}
FunctionEnd

; Waits until the programs in folder $2 have exited (a running program can't be opened
; for writing), at most two minutes; then quits, having changed nothing.
Function WaitForPrograms
  StrCpy $4 0
  ${Do}
    StrCpy $5 0
    ${ForEach} $6 0 1 + 1
      ${If} $6 == 0
        StrCpy $7 "$2\robocode-s-gui.exe"
      ${Else}
        StrCpy $7 "$2\robocode-s.exe"
      ${EndIf}
      ${If} ${FileExists} $7
        ClearErrors
        FileOpen $8 $7 a
        ${If} ${Errors}
          StrCpy $5 1
        ${Else}
          FileClose $8
        ${EndIf}
      ${EndIf}
    ${Next}
    ${If} $5 == 0
      ${Break}
    ${EndIf}
    IntOp $4 $4 + 1
    ${If} $4 > 240
      Abort
    ${EndIf}
    Sleep 500
  ${Loop}
FunctionEnd

Function .onInstSuccess
  ${If} $Relaunch == 1
    SetOutPath "$INSTDIR"
    Exec '"$INSTDIR\robocode-s-gui.exe"'
  ${EndIf}
FunctionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The programs, the robot host and the sample robots. The Java runtime for the robots is downloaded on the first battle."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStart} "A Start menu entry for the window."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecPath} "Adds the installation folder to your user PATH, so that robocode-s runs in a new Command Prompt or PowerShell."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAssoc} "Double-clicking a recording or a website replay track opens it in robocode-s."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  !include "${UNINSTALL_LIST}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\robocode-s.lnk"
  Delete "$DESKTOP\robocode-s.lnk"

  ; The PATH entry, if this installation added it.
  ReadRegDWORD $0 HKCU "${APP_KEY}" "PathAdded"
  ${If} $0 == 1
    ClearErrors
    ReadRegStr $0 HKCU "${ENV_KEY}" "Path"
    ; Unreadable (grown past NSIS's 1024 characters since, or gone): left alone, not cut.
    ${If} ${Errors}
      DetailPrint "The user's PATH couldn't be read; remove $INSTDIR from it by hand."
    ${Else}
      ${UnStrRep} $1 ";$0;" ";$INSTDIR;" ";"
      StrCpy $2 $1 1
      ${If} $2 == ";"
        StrCpy $1 $1 "" 1
      ${EndIf}
      StrCpy $2 $1 1 -1
      ${If} $2 == ";"
        StrCpy $1 $1 -1
      ${EndIf}
      ${If} $1 != $0
        ${If} $1 == ""
          DeleteRegValue HKCU "${ENV_KEY}" "Path"
        ${Else}
          WriteRegExpandStr HKCU "${ENV_KEY}" "Path" "$1"
        ${EndIf}
        SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
      ${EndIf}
    ${EndIf}
  ${EndIf}

  ; The file types, if they are still ours.
  ReadRegDWORD $0 HKCU "${APP_KEY}" "Associated"
  ${If} $0 == 1
    ReadRegStr $0 HKCU "Software\Classes\.rsr" ""
    ${If} $0 == "robocode-s.recording"
      DeleteRegKey HKCU "Software\Classes\.rsr"
    ${EndIf}
    ReadRegStr $0 HKCU "Software\Classes\.track" ""
    ${If} $0 == "robocode-s.track"
      DeleteRegKey HKCU "Software\Classes\.track"
    ${EndIf}
    DeleteRegValue HKCU "Software\Classes\.rec\OpenWithProgids" "robocode-s.recording"
    DeleteRegKey /ifempty HKCU "Software\Classes\.rec\OpenWithProgids"
    DeleteRegKey /ifempty HKCU "Software\Classes\.rec"
    DeleteRegKey HKCU "Software\Classes\robocode-s.recording"
    DeleteRegKey HKCU "Software\Classes\robocode-s.track"
    System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
  ${EndIf}

  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "${APP_KEY}"
SectionEnd
