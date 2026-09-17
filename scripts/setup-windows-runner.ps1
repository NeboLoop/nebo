<#
.SYNOPSIS
  Turn a Windows machine into the house release runner for Nebo.

.DESCRIPTION
  Run once, as Administrator, on the Windows box that will build the Windows
  release (see CLAUDE.md "Release builds" and docs/sme/RELEASE.md). It installs
  the toolchain the release workflow's build-windows and sign-windows jobs
  expect, registers a GitHub Actions runner for the NeboLoop org as a Windows
  service, and enables OpenSSH Server so the box can be serviced remotely.

  Everything the build needs lives under C:\rust (rustup + cargo) and
  C:\actions-runner, readable by the account the service runs as.

.PARAMETER Token
  A runner registration token for the org (expires after an hour):
    gh api -X POST orgs/NeboLoop/actions/runners/registration-token -q .token

.PARAMETER Name
  Runner name shown in GitHub (default: stadium-win-1).

.PARAMETER SshPublicKey
  A public key to authorize for Administrators over OpenSSH (optional).

.EXAMPLE
  Set-ExecutionPolicy Bypass -Scope Process -Force
  iwr https://raw.githubusercontent.com/NeboLoop/nebo/main/scripts/setup-windows-runner.ps1 -OutFile setup-windows-runner.ps1
  .\setup-windows-runner.ps1 -Token <token>

.NOTES
  The runner service runs as the account you are logged in as, and
  config.cmd only takes that account's password as a command-line argument
  (there is no other way to set a service logon through it). The script
  asks for it interactively and never stores it. A machine-wide Rust under
  C:\rust is what lets that account's cargo be found by the service.
#>
param(
  [Parameter(Mandatory = $true)] [string] $Token,
  [string] $Name = "stadium-win-1",
  [string] $SshPublicKey = "",
  [string] $RunnerVersion = "2.337.0",
  [string] $RustVersion = "1.95",
  [string] $TauriCliVersion = "2.11.4"
)
$ErrorActionPreference = "Stop"
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw "Run this from an Administrator PowerShell."
}
function Step($m) { Write-Host "`n==> $m" -ForegroundColor Cyan }

# ── Toolchain (machine-wide) ─────────────────────────────────────────────
Step "Chocolatey"
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
  iex ((New-Object Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
  $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
}

# A workflow's PowerShell steps are temp .ps1 files; a box whose policy is
# Restricted refuses them ("running scripts is disabled on this system").
# Hosted runners are Unrestricted; RemoteSigned is enough here. PowerShell 7
# (pwsh) is what the runner picks as the default shell when present, and the
# signing action wants it.
Step "Execution policy + PowerShell 7"
Set-ExecutionPolicy RemoteSigned -Scope LocalMachine -Force
choco install -y --no-progress powershell-core

Step "Git, Node, pnpm, protoc, .NET runtime (for the signing action), 7zip"
choco install -y --no-progress git nodejs-lts protoc dotnet-8.0-runtime 7zip
$env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
npm install -g pnpm@10.33.2

Step "Visual Studio 2022 Build Tools with the C++ workload (this one takes a while)"
choco install -y --no-progress visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive --norestart"

Step "WebView2 runtime"
choco install -y --no-progress microsoft-edge-webview2-runtime

Step "Rust $RustVersion under C:\rust (readable by the runner service)"
[Environment]::SetEnvironmentVariable("RUSTUP_HOME", "C:\rust\rustup", "Machine")
[Environment]::SetEnvironmentVariable("CARGO_HOME",  "C:\rust\cargo",  "Machine")
$env:RUSTUP_HOME = "C:\rust\rustup"; $env:CARGO_HOME = "C:\rust\cargo"
New-Item -ItemType Directory -Force -Path C:\rust | Out-Null
iwr https://win.rustup.rs/x86_64 -OutFile C:\rust\rustup-init.exe
& C:\rust\rustup-init.exe -y --profile minimal --default-toolchain $RustVersion --no-modify-path
Remove-Item C:\rust\rustup-init.exe
$machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
if ($machinePath -notlike "*C:\rust\cargo\bin*") {
  [Environment]::SetEnvironmentVariable("Path", "$machinePath;C:\rust\cargo\bin", "Machine")
}
$env:Path = "C:\rust\cargo\bin;" + $env:Path
& C:\rust\cargo\bin\cargo.exe install tauri-cli --version $TauriCliVersion --locked

# ── OpenSSH Server, so the box can be serviced without anyone at it ─────────
Step "OpenSSH Server"
$cap = Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
# On a fresh install the capability is only staged ("InstallPending") until
# the next reboot, and the sshd service does not exist yet; everything else
# here is set up so that it starts on its own after that reboot.
if (Get-Service sshd -ErrorAction SilentlyContinue) {
  Set-Service -Name sshd -StartupType Automatic
  Start-Service sshd
} else {
  Write-Warning "OpenSSH Server is installed but needs a reboot before sshd exists (state: $($cap.RestartNeeded)). After the reboot: Set-Service sshd -StartupType Automatic; Start-Service sshd"
}
if (-not (Get-NetFirewallRule -Name "OpenSSH-Server-In-TCP" -ErrorAction SilentlyContinue)) {
  # Local network only: the box is serviced from the house LAN, never from the internet.
  New-NetFirewallRule -Name "OpenSSH-Server-In-TCP" -DisplayName "OpenSSH Server (sshd)" -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 -RemoteAddress LocalSubnet | Out-Null
}
# PowerShell as the SSH shell
New-ItemProperty -Path "HKLM:\SOFTWARE\OpenSSH" -Name DefaultShell -Value "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe" -PropertyType String -Force | Out-Null
if ($SshPublicKey) {
  $ak = "C:\ProgramData\ssh\administrators_authorized_keys"
  Add-Content -Path $ak -Value $SshPublicKey
  icacls $ak /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F" | Out-Null
}

# ── GitHub Actions runner as a service ─────────────────────────────────────
Step "GitHub Actions runner $RunnerVersion → C:\actions-runner"
New-Item -ItemType Directory -Force -Path C:\actions-runner | Out-Null
Set-Location C:\actions-runner
if (Test-Path .\config.cmd) { .\config.cmd remove --token $Token 2>$null }
iwr "https://github.com/actions/runner/releases/download/v$RunnerVersion/actions-runner-win-x64-$RunnerVersion.zip" -OutFile runner.zip
Expand-Archive -Force runner.zip . ; Remove-Item runner.zip
# The service runs as the account you are logged in as, so cargo, the VS
# toolchain and the signing action see a real user profile.
$account = "$env:USERDOMAIN\$env:USERNAME"
$password = Read-Host "Password for $account (the runner service runs as this account)" -AsSecureString
$plain = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR($password))
.\config.cmd --unattended --url https://github.com/NeboLoop --token $Token --name $Name `
  --labels stadium-win --work _work --replace --runasservice `
  --windowslogonaccount $account --windowslogonpassword $plain
$plain = $null

Step "Done"
Write-Host "Runner '$Name' registered with labels self-hosted, Windows, X64, stadium-win and running as a service."
Write-Host "SSH: ssh $env:USERNAME@$((Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike '127.*' -and $_.IPAddress -notlike '169.*' } | Select-Object -First 1).IPAddress)"
