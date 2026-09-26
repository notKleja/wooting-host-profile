$ErrorActionPreference = 'Stop'
$releaseDirectory = Join-Path $PSScriptRoot 'app'
$repositoryDirectory = Join-Path $PSScriptRoot '..\dist\windows'
$sourceDirectory = if (Test-Path -LiteralPath (Join-Path $releaseDirectory 'WootingHostProfile.WinUI.exe')) {
    $releaseDirectory
} else {
    $repositoryDirectory
}
$installDirectory = Join-Path $env:LOCALAPPDATA 'WootingHostProfile'
$executable = Join-Path $installDirectory 'WootingHostProfile.WinUI.exe'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcutPath = Join-Path $startMenu 'Wooting Switch.lnk'
$legacyShortcutPath = Join-Path $startMenu 'Wooting Host Profile.lnk'

if (-not (Test-Path -LiteralPath (Join-Path $sourceDirectory 'WootingHostProfile.WinUI.exe'))) {
    throw "WinUI application not found under $sourceDirectory"
}

New-Item -ItemType Directory -Path $installDirectory -Force | Out-Null
$runningProcesses = @(Get-Process -Name 'WootingHostProfile.WinUI','wooting-host-profile-agent' -ErrorAction SilentlyContinue)
if ($runningProcesses.Count -gt 0) {
    $runningProcesses | Stop-Process
    $runningProcesses | Wait-Process -Timeout 5 -ErrorAction Stop
}
Copy-Item -Path (Join-Path $sourceDirectory '*') -Destination $installDirectory -Recurse -Force

$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($shortcutPath)
$shortcut.TargetPath = $executable
$shortcut.WorkingDirectory = $installDirectory
$shortcut.Description = 'Configure the per-system Wooting profile'
$shortcut.Save()

if (Test-Path -LiteralPath $legacyShortcutPath) {
    Remove-Item -LiteralPath $legacyShortcutPath
}

Start-Process -FilePath $executable
Write-Host 'Installed and opened Wooting Switch.'
Write-Host 'Choose a profile and select Remember; check options apply immediately.'
