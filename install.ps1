# Snapreq installer for Windows. Installs for the current user only, no admin needed.
#   irm https://raw.githubusercontent.com/luxend0/snapreq/main/install.ps1 | iex
$ErrorActionPreference = 'Stop'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\Snapreq'
$zip = Join-Path $env:TEMP 'snapreq.zip'
Write-Host 'Downloading Snapreq ...'
Invoke-WebRequest 'https://github.com/luxend0/snapreq/releases/latest/download/snapreq-windows-x86_64.zip' -OutFile $zip
New-Item -ItemType Directory -Force $dir | Out-Null
Expand-Archive $zip -DestinationPath $dir -Force
Remove-Item $zip

# Start menu entry
$lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'Snapreq.lnk'
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$s.TargetPath = Join-Path $dir 'snapreq.exe'
$s.WorkingDirectory = [Environment]::GetFolderPath('MyDocuments')
$s.Save()

# PATH for the terminal command
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($path -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', "$path;$dir", 'User')
}
Write-Host "Installed to $dir. Open a new terminal to use the 'snapreq' command."
