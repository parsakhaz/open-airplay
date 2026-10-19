; open-airplay installer hooks (Tauri NSIS bundler).
; iPhones must reach the receiver engine's AirPlay ports, so the installer adds one inbound firewall
; rule scoped to the bundled uxplay.exe only, and the uninstaller removes it. Mirroring this PC to a TV
; needs the TV to reach the bundled sender (timing and event ports), so it gets its own scoped rule.

; Each name can be defined before this file is included, so a test build never touches a real install's
; firewall rules or saved pairings.
!ifndef OAP_FW_RULE
  !define OAP_FW_RULE "open-airplay receiver"
!endif
!ifndef OAP_FW_SENDER_RULE
  !define OAP_FW_SENDER_RULE "open-airplay mirror sender"
!endif
!ifndef OAP_CREDENTIAL_PREFIXES
  !define OAP_CREDENTIAL_PREFIXES "open-airplay-cast: open-airplay:"
!endif

; An upgrade never deletes settings or pairings, and never offers to. A standalone uninstall always re-runs
; itself from %TEMP% first; a setup runs it in place (from $INSTDIR, with _?=), without /UPDATE or /P, both
; for another version's "uninstall before installing" (an upgrade or downgrade) and for the same version's
; explicit Uninstall choice (a removal, which keeps the box). In place, uninstall-context.ps1 tells the two
; apart by the launching setup's version; unknown counts as an upgrade. On an upgrade, or with /UPDATE, the
; uninstaller goes passive, which makes Tauri skip the confirm page that holds the "Also delete my settings
; and saved pairings" box, and NSIS_HOOK_PREUNINSTALL forces the box's state off as well.
; Tauri declares $UpdateMode and $PassiveMode after including this file, so the function is a macro that
; English.nsh (included after them) inserts.
Var OapUpgrade
!define MUI_CUSTOMFUNCTION_UNGUIINIT un.OapGuiInit
!macro OAP_UNINSTALL_GUIINIT
  Function un.OapGuiInit
    Push $0
    Push $1
    Push $R8
    Push $R9
    StrCpy $OapUpgrade 0
    StrCpy $R9 "$INSTDIR"
    StrCpy $R8 "$R9" "" -1
    ${If} $R8 == "\"
      StrCpy $R9 "$R9" -1
    ${EndIf}
    ${If} $UpdateMode = 1
      StrCpy $OapUpgrade 1
    ${ElseIf} "$EXEDIR" == "$R9"
      System::Call 'kernel32::GetCurrentProcessId() i .r0'
      nsExec::ExecToStack `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\uninstall-context.ps1" -UninstallerPid $0 -Version "${VERSION}"`
      Pop $0 ; exit code
      Pop $1 ; "remove" or "upgrade", then a line break
      StrCpy $1 "$1" 6
      ${If} $1 != "remove"
        StrCpy $OapUpgrade 1
      ${EndIf}
    ${EndIf}
    ${If} $OapUpgrade = 1
      StrCpy $PassiveMode 1
    ${EndIf}
    Pop $R9
    Pop $R8
    Pop $1
    Pop $0
  FunctionEnd
!macroend

!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_RULE}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall add rule name="${OAP_FW_RULE}" dir=in action=allow program="$INSTDIR\engine\bin\uxplay.exe" enable=yes profile=any'
  Pop $0
  DetailPrint "Firewall rule for the receiver: netsh exit code $0"
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_SENDER_RULE}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall add rule name="${OAP_FW_SENDER_RULE}" dir=in action=allow program="$INSTDIR\engine\bin\open-airplay-sender.exe" enable=yes profile=any'
  Pop $0
  DetailPrint "Firewall rule for the mirror sender: netsh exit code $0"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Stop this install's app, engine and cast sidecar (only processes running from $INSTDIR, never another UxPlay
  ; or another app with the same name) so their files can be removed.
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Get-Process open-airplay,uxplay,open-airplay-sender,ffmpeg,oap-sender -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force"`
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_RULE}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_SENDER_RULE}"'
  Pop $0
  ; The uninstaller's "Also delete my settings and saved pairings" box (Tauri's delete-app-data box, worded in
  ; English.nsh) is unticked by default, and a silent uninstall never ticks it. Ticked, Tauri removes the
  ; config and local data folders (settings, pairing keys, logs) after this hook; here, while the bundled
  ; script is still installed, the saved pairings in Credential Manager go too: cast devices
  ; (open-airplay-cast:) and the mirror sender's TVs (open-airplay:). Never on an update.
  ${If} $OapUpgrade = 1
    StrCpy $DeleteAppDataCheckboxState 0 ; Tauri's folder removal checks it after this hook
  ${EndIf}
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    ; This runs as the account the uninstaller runs as. If that isn't the account signed in (a standard
    ; user's uninstall approved by an administrator), the script names the account it cleaned and says
    ; the signed-in user's data stayed; Tauri's folder removal acts for the same account.
    nsExec::ExecToStack `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\forget-credentials.ps1" -AppId "${BUNDLEID}" ${OAP_CREDENTIAL_PREFIXES}`
    Pop $0 ; 0 all done; 1 some entries left; 2 another (or an unidentified) account's data may remain; 3 both (other: PowerShell failed)
    Pop $1 ; what it did and what is left, in words (counts only, never the entries)
    DetailPrint "Credential Manager: $1"
    ${If} $0 <> 0
      DetailPrint "Not all saved settings and pairings were removed (exit code $0)."
      MessageBox MB_ICONEXCLAMATION|MB_OK "The uninstall finished, but not everything was removed.$\r$\n$\r$\n$1" /SD IDOK
    ${EndIf}
  ${EndIf}
!macroend
