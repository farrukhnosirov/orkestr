; Orkestr NSIS installer hooks (ADR-0048), included by Tauri's installer template.
;
; User data lives outside the install folder and survives updates and
; uninstalls by default:
;   %APPDATA%\Orkestr\Orkestr\{data,config}   (or ORKESTR_HOME)
;   %LOCALAPPDATA%\Orkestr\Orkestr\cache       (disposable)
; Updates run the installer with /UPDATE and never reach the uninstall branch
; below. A real uninstall removes the disposable cache; the data and settings
; are deleted only when the user ticks "Delete the application data".

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    SetShellVarContext current
    RmDir /r "$LOCALAPPDATA\Orkestr\Orkestr\cache"
    RmDir "$LOCALAPPDATA\Orkestr\Orkestr"
    ${If} $DeleteAppDataCheckboxState = 1
      RmDir /r "$APPDATA\Orkestr\Orkestr"
      RmDir "$APPDATA\Orkestr"
      RmDir /r "$LOCALAPPDATA\Orkestr\Orkestr"
    ${EndIf}
    ; The per-user install folder, when nothing else is left in it.
    RmDir "$INSTDIR"
  ${EndIf}
!macroend
