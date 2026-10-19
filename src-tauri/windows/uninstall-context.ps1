# Tells the uninstaller, when a setup started it in place (from the install folder, with _?=), whether
# that setup is replacing this version ("upgrade": a newer or older version's "uninstall before
# installing") or is the same version's explicit Uninstall choice ("remove"). Only a removal may offer
# "Also delete my settings and saved pairings". Anything unknown counts as an upgrade, so data is never
# offered for deletion mid-upgrade.
# Tests dot-source this file (tests/installer/uninstall-context.tests.ps1).
#   uninstall-context.ps1 -UninstallerPid 1234 -Version 0.1.1   ->  prints upgrade or remove
[CmdletBinding(PositionalBinding = $false)]
param([int]$UninstallerPid, [string]$Version)

# "0.1.1", "0.1.1.0" and "0.1.1-beta" compare as the same release; $null when it isn't a version.
function ConvertTo-Release([string]$Text) {
  try {
    $v = [version]($Text -replace '[-+].*$', '')
    return [version]::new($v.Major, $v.Minor, [Math]::Max($v.Build, 0), [Math]::Max($v.Revision, 0))
  } catch {
    return $null
  }
}

function Get-UninstallKind {
  param([string]$OwnVersion, [string]$LauncherVersion)
  $own = ConvertTo-Release $OwnVersion
  $launcher = ConvertTo-Release $LauncherVersion
  if ($own -and $launcher -and $own -eq $launcher) { return 'remove' }
  return 'upgrade'
}

# The product version of the program that started process $ProcessId (the setup), or '' if unknown.
function Get-LauncherVersion {
  param([int]$ProcessId)
  try {
    $parent = (Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId").ParentProcessId
    if (-not $parent) { return '' }
    $path = (Get-CimInstance Win32_Process -Filter "ProcessId=$parent").ExecutablePath
    if (-not $path) { return '' }
    return [string](Get-Item $path).VersionInfo.ProductVersion
  } catch {
    return ''
  }
}

if ($MyInvocation.InvocationName -ne '.') {
  Get-UninstallKind -OwnVersion $Version -LauncherVersion (Get-LauncherVersion -ProcessId $UninstallerPid)
}
