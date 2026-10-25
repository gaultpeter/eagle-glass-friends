# Pre-flight checks for a release binary, plus hash generation.
#
# Confirms the packaging metadata that SmartScreen, Defender and the shell
# actually read is present, optionally requires a valid Authenticode
# signature, and writes a SHA-256 sums file.
#
#   powershell -ExecutionPolicy Bypass -File scripts\verify-release.ps1 `
#       -Path target\release\eglass.exe -SumsPath dist\SHA256SUMS.txt
#
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Path,

    [switch]$RequireSignature,

    [string]$SumsPath
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$file = (Resolve-Path -LiteralPath $Path).Path
if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "not a file: $file" }

$problems = New-Object System.Collections.Generic.List[string]
$info = Get-Item -LiteralPath $file
$bytes = [System.IO.File]::ReadAllBytes($file)

Write-Host ("file    : {0}" -f $file)
Write-Host ("size    : {0:N0} bytes" -f $info.Length)

# 1. Version resources ------------------------------------------------------
$vi = $info.VersionInfo
$required = 'FileDescription', 'ProductName', 'CompanyName', 'FileVersion', 'OriginalFilename'
foreach ($field in $required) {
    $value = $vi.$field
    if ([string]::IsNullOrWhiteSpace($value)) {
        $problems.Add("missing version resource: $field")
    } else {
        Write-Host ("{0,-16}: {1}" -f $field, $value)
    }
}

# 2. Execution manifest -----------------------------------------------------
$text = [System.Text.Encoding]::UTF8.GetString($bytes)
$manifestOk = $true
foreach ($needle in 'requestedExecutionLevel', 'asInvoker') {
    if (-not $text.Contains($needle)) {
        $problems.Add("manifest is missing '$needle'")
        $manifestOk = $false
    }
}
if ($manifestOk) { Write-Host 'manifest        : asInvoker' }

# 3. Icon -------------------------------------------------------------------
try {
    $icon = [System.Drawing.Icon]::ExtractAssociatedIcon($file)
    if ($icon) {
        Write-Host ("icon            : {0}x{1}" -f $icon.Width, $icon.Height)
        $icon.Dispose()
    } else {
        $problems.Add('no icon resource found')
    }
} catch {
    $problems.Add("icon resource could not be read: $($_.Exception.Message)")
}

# 4. Signature --------------------------------------------------------------
$sig = Get-AuthenticodeSignature -LiteralPath $file
Write-Host ("signature       : {0}" -f $sig.Status)
if ($sig.SignerCertificate) {
    Write-Host ("  signer        : {0}" -f $sig.SignerCertificate.Subject)
    Write-Host ("  valid until   : {0:yyyy-MM-dd}" -f $sig.SignerCertificate.NotAfter)
    Write-Host ("  timestamped   : {0}" -f ([bool]$sig.TimeStamperCertificate))
}
if ($RequireSignature -and $sig.Status -ne 'Valid') {
    $problems.Add("signature is $($sig.Status): $($sig.StatusMessage)")
}
if (-not $RequireSignature -and $sig.Status -ne 'Valid') {
    Write-Warning 'binary is not signed; SmartScreen will show "Unknown publisher"'
}

# 5. Hashes -----------------------------------------------------------------
$hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host ("sha256          : {0}" -f $hash)

if ($SumsPath) {
    $dir = Split-Path -Parent $SumsPath
    if ($dir -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    $line = "{0}  {1}" -f $hash, (Split-Path -Leaf $file)
    # Set-Content honours the PowerShell location, unlike System.IO.
    Set-Content -LiteralPath $SumsPath -Value $line -Encoding Ascii
    Write-Host ("sums            : {0}" -f (Get-Item -LiteralPath $SumsPath).FullName)
}

if ($problems.Count -gt 0) {
    Write-Host ''
    foreach ($p in $problems) { Write-Host "FAIL: $p" -ForegroundColor Red }
    throw "$($problems.Count) release check(s) failed"
}

Write-Host ''
Write-Host 'Release checks passed.' -ForegroundColor Green
