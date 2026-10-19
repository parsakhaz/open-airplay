# Deletes the current user's Windows Credential Manager entries whose names start with any of the
# given prefixes. The uninstaller runs it when the user ticks "Also delete my settings and saved
# pairings": open-airplay-cast: (cast pairings) and open-airplay: (the mirror sender's pairings).
# Prints how many it deleted per prefix, never the entries themselves.
param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Prefix)

Add-Type -Namespace OapUninstall -Name Cred -MemberDefinition @'
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredEnumerateW(string filter, int flags, out int count, out IntPtr credentials);
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredDeleteW(string target, int type, int flags);
[DllImport("advapi32.dll")]
public static extern void CredFree(IntPtr buffer);
'@

$M = [Runtime.InteropServices.Marshal]
foreach ($p in $Prefix) {
  $count = 0
  $list = [IntPtr]::Zero
  $deleted = 0
  # The filter's * matches only after the whole prefix, so "open-airplay:*" never matches "open-airplay-cast:...".
  if ([OapUninstall.Cred]::CredEnumerateW("$p*", 0, [ref]$count, [ref]$list)) {
    try {
      for ($i = 0; $i -lt $count; $i++) {
        $cred = $M::ReadIntPtr($list, $i * [IntPtr]::Size)
        # CREDENTIALW: DWORD Flags, DWORD Type, LPWSTR TargetName, ...
        $type = $M::ReadInt32($cred, 4)
        $target = $M::PtrToStringUni($M::ReadIntPtr($cred, 8))
        if ($target.StartsWith($p) -and [OapUninstall.Cred]::CredDeleteW($target, $type, 0)) { $deleted++ }
      }
    } finally {
      [OapUninstall.Cred]::CredFree($list)
    }
  }
  "$p deleted $deleted"
}
