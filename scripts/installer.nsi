; installer.nsi — NSIS installer script for Nebo (Windows)
;
; Prerequisites:
;   - NSIS 3.x installed (https://nsis.sourceforge.io)
;   - nebo-windows-amd64.exe built and available
;
; Usage (from repo root):
;   makensis /DVERSION=1.2.3 /DEXE_PATH=nebo-windows-amd64.exe scripts/installer.nsi
;
; The output installer is written to dist/Nebo-{VERSION}-setup.exe
;
; To add a custom icon, create assets/icons/nebo.ico and uncomment the
; MUI_ICON / MUI_UNICON lines below.

;--- Includes ----------------------------------------------------------------
!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"

;--- Build-time defines (pass via /D on command line) ------------------------
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE_PATH
  !define EXE_PATH "nebo-windows-amd64.exe"
!endif

;--- Installer attributes ----------------------------------------------------
Name "Nebo ${VERSION}"
OutFile "..\dist\Nebo-${VERSION}-setup.exe"
InstallDir "$PROGRAMFILES64\Nebo"
InstallDirRegKey HKLM "Software\Nebo" "InstallDir"
RequestExecutionLevel admin
Unicode True

;--- Modern UI configuration -------------------------------------------------
!define MUI_ABORTWARNING

!define MUI_ICON "..\assets\icons\nebo.ico"
!define MUI_UNICON "..\assets\icons\nebo.ico"

; Branding
!define MUI_WELCOMEPAGE_TITLE "Welcome to Nebo Setup"
!define MUI_WELCOMEPAGE_TEXT "This will install Nebo ${VERSION} on your computer.$\r$\n$\r$\nNebo is an AI agent with a web UI — your personal AI companion.$\r$\n$\r$\nClick Next to continue."
!define MUI_FINISHPAGE_TITLE "Nebo ${VERSION} Installed"
!define MUI_FINISHPAGE_TEXT "Nebo has been installed on your computer.$\r$\n$\r$\nClick Finish to close this wizard."
!define MUI_FINISHPAGE_RUN "$INSTDIR\nebo.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Launch Nebo"

;--- Pages -------------------------------------------------------------------
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

; Uninstaller pages
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

;--- Language ----------------------------------------------------------------
!insertmacro MUI_LANGUAGE "English"

;--- Installer section -------------------------------------------------------
Section "Nebo" SecMain
  SectionIn RO ; Required section — cannot be deselected

  SetOutPath "$INSTDIR"

  ; Install the main executable
  File "/oname=nebo.exe" "${EXE_PATH}"

  ; Create uninstaller
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; Start Menu shortcuts
  CreateDirectory "$SMPROGRAMS\Nebo"
  CreateShortcut "$SMPROGRAMS\Nebo\Nebo.lnk" "$INSTDIR\nebo.exe"
  CreateShortcut "$SMPROGRAMS\Nebo\Uninstall Nebo.lnk" "$INSTDIR\uninstall.exe"

  ; Desktop shortcut
  CreateShortcut "$DESKTOP\Nebo.lnk" "$INSTDIR\nebo.exe"

  ; Add install directory to system PATH via registry
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path"
  StrCpy $0 "$0;$INSTDIR"
  WriteRegExpandStr HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path" "$0"
  ; Broadcast environment change so running shells pick it up
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000

  ; Register in Add/Remove Programs
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "DisplayName" "Nebo"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "Publisher" "Nebo Labs"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "URLInfoAbout" "https://github.com/NeboLoop/nebo"
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "NoRepair" 1

  ; Compute and write installed size (in KB)
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo" "EstimatedSize" $0

  ; Store install dir for future upgrades
  WriteRegStr HKLM "Software\Nebo" "InstallDir" "$INSTDIR"
SectionEnd

;--- Uninstaller section -----------------------------------------------------
Section "Uninstall"
  ; Remove files
  Delete "$INSTDIR\nebo.exe"
  Delete "$INSTDIR\uninstall.exe"

  ; Remove shortcuts
  Delete "$SMPROGRAMS\Nebo\Nebo.lnk"
  Delete "$SMPROGRAMS\Nebo\Uninstall Nebo.lnk"
  RMDir "$SMPROGRAMS\Nebo"
  Delete "$DESKTOP\Nebo.lnk"

  ; Remove install directory from system PATH
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path"
  ; Remove ";$INSTDIR" from the PATH string
  ${WordReplace} "$0" ";$INSTDIR" "" "+" $0
  WriteRegExpandStr HKLM "SYSTEM\CurrentControlSet\Control\Session Manager\Environment" "Path" "$0"
  SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000

  ; Remove install directory (only if empty)
  RMDir "$INSTDIR"

  ; Remove registry entries
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Nebo"
  DeleteRegKey HKLM "Software\Nebo"
SectionEnd
