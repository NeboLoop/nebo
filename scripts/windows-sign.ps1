# Signs one file with Azure Trusted Signing: Tauri's `bundle.windows.signCommand`
# in the release build, so every file the bundler packs into the NSIS installer
# (nebo.exe, the sidecars, the uninstaller, the plugin DLLs) is signed BEFORE
# it is packed, and the installer itself after.
#
#   pwsh -NoProfile -File scripts/windows-sign.ps1 <file>
#
# The same signer, account and profile as azure/trusted-signing-action in
# release.yml: that action's earlier step in the job installs the
# TrustedSigning module this calls. Azure credentials come from the
# environment (AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET), the
# account from AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and
# AZURE_SIGNING_PROFILE. A file that does not end up signed fails the build.
param([Parameter(Mandatory = $true)][string]$File)

$ErrorActionPreference = 'Stop'

foreach ($name in 'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_SIGNING_PROFILE') {
  if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))) {
    throw "windows-sign: $name is not set"
  }
}

Invoke-TrustedSigning `
  -Endpoint $env:AZURE_SIGNING_ENDPOINT `
  -CodeSigningAccountName $env:AZURE_SIGNING_ACCOUNT `
  -CertificateProfileName $env:AZURE_SIGNING_PROFILE `
  -Files $File `
  -FileDigest SHA256 `
  -TimestampRfc3161 'http://timestamp.acs.microsoft.com' `
  -TimestampDigest SHA256

$sig = Get-AuthenticodeSignature -FilePath $File
if ($sig.Status -ne 'Valid') {
  throw "windows-sign: $File is not signed after signing ($($sig.Status): $($sig.StatusMessage))"
}
Write-Host "windows-sign: signed $File ($($sig.SignerCertificate.Subject))"
