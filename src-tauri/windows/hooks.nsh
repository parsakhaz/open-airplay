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
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    nsExec::ExecToStack `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\forget-credentials.ps1" ${OAP_CREDENTIAL_PREFIXES}`
    Pop $0 ; exit code: 0 when everything went, anything else when some is left (or PowerShell didn't run)
    Pop $1 ; what it did, one line per prefix (counts only)
    DetailPrint "Credential Manager: $1"
    ${If} $0 <> 0
      DetailPrint "Some saved pairings couldn't be removed from Credential Manager (exit code $0)."
      MessageBox MB_ICONEXCLAMATION|MB_OK "Some saved pairings couldn't be removed from Windows Credential Manager.$\r$\n$\r$\n$1$\r$\nYou can remove the ones named open-airplay in Control Panel > Credential Manager > Windows Credentials." /SD IDOK
    ${EndIf}
  ${EndIf}
!macroend
