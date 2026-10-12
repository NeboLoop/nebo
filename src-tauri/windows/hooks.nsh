; Nebo's NSIS installer hooks (tauri.conf.json bundle.windows.nsis.installerHooks).
;
; Install: nothing. The app registers its engine task itself when it opens,
; and only once the engine service is switched on (src-tauri/src/service).
;
; Uninstall: `nebo.exe --engine-service uninstall --app-removed` stops the
; engine the graceful way (POST /api/v1/engine/quit), removes the browser's
; native-messaging host (manifests and HKCU keys: they name the nebo.exe
; being removed) and deletes the engine task. It does nothing unless the
; service was switched on on this machine or a task is left from when it was
; (off by default). Not when the uninstaller runs as part of an update
; (/UPDATE): the same install comes straight back.

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    nsExec::Exec '"$INSTDIR\${MAINBINARYNAME}.exe" --engine-service uninstall --app-removed'
    Pop $0
    DetailPrint "Nebo engine service removed ($0)"
  ${EndIf}
!macroend
