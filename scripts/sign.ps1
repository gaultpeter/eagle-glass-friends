# Signs Eagle Glass release binaries and verifies the result.
#
# One entry point for local runs and CI. It auto-selects the first configured
# signing method, and is a no-op (exit 0) when nothing is configured so a
# release job can call it unconditionally.
#
#   powershell -ExecutionPolicy Bypass -File scripts\sign.ps1 target\release\eglass.exe
#
# Methods, in priority order:
#
#   azure       Trusted Signing. Requires TS_ENDPOINT, TS_ACCOUNT_NAME,
#               TS_CERT_PROFILE and the Azure credential env vars, plus
#               `trusted-signing-cli` on PATH.
#   pfx-base64  WINDOWS_CERT_PFX_BASE64 + WINDOWS_CERT_PASSWORD (CI secret form).
#   pfx         WINDOWS_CERT_PFX + WINDOWS_CERT_PASSWORD.
#   thumbprint  WINDOWS_CERT_THUMBPRINT, certificate installed in CurrentUser\My.
#
# See docs/SIGNING.md for how to obtain any of these.
#
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string[]]$Path,

    [string]$TimestampUrl = 'http://timestamp.digicert.com',

    [string]$Description = 'Eagle Glass',

    [string]$Url = 'https://github.com/gaultpeter/eagle-glass-friends',

    # Fail instead of skipping when no signing method is configured. The
    # release workflow sets this once signing is meant to be mandatory.
    [switch]$RequireSignature
)

$ErrorActionPreference = 'Stop'

# RFC3161 timestamp endpoints, tried in order. A signature without a
# timestamp stops validating when the certificate expires.
$TimestampFallbacks = @(
    $TimestampUrl,
    'http://timestamp.acs.microsoft.com',
    'http://timestamp.sectigo.com'
) | Select-Object -Unique

function Find-SignTool {
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    $roots = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\bin')
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

    foreach ($root in $roots) {
        $candidate = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            ForEach-Object {
                $numeric = ($_.Name -replace '[^0-9.]', '').TrimEnd('.')
                $parsed = $null
                if ($numeric -and [version]::TryParse($numeric, [ref]$parsed)) {
                    [pscustomobject]@{ Version = $parsed; Exe = (Join-Path $_.FullName 'x64\signtool.exe') }
                }
            } |
            Where-Object { Test-Path -LiteralPath $_.Exe } |
            Sort-Object Version -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.Exe }
    }
    return $null
}

function Get-SigningMode {
    if ($env:TS_ENDPOINT -and $env:TS_ACCOUNT_NAME -and $env:TS_CERT_PROFILE) { return 'azure' }
    if ($env:WINDOWS_CERT_PFX_BASE64) { return 'pfx-base64' }
    if ($env:WINDOWS_CERT_PFX) { return 'pfx' }
    if ($env:WINDOWS_CERT_THUMBPRINT) { return 'thumbprint' }
    return 'none'
}

function Invoke-Sign {
    param([string]$File, [string]$SignTool, [string]$Mode, [string[]]$Common)

    if ($Mode -eq 'azure') {
        $cli = Get-Command trusted-signing-cli -ErrorAction SilentlyContinue
        if (-not $cli) {
            throw "Azure Trusted Signing selected but 'trusted-signing-cli' is not on PATH. Install it with: dotnet tool install --global trusted-signing-cli"
        }
        & $cli.Source `
            -e $env:TS_ENDPOINT `
            -a $env:TS_ACCOUNT_NAME `
            -c $env:TS_CERT_PROFILE `
            -d $Description `
            -u $Url `
            $File
        if ($LASTEXITCODE -ne 0) { throw "trusted-signing-cli failed for $File (exit $LASTEXITCODE)" }
        return
    }

    $signArgs = @('sign', '/fd', 'SHA256', '/td', 'SHA256', '/d', $Description, '/du', $Url) + $Common + @($File)

    $lastError = $null
    foreach ($ts in $TimestampFallbacks) {
        Write-Host "  timestamp: $ts"
        & $SignTool @signArgs /tr $ts
        if ($LASTEXITCODE -eq 0) { return }
        $lastError = "signtool exited $LASTEXITCODE"
        Write-Warning "  timestamping via $ts failed ($lastError); trying next"
    }
    throw "signing failed for $File ($lastError)"
}

