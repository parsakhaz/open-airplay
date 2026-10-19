# Deletes the current user's Windows Credential Manager entries whose names start with any of the
# given prefixes. The uninstaller runs it when the user ticks "Also delete my settings and saved
# pairings": open-airplay-cast: (cast pairings) and open-airplay: (the mirror sender's pairings).
# Prints one line per prefix (counts, never the entries themselves). Exits 1 when any entry could not
# be listed or deleted, so the uninstaller can say the cleanup is incomplete.
# Tests dot-source this file (tests/installer/forget-credentials.tests.ps1) and pass a fake -Native.
param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Prefix)

$ERROR_NOT_FOUND = 1168

# Credential Manager through advapi32. Enumerate returns @{ Error; Items = @(@{ Target; Type }) },
# Delete returns the Win32 error (0 when it worked).
function New-NativeCredentials {
  Add-Type -Namespace OapUninstall -Name Cred -MemberDefinition @'
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredEnumerateW(string filter, int flags, out int count, out IntPtr credentials);
[DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern bool CredDeleteW(string target, int type, int flags);
[DllImport("advapi32.dll")]
public static extern void CredFree(IntPtr buffer);
'@
  $native = New-Object psobject
  $native | Add-Member ScriptMethod Enumerate {
    param([string]$Filter)
    $M = [Runtime.InteropServices.Marshal]
    $count = 0
    $list = [IntPtr]::Zero
    if (-not [OapUninstall.Cred]::CredEnumerateW($Filter, 0, [ref]$count, [ref]$list)) {
      return @{ Error = $M::GetLastWin32Error(); Items = @() }
    }
    try {
      $items = for ($i = 0; $i -lt $count; $i++) {
        $cred = $M::ReadIntPtr($list, $i * [IntPtr]::Size)
        # CREDENTIALW: DWORD Flags, DWORD Type, LPWSTR TargetName, ...
        @{ Type = $M::ReadInt32($cred, 4); Target = $M::PtrToStringUni($M::ReadIntPtr($cred, 8)) }
      }
      return @{ Error = 0; Items = @($items) }
    } finally {
      [OapUninstall.Cred]::CredFree($list)
    }
  }
  $native | Add-Member ScriptMethod Delete {
    param([string]$Target, [int]$Type)
    if ([OapUninstall.Cred]::CredDeleteW($Target, $Type, 0)) { return 0 }
    return [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  }
  return $native
}

# Deletes every entry under each prefix. "Not found" (nothing saved, or already gone) is not a failure.
# Returns @{ Lines; Failed }: Lines for the log, Failed true when anything was left behind.
function Remove-SavedCredentials {
  param([string[]]$Prefix, $Native)
  $lines = @()
  $failed = $false
  foreach ($p in $Prefix) {
    # The filter's * matches only after the whole prefix, so "open-airplay:*" never matches "open-airplay-cast:...".
    $found = $Native.Enumerate("$p*")
    if ($found.Error -ne 0 -and $found.Error -ne $ERROR_NOT_FOUND) {
      $lines += "$p couldn't list the saved entries (error $($found.Error))"
      $failed = $true
      continue
    }
    $deleted = 0
    $left = 0
    foreach ($item in $found.Items) {
      if (-not $item.Target.StartsWith($p)) { continue }
      $err = $Native.Delete($item.Target, $item.Type)
      if ($err -eq 0 -or $err -eq $ERROR_NOT_FOUND) { $deleted++ } else { $left++ }
    }
    if ($left -gt 0) {
      $lines += "$p deleted $deleted, couldn't delete $left"
      $failed = $true
    } else {
      $lines += "$p deleted $deleted"
    }
  }
  return @{ Lines = $lines; Failed = $failed }
}

if ($MyInvocation.InvocationName -ne '.') {
  $result = Remove-SavedCredentials -Prefix $Prefix -Native (New-NativeCredentials)
  $result.Lines
  if ($result.Failed) { exit 1 }
  exit 0
}
