"""Windows Firewall check and a user-approved (UAC) allow rule, so receivers can fetch the stream from this PC."""
from __future__ import annotations

import ctypes
import json
import subprocess
import sys

RULE = "open-airplay screencast"


def status(port: int) -> dict:
    """Is there an enabled inbound Allow rule for this Python (any port, or ours) on the active profiles?"""
    ps = (
        f"$p = '{sys.executable}';"
        "$r = Get-NetFirewallApplicationFilter -Program $p -ErrorAction SilentlyContinue | Get-NetFirewallRule |"
        " ? { $_.Enabled -eq 'True' -and $_.Direction -eq 'Inbound' -and $_.Action -eq 'Allow' };"
        "$prof = (Get-NetConnectionProfile | % { $_.NetworkCategory.ToString() }) -join ',';"
        "@{ rules = @($r | % { $_.Profile.ToString() }); profiles = $prof } | ConvertTo-Json -Compress"
    )
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True, timeout=20,
                             creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout
        data = json.loads(out or "{}")
    except Exception as e:
        return {"allowed": None, "detail": f"could not read firewall rules: {e}"}
    rules = data.get("rules") or []
    profiles = [p for p in (data.get("profiles") or "").split(",") if p]
    covered = all(any(p in r or r == "Any" for r in rules) for p in profiles) if rules else False
    return {"allowed": covered, "profiles": profiles, "rules": rules,
            "detail": "Receivers can reach this PC." if covered else
                      f"Windows Firewall may block receivers on your {'/'.join(profiles) or 'current'} network."}


def request_allow(port: int) -> bool:
    """Ask Windows (UAC prompt, the user approves) to add an inbound allow rule for this app's port only."""
    args = (f'advfirewall firewall add rule name="{RULE}" dir=in action=allow enable=yes '
            f'program="{sys.executable}" protocol=TCP localport={port} profile=private,public')
    return ctypes.windll.shell32.ShellExecuteW(None, "runas", "netsh", args, None, 0) > 32