$mode = Get-SigningMode
$files = @($Path | ForEach-Object { (Resolve-Path -LiteralPath $_).Path })

foreach ($f in $files) {
    if (-not (Test-Path -LiteralPath $f -PathType Leaf)) { throw "not a file: $f" }
}

if ($mode -eq 'none') {
    Write-Host ''
    Write-Host 'No code-signing credentials configured - producing UNSIGNED binaries.' -ForegroundColor Yellow
    Write-Host 'Set one of the following to sign (see docs/SIGNING.md):'
    Write-Host '  Azure Trusted Signing : TS_ENDPOINT, TS_ACCOUNT_NAME, TS_CERT_PROFILE'
    Write-Host '  PFX (base64)          : WINDOWS_CERT_PFX_BASE64, WINDOWS_CERT_PASSWORD'
    Write-Host '  PFX (file)            : WINDOWS_CERT_PFX, WINDOWS_CERT_PASSWORD'
    Write-Host '  Installed certificate : WINDOWS_CERT_THUMBPRINT'
    Write-Host ''
    if ($RequireSignature) {
        throw 'signing was required but no credentials are configured'
    }
    return
}

Write-Host "Signing $($files.Count) file(s) using mode '$mode'"

$signTool = Find-SignTool
if (-not $signTool -and $mode -ne 'azure') {
    throw 'signtool.exe not found. Install the Windows SDK signing tools (or the "Windows SDK Signing Tools for Desktop Apps" component).'
}
if ($signTool) { Write-Host "  signtool: $signTool" }

$common = @()
$tempPfx = $null
switch ($mode) {
    'pfx' {
        $common = @('/f', $env:WINDOWS_CERT_PFX, '/p', $env:WINDOWS_CERT_PASSWORD, '/a')
    }
    'pfx-base64' {
        $tempPfx = Join-Path ([System.IO.Path]::GetTempPath()) ("eglass-sign-{0}.pfx" -f ([guid]::NewGuid()))
        [System.IO.File]::WriteAllBytes($tempPfx, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX_BASE64))
        $common = @('/f', $tempPfx, '/p', $env:WINDOWS_CERT_PASSWORD, '/a')
    }
    'thumbprint' {
        $common = @('/sha1', ($env:WINDOWS_CERT_THUMBPRINT -replace '\s', ''), '/a')
    }
}

try {
    foreach ($f in $files) {
        Write-Host "  signing $f"
        Invoke-Sign -File $f -SignTool $signTool -Mode $mode -Common $common
    }
}
finally {
    if ($tempPfx -and (Test-Path -LiteralPath $tempPfx)) {
        Remove-Item -LiteralPath $tempPfx -Force -ErrorAction SilentlyContinue
    }
}

# Verification is part of signing: an unverifiable signature is a bug.
$failed = @()
foreach ($f in $files) {
    $sig = Get-AuthenticodeSignature -LiteralPath $f
    Write-Host ("  {0}: {1}" -f (Split-Path -Leaf $f), $sig.Status)
    if ($sig.SignerCertificate) {
        Write-Host ("    signer : {0}" -f $sig.SignerCertificate.Subject)
        Write-Host ("    expires: {0:yyyy-MM-dd}" -f $sig.SignerCertificate.NotAfter)
    }
    Write-Host ("    timestamp: {0}" -f $(if ($sig.TimeStamperCertificate) { $sig.TimeStamperCertificate.Subject } else { 'NONE' }))

    if ($sig.Status -ne 'Valid') { $failed += "$f ($($sig.Status): $($sig.StatusMessage))" }

    if ($signTool) {
        & $signTool verify /pa /all $f | Out-Null
        if ($LASTEXITCODE -ne 0) { $failed += "$f (signtool verify /pa failed)" }
    }
}

if ($failed.Count -gt 0) {
    throw ("signature verification failed:`n  " + ($failed -join "`n  "))
}

Write-Host 'Signature verified.' -ForegroundColor Green
