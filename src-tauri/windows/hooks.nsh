; open-airplay installer hooks (Tauri NSIS bundler).
; iPhones must reach the receiver engine's AirPlay ports, so the installer adds one inbound firewall
; rule scoped to the bundled uxplay.exe only, and the uninstaller removes it. Mirroring this PC to a TV
; needs the TV to reach the bundled sender (timing and event ports), so it gets its own scoped rule.

!define OAP_FW_RULE "open-airplay receiver"
!define OAP_FW_SENDER_RULE "open-airplay mirror sender"

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
  ; Stop this install's app and engine (only processes running from $INSTDIR, never another UxPlay
  ; or another app with the same name) so their files can be removed.
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "Get-Process open-airplay,uxplay,open-airplay-sender,ffmpeg -ErrorAction SilentlyContinue | Where-Object { $$_.Path -like '$INSTDIR\*' } | Stop-Process -Force"`
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_RULE}"'
  Pop $0
  nsExec::Exec 'netsh advfirewall firewall delete rule name="${OAP_FW_SENDER_RULE}"'
  Pop $0
!macroend
