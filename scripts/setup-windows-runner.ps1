<#
Set up a house Windows machine as the Nebo Windows release runner.

Run in an elevated PowerShell on the laptop, once:

  Set-ExecutionPolicy -Scope Process Bypass
  .\setup-windows-runner.ps1 -Token <registration token>

Get the token from a machine with the gh CLI (org admin):

  gh api -X POST orgs/NeboLoop/actions/runners/registration-token -q .token

It installs what release.yml's build-windows job needs on the host (the job
installs Rust, Node and the Tauri CLI itself; Tauri fetches NSIS), registers
the runner at the org level with label `stadium-win`, and installs it as a
Windows service so it survives reboots. Then switch `build-windows` and
`sign-windows` in release.yml to `runs-on: [self-hosted, Windows, stadium-win]`
and tighten scripts/check-release-runners.py to require it.

Keep the laptop plugged in with sleep disabled (Settings > Power: Never).
#>
param(
  [Parameter(Mandatory = $true)] [string] $Token,
  [string] $Name = "stadium-win-1",
  [string] $Dir = "C:\actions-runner"
)
$ErrorActionPreference = "Stop"

# Toolchain the job assumes on the host.
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
  Set-ExecutionPolicy Bypass -Scope Process -Force
  [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
  Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
}
choco install -y --no-progress git dotnet-8.0-runtime protoc
# MSVC linker + Windows SDK for the Rust msvc target.
choco install -y --no-progress visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"

# Runner.
$ver = (Invoke-RestMethod https://api.github.com/repos/actions/runner/releases/latest).tag_name.TrimStart('v')
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
Set-Location $Dir
if (-not (Test-Path .\run.cmd)) {
  Invoke-WebRequest -Uri "https://github.com/actions/runner/releases/download/v$ver/actions-runner-win-x64-$ver.zip" -OutFile runner.zip
  Expand-Archive runner.zip -DestinationPath . -Force
  Remove-Item runner.zip
}
.\config.cmd --unattended --url https://github.com/NeboLoop --token $Token --name $Name --labels stadium-win --work _work --replace --runasservice
Write-Host "Runner $Name registered with label stadium-win and installed as a service."
