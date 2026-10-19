# Tests for the uninstaller's upgrade check (src-tauri/windows/uninstall-context.ps1).
# Run: npm run test:installer (Windows only).
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\..\..\src-tauri\windows\uninstall-context.ps1"

$script:failures = 0
function Test([string]$Name, [scriptblock]$Body) {
  try { & $Body; "ok   $Name" } catch { $script:failures++; "FAIL $Name`n     $($_.Exception.Message)" }
}
function Assert-Equal($Expected, $Actual, [string]$What) {
  if ("$Expected" -ne "$Actual") { throw "$What`: expected '$Expected', got '$Actual'" }
}

Test "the same version's setup choosing Uninstall is a removal: the cleanup box stays" {
  Assert-Equal 'remove' (Get-UninstallKind -OwnVersion '0.1.1' -LauncherVersion '0.1.1') 'same'
  Assert-Equal 'remove' (Get-UninstallKind -OwnVersion '0.1.1' -LauncherVersion '0.1.1.0') 'same, with a build part'
}

Test "a newer or older version's setup replacing this one is an upgrade: no box" {
  Assert-Equal 'upgrade' (Get-UninstallKind -OwnVersion '0.1.0' -LauncherVersion '0.1.1') 'newer'
  Assert-Equal 'upgrade' (Get-UninstallKind -OwnVersion '0.2.0' -LauncherVersion '0.1.9') 'older'
}

Test "when the launcher's version can't be read, it counts as an upgrade (never offer to delete data mid-upgrade)" {
  Assert-Equal 'upgrade' (Get-UninstallKind -OwnVersion '0.1.1' -LauncherVersion $null) 'unknown'
  Assert-Equal 'upgrade' (Get-UninstallKind -OwnVersion '0.1.1' -LauncherVersion 'not a version') 'unreadable'
}

Test "the launcher's version is read from the parent process's file" {
  # This test's own process was started by a process whose file has a version; the script's
  # lookup must find the same one PowerShell reports for the parent.
  $parent = (Get-CimInstance Win32_Process -Filter "ProcessId=$PID").ParentProcessId
  $expected = (Get-Item (Get-CimInstance Win32_Process -Filter "ProcessId=$parent").ExecutablePath).VersionInfo.ProductVersion
  Assert-Equal $expected (Get-LauncherVersion -ProcessId $PID) 'parent version'
  Assert-Equal '' (Get-LauncherVersion -ProcessId 999999999) 'no such process'
}

if ($script:failures) { "$script:failures failed"; exit 1 }
"all passed"
