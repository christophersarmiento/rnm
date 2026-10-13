<#
.SYNOPSIS
Installs rnm on Windows.

.DESCRIPTION
Downloads a release archive from GitHub, checks it against the release's
SHA256SUMS, puts rnm.exe in %LOCALAPPDATA%\rnm\bin, adds that folder to your
user Path, and adds rnm to your PowerShell profiles (Windows PowerShell 5.1
and PowerShell 7).

  irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1 | iex

With options:

  & ([scriptblock]::Create((irm https://raw.githubusercontent.com/christophersarmiento/rnm/main/install.ps1))) -Version v0.1.0 -SkipShell

The script is ASCII only, so Windows PowerShell 5.1 reads it correctly.

.PARAMETER Version
The release to install, for example v0.1.0. Default: the latest release.

.PARAMETER Nightly
Install the newest nightly build of the dev branch.

.PARAMETER InstallDir
Where to put rnm.exe. Default: %LOCALAPPDATA%\rnm\bin.

.PARAMETER SkipShell
Do not change your user Path or PowerShell profiles.

.PARAMETER UseOnCd
Set up rnm to switch versions on every directory change.
#>
[CmdletBinding()]
param(
  [string]$Version = $(if ($env:RNM_INSTALL_VERSION) { $env:RNM_INSTALL_VERSION } else { 'latest' }),
  [switch]$Nightly,
  [string]$InstallDir = $(if ($env:RNM_INSTALL_DIR) { $env:RNM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'rnm\bin' }),
  [switch]$SkipShell,
  [switch]$UseOnCd
)

Set-StrictMode -Version 2
$ErrorActionPreference = 'Stop'
# Invoke-WebRequest's progress bar makes downloads several times slower in 5.1.
$ProgressPreference = 'SilentlyContinue'

$Repo = 'christophersarmiento/rnm'
$Marker = '# rnm'

function Fail([string]$Message) {
  Write-Host "error: $Message" -ForegroundColor Red
  throw $Message
}

# The Rust target of the release archive. The native architecture comes from
# the registry, so an x64 PowerShell on an Arm64 PC still gets the Arm64 build.
function Get-Target {
  $native = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE
  switch ($native) {
    'AMD64' { return 'x86_64-pc-windows-msvc' }
    'ARM64' { return 'aarch64-pc-windows-msvc' }
    default { Fail "rnm has no prebuilt binary for $native Windows" }
  }
}

function Get-BaseUrl {
  if ($env:RNM_INSTALL_BASE_URL) { return $env:RNM_INSTALL_BASE_URL.TrimEnd('/', '\') }
  if ($Nightly) { return "https://github.com/$Repo/releases/download/nightly" }
  if ($Version -eq 'latest') { return "https://github.com/$Repo/releases/latest/download" }
  return "https://github.com/$Repo/releases/download/$Version"
}

# Copies a URL, or a file when the base is a local directory, to $Destination.
function Get-File([string]$Source, [string]$Destination) {
  if ($Source -match '^https?://') {
    Invoke-WebRequest -UseBasicParsing -Uri $Source -OutFile $Destination
  } else {
    Copy-Item -LiteralPath $Source -Destination $Destination
  }
}

function Install-Binary {
  # Windows PowerShell 5.1 may not offer TLS 1.2 by default.
  [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

  $archive = "rnm-$(Get-Target).zip"
  $base = Get-BaseUrl
  $temp = Join-Path ([IO.Path]::GetTempPath()) ("rnm-install-" + [Guid]::NewGuid())
  New-Item -ItemType Directory -Path $temp | Out-Null
  try {
    Write-Host "Downloading $base/$archive"
    try {
      Get-File "$base/$archive" (Join-Path $temp $archive)
      Get-File "$base/SHA256SUMS" (Join-Path $temp 'SHA256SUMS')
    } catch {
      Fail "download failed ($($_.Exception.Message)); check that the release exists at https://github.com/$Repo/releases"
    }

    $expected = $null
    foreach ($line in Get-Content -LiteralPath (Join-Path $temp 'SHA256SUMS')) {
      $fields = $line -split '\s+'
      if ($fields.Count -ge 2 -and $fields[1] -eq $archive) { $expected = $fields[0].ToLowerInvariant() }
    }
    if (-not $expected) { Fail "SHA256SUMS lists no checksum for $archive" }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $temp $archive)).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
      Fail "checksum mismatch for ${archive}: expected $expected, got $actual; the download is corrupt or was tampered with"
    }
    Write-Host 'Checksum verified.'

    Expand-Archive -LiteralPath (Join-Path $temp $archive) -DestinationPath (Join-Path $temp 'unpacked') -Force
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $exe = Join-Path $InstallDir 'rnm.exe'
    # A running rnm.exe cannot be overwritten, but it can be renamed.
    if (Test-Path -LiteralPath $exe) {
      Move-Item -LiteralPath $exe -Destination "$exe.old" -Force
    }
    Copy-Item -LiteralPath (Join-Path $temp 'unpacked\rnm.exe') -Destination $exe
    Remove-Item -LiteralPath "$exe.old" -Force -ErrorAction SilentlyContinue
    # Tells `rnm self-update` that this copy may update itself, and from which
    # channel.
    $channel = 'stable'
    if ($Nightly -or $Version -eq 'nightly') { $channel = 'nightly' }
    $marker = '{"source":"install.ps1","channel":"' + $channel + '"}'
    [IO.File]::WriteAllText((Join-Path $InstallDir 'install.json'), $marker, (New-Object Text.UTF8Encoding $false))
    Write-Host "Installed $(& $exe --version) to $exe"
  } finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
  }
}

function Test-SamePath([string]$A, [string]$B) {
  return $A.Trim().TrimEnd('\', '/') -ieq $B.Trim().TrimEnd('\', '/')
}

# Adds $InstallDir to the user Path in the registry. The value is read and
# written raw (REG_EXPAND_SZ), so entries such as %USERPROFILE%\bin survive.
function Add-ToUserPath {
  $key = Get-Item -LiteralPath 'HKCU:\Environment'
  $current = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
  $entries = @($current -split ';' | Where-Object { $_ -ne '' })
  $present = $false
  foreach ($entry in $entries) { if (Test-SamePath $entry $InstallDir) { $present = $true } }
  if (-not $present) {
    $updated = (@($InstallDir) + $entries) -join ';'
    Set-ItemProperty -LiteralPath 'HKCU:\Environment' -Name 'Path' -Value $updated -Type ExpandString
    # Setting any user variable through .NET broadcasts WM_SETTINGCHANGE, so
    # programs started from now on see the new Path.
    [Environment]::SetEnvironmentVariable('RNM_INSTALLER', '1', 'User')
    [Environment]::SetEnvironmentVariable('RNM_INSTALLER', $null, 'User')
    Write-Host "Added $InstallDir to your user Path."
  }
  $sessionHasIt = $false
  foreach ($entry in ($env:Path -split ';')) { if ($entry -and (Test-SamePath $entry $InstallDir)) { $sessionHasIt = $true } }
  if (-not $sessionHasIt) { $env:Path = "$InstallDir;$env:Path" }
}

# Adds rnm to the all-hosts profile of Windows PowerShell 5.1 and of
# PowerShell 7, whichever runs this script.
function Add-ToProfiles {
  $documents = [Environment]::GetFolderPath('MyDocuments')
  $flags = ''
  if ($UseOnCd) { $flags = ' --use-on-cd' }
  $block = @(
    $Marker,
    'if (Get-Command rnm -ErrorAction SilentlyContinue) {',
    "  rnm env --shell powershell$flags | Out-String | Invoke-Expression",
    '}'
  ) -join [Environment]::NewLine
  foreach ($edition in @('WindowsPowerShell', 'PowerShell')) {
    $profilePath = Join-Path $documents "$edition\profile.ps1"
    $existing = ''
    if (Test-Path -LiteralPath $profilePath) { $existing = [IO.File]::ReadAllText($profilePath) }
    if ($existing -match 'rnm env') {
      Write-Host "$profilePath already sets up rnm; left unchanged."
      continue
    }
    New-Item -ItemType Directory -Path (Split-Path $profilePath) -Force | Out-Null
    $text = [Environment]::NewLine + $block + [Environment]::NewLine
    [IO.File]::AppendAllText($profilePath, $text, (New-Object Text.UTF8Encoding $false))
    Write-Host "Added rnm to $profilePath"
  }
  $policy = Get-ExecutionPolicy
  if ($policy -eq 'Restricted' -or $policy -eq 'AllSigned') {
    Write-Host ''
    Write-Host "Your execution policy ($policy) stops PowerShell from running profiles. Run:" -ForegroundColor Yellow
    Write-Host '  Set-ExecutionPolicy -Scope CurrentUser RemoteSigned' -ForegroundColor Yellow
  }
}

Install-Binary
if ($SkipShell) {
  Write-Host "Skipped the Path and profile setup (-SkipShell). See https://github.com/$Repo/blob/main/docs/shell-setup.md"
} else {
  Add-ToUserPath
  Add-ToProfiles
}
Write-Host ''
Write-Host 'Open a new terminal, then run:'
Write-Host '  rnm install --lts'
Write-Host '  rnm doctor'
Write-Host ''
Write-Host 'Update rnm later with: rnm self-update'
