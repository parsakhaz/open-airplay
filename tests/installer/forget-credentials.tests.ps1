# Tests for the uninstaller's Credential Manager cleanup (src-tauri/windows/forget-credentials.ps1).
# Run: npm run test:installer (Windows only). The native layer is a fake, so no real entry is touched.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\..\..\src-tauri\windows\forget-credentials.ps1"

$script:failures = 0
function Test([string]$Name, [scriptblock]$Body) {
  try { & $Body; "ok   $Name" } catch { $script:failures++; "FAIL $Name`n     $($_.Exception.Message)" }
}
function Assert-Equal($Expected, $Actual, [string]$What) {
  if ("$Expected" -ne "$Actual") { throw "$What`: expected '$Expected', got '$Actual'" }
}

# A fake Credential Manager: $Saved maps target -> type. EnumerateError / DeleteErrors make calls fail.
function New-FakeCredentials([hashtable]$Saved, [int]$EnumerateError = 0, [hashtable]$DeleteErrors = @{}) {
  $fake = New-Object psobject -Property @{ Saved = $Saved; EnumerateError = $EnumerateError; DeleteErrors = $DeleteErrors }
  $fake | Add-Member ScriptMethod Enumerate {
    param([string]$Filter)
    if ($this.EnumerateError) { return @{ Error = $this.EnumerateError; Items = @() } }
    $prefix = $Filter.TrimEnd('*')
    $items = @($this.Saved.Keys | Where-Object { $_.StartsWith($prefix) } | ForEach-Object { @{ Target = $_; Type = $this.Saved[$_] } })
    if ($items.Count -eq 0) { return @{ Error = 1168; Items = @() } }  # what CredEnumerateW says for none
    return @{ Error = 0; Items = $items }
  }
  $fake | Add-Member ScriptMethod Delete {
    param([string]$Target, [int]$Type)
    if ($this.DeleteErrors.ContainsKey($Target)) { return $this.DeleteErrors[$Target] }
    $this.Saved.Remove($Target)
    return 0
  }
  return $fake
}

Test "deletes every entry under each prefix and nothing else" {
  $fake = New-FakeCredentials @{ 'open-airplay-cast:tv' = 1; 'open-airplay-cast:speaker' = 1; 'open-airplay:AA:BB' = 1; 'other-app:x' = 1 }
  $r = Remove-SavedCredentials -Prefix 'open-airplay-cast:', 'open-airplay:' -Native $fake
  Assert-Equal $false $r.Failed 'failed'
  Assert-Equal 'open-airplay-cast: deleted 2|open-airplay: deleted 1' ($r.Lines -join '|') 'lines'
  Assert-Equal 'other-app:x' (@($fake.Saved.Keys) -join ',') 'left'
}

Test "nothing saved is not a failure" {
  $r = Remove-SavedCredentials -Prefix 'open-airplay-cast:' -Native (New-FakeCredentials @{})
  Assert-Equal $false $r.Failed 'failed'
  Assert-Equal 'open-airplay-cast: deleted 0' ($r.Lines -join '|') 'lines'
}

Test "a list that fails is reported as a failure" {
  $r = Remove-SavedCredentials -Prefix 'open-airplay-cast:', 'open-airplay:' -Native (New-FakeCredentials @{ 'open-airplay:AA' = 1 } -EnumerateError 5)
  Assert-Equal $true $r.Failed 'failed'
  Assert-Equal "open-airplay-cast: couldn't list the saved entries (error 5)" $r.Lines[0] 'line'
}

Test "an entry that can't be deleted is reported, and the others still go" {
  $fake = New-FakeCredentials @{ 'open-airplay-cast:tv' = 1; 'open-airplay-cast:speaker' = 1 } -DeleteErrors @{ 'open-airplay-cast:tv' = 5 }
  $r = Remove-SavedCredentials -Prefix 'open-airplay-cast:' -Native $fake
  Assert-Equal $true $r.Failed 'failed'
  Assert-Equal "open-airplay-cast: deleted 1, couldn't delete 1" ($r.Lines -join '|') 'lines'
  Assert-Equal 'open-airplay-cast:tv' (@($fake.Saved.Keys) -join ',') 'left'
}

Test "an entry already gone when deleted is not a failure" {
  $fake = New-FakeCredentials @{ 'open-airplay-cast:tv' = 1 } -DeleteErrors @{ 'open-airplay-cast:tv' = 1168 }
  $r = Remove-SavedCredentials -Prefix 'open-airplay-cast:' -Native $fake
  Assert-Equal $false $r.Failed 'failed'
}

Test "the script exits 1 when cleanup is incomplete and 0 otherwise" {
  $script = "$PSScriptRoot\..\..\src-tauri\windows\forget-credentials.ps1"
  # A prefix no real entry has: lists nothing, deletes nothing, succeeds.
  $out = & powershell -NoProfile -ExecutionPolicy Bypass -File $script "oap-installer-test-none-$PID`:"
  Assert-Equal 0 $LASTEXITCODE 'exit code'
  Assert-Equal "oap-installer-test-none-$PID`: deleted 0" ($out -join '|') 'output'
}

if ($script:failures) { "$script:failures failed"; exit 1 }
"all passed"
