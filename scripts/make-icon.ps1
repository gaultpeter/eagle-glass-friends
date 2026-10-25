# Generates assets/eglass.ico and assets/icon-256.png for Eagle Glass.
# Deterministic, no external assets: draws the mark with System.Drawing and
# writes a multi-resolution ICO (DIB entries <=64px, PNG entries >=128px).
#
#   powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1
#
[CmdletBinding()]
param(
    [string]$OutDir
)

$ErrorActionPreference = 'Stop'
if (-not $OutDir) { $OutDir = Join-Path $PSScriptRoot '..\assets' }
Add-Type -AssemblyName System.Drawing

$AccentTop = [System.Drawing.Color]::FromArgb(255, 0xAA, 0xB7, 0xFF)
$AccentBot = [System.Drawing.Color]::FromArgb(255, 0x64, 0x74, 0xFF)
$BgTop     = [System.Drawing.Color]::FromArgb(255, 0x1C, 0x22, 0x33)
$BgBot     = [System.Drawing.Color]::FromArgb(255, 0x0A, 0x0C, 0x12)
$Edge      = [System.Drawing.Color]::FromArgb(110, 0x7C, 0x88, 0xB0)

function New-RoundedRectPath {
    param([System.Drawing.RectangleF]$Rect, [double]$Radius)
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $Radius * 2
    $p.AddArc($Rect.X, $Rect.Y, $d, $d, 180, 90)
    $p.AddArc($Rect.Right - $d, $Rect.Y, $d, $d, 270, 90)
    $p.AddArc($Rect.Right - $d, $Rect.Bottom - $d, $d, $d, 0, 90)
    $p.AddArc($Rect.X, $Rect.Bottom - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# Renders the mark at an arbitrary size. Coordinates are authored in a 1024
# grid and scaled, so every output size is a true vector render.
function New-Mark {
    param([int]$Size)

    $bmp = New-Object System.Drawing.Bitmap($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode   = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)

    $s = $Size / 1024.0

    $inset = 26.0 * $s
    $rect = New-Object System.Drawing.RectangleF($inset, $inset, ($Size - (2 * $inset)), ($Size - (2 * $inset)))
    $path = New-RoundedRectPath -Rect $rect -Radius (215.0 * $s)

    $bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush($rect, $BgTop, $BgBot, 90.0)
    $g.FillPath($bg, $path)

    # Subtle top glass highlight, clipped to the tile.
    $state = $g.Save()
    $g.SetClip($path)
    $hlRect = New-Object System.Drawing.RectangleF($rect.X, $rect.Y, $rect.Width, ($rect.Height * 0.52))
    $hl = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        $hlRect,
        [System.Drawing.Color]::FromArgb(34, 255, 255, 255),
        [System.Drawing.Color]::FromArgb(0, 255, 255, 255),
        90.0)
    $g.FillRectangle($hl, $hlRect)
    $g.Restore($state)

    $edgePen = New-Object System.Drawing.Pen($Edge, (7.0 * $s))
    $g.DrawPath($edgePen, $path)

    # Play glyph. The stroke is inset into the points so the rounded joins
    # land on the intended silhouette instead of growing it.
    $ax = 412.0 * $s; $ay = 342.0 * $s
    $bx = 412.0 * $s; $by = 682.0 * $s
    $cx = 704.0 * $s; $cy = 512.0 * $s

    $tri = New-Object System.Drawing.Drawing2D.GraphicsPath
    $tri.AddPolygon([System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF($ax, $ay)),
        (New-Object System.Drawing.PointF($bx, $by)),
        (New-Object System.Drawing.PointF($cx, $cy))
    ))

    $triBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
        (New-Object System.Drawing.PointF($ax, $ay)),
        (New-Object System.Drawing.PointF($bx, $by)),
        $AccentTop, $AccentBot)
    $triPen = New-Object System.Drawing.Pen($triBrush, (52.0 * $s))
    $triPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
    $g.FillPath($triBrush, $tri)
    $g.DrawPath($triPen, $tri)

    $tri.Dispose(); $triPen.Dispose(); $triBrush.Dispose()
    $edgePen.Dispose(); $hl.Dispose(); $bg.Dispose(); $path.Dispose(); $g.Dispose()
    return $bmp
}

function Resize-Bitmap {
    param([System.Drawing.Bitmap]$Source, [int]$Size)
    $dst = New-Object System.Drawing.Bitmap($Size, $Size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($dst)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode   = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.DrawImage($Source, (New-Object System.Drawing.Rectangle(0, 0, $Size, $Size)))
    $g.Dispose()
    return $dst
}

function Get-PngBytes {
    param([System.Drawing.Bitmap]$Bitmap)
    $ms = New-Object System.IO.MemoryStream
    $Bitmap.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()
    $ms.Dispose()
    # Unary comma: keep PowerShell from enumerating the byte[] into the pipeline.
    return ,$bytes
}

# 32bpp BGRA DIB entry: BITMAPINFOHEADER + bottom-up XOR + empty AND mask.
function Get-DibBytes {
    param([System.Drawing.Bitmap]$Bitmap)
    $w = $Bitmap.Width
    $h = $Bitmap.Height
    $rect = New-Object System.Drawing.Rectangle(0, 0, $w, $h)
    $data = $Bitmap.LockBits(
        $rect,
        [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
        [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $stride = $data.Stride
    $buf = New-Object byte[] ($stride * $h)
    [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $buf, 0, $buf.Length)
    $Bitmap.UnlockBits($data)

    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    $bw.Write([int]40)              # biSize
    $bw.Write([int]$w)              # biWidth
    $bw.Write([int]($h * 2))        # biHeight (XOR + AND)
    $bw.Write([int16]1)             # biPlanes
    $bw.Write([int16]32)            # biBitCount
    $bw.Write([int]0)               # biCompression = BI_RGB
    $bw.Write([int]($w * $h * 4))   # biSizeImage
    $bw.Write([int]0); $bw.Write([int]0)
    $bw.Write([int]0); $bw.Write([int]0)

    for ($y = $h - 1; $y -ge 0; $y--) {
        $bw.Write($buf, $y * $stride, $w * 4)
    }

    $maskRow = [int]([math]::Ceiling($w / 32.0) * 4)
    $zeros = New-Object byte[] $maskRow
    for ($y = 0; $y -lt $h; $y++) { $bw.Write($zeros) }

    $bw.Flush()
    $bytes = $ms.ToArray()
    $bw.Dispose(); $ms.Dispose()
    return ,$bytes
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$entries = New-Object System.Collections.ArrayList

foreach ($size in $sizes) {
    # Supersample 4x then reduce once: avoids the aliasing a single 64x
    # downscale produces at 16px.
    $big = New-Mark -Size ($size * 4)
    $small = Resize-Bitmap -Source $big -Size $size
    $big.Dispose()

    # 256px is stored PNG-compressed (what Explorer itself uses); everything
    # else stays uncompressed 32bpp so every tool can read it.
    if ($size -ge 256) {
        $bytes = Get-PngBytes -Bitmap $small
        $isPng = $true
    } else {
        $bytes = Get-DibBytes -Bitmap $small
        $isPng = $false
    }
    [void]$entries.Add([pscustomobject]@{ Size = $size; Bytes = $bytes; Png = $isPng })
    $small.Dispose()
}

$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([int16]0)                      # reserved
$bw.Write([int16]1)                      # type = icon
$bw.Write([int16]$entries.Count)

$offset = 6 + (16 * $entries.Count)
foreach ($e in $entries) {
    $dim = if ($e.Size -ge 256) { 0 } else { $e.Size }
    $bw.Write([byte]$dim)
    $bw.Write([byte]$dim)
    $bw.Write([byte]0)                   # color count
    $bw.Write([byte]0)                   # reserved
    $bw.Write([int16]1)                  # planes
    $bw.Write([int16]32)                 # bit count
    $bw.Write([int]$e.Bytes.Length)
    $bw.Write([int]$offset)
    $offset += $e.Bytes.Length
}
foreach ($e in $entries) {
    $payload = [byte[]]$e.Bytes
    $bw.Write($payload, 0, $payload.Length)
}
$bw.Flush()
$ico = $ms.ToArray()
$bw.Dispose(); $ms.Dispose()

if (-not (Test-Path -LiteralPath $OutDir)) {
    New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
}
$icoPath = Join-Path $OutDir 'eglass.ico'
[System.IO.File]::WriteAllBytes($icoPath, $ico)

# PNG copies for the runtime window icon (eframe::icon_data::from_png_bytes).
foreach ($size in 64, 256) {
    $big = New-Mark -Size ($size * 4)
    $small = Resize-Bitmap -Source $big -Size $size
    $big.Dispose()
    $pngPath = Join-Path $OutDir ("icon-{0}.png" -f $size)
    [System.IO.File]::WriteAllBytes($pngPath, (Get-PngBytes -Bitmap $small))
    $small.Dispose()
    Write-Output ("wrote {0} ({1} bytes)" -f $pngPath, (Get-Item $pngPath).Length)
}

# Sanity check: verify the directory is self-consistent and every payload
# decodes. (System.Drawing.Icon cannot read PNG-compressed entries, so the
# 256px PNG is validated through Image instead.)
function Test-IcoBytes {
    param([byte[]]$Bytes)
    if ([BitConverter]::ToUInt16($Bytes, 0) -ne 0) { throw 'reserved field is not 0' }
    if ([BitConverter]::ToUInt16($Bytes, 2) -ne 1) { throw 'type is not 1 (icon)' }
    $count = [BitConverter]::ToUInt16($Bytes, 4)
    if ($count -ne 7) { throw "unexpected entry count $count" }

    for ($i = 0; $i -lt $count; $i++) {
        $o = 6 + (16 * $i)
        $len = [BitConverter]::ToInt32($Bytes, $o + 8)
        $off = [BitConverter]::ToInt32($Bytes, $o + 12)
        if ($len -le 0 -or $off -lt (6 + 16 * $count) -or ($off + $len) -gt $Bytes.Length) {
            throw "entry $i has an out-of-bounds payload"
        }
        if ($Bytes[$off] -eq 0x89 -and $Bytes[$off + 1] -eq 0x50) {
            $ms = [System.IO.MemoryStream]::new($Bytes, $off, $len)
            $img = [System.Drawing.Image]::FromStream($ms)
            if (-not $img) { throw "entry $i PNG failed to decode" }
            $img.Dispose(); $ms.Dispose()
        }
    }
}

Test-IcoBytes -Bytes $ico

Write-Output ("wrote {0} ({1} bytes, {2} entries: {3})" -f `
    $icoPath, (Get-Item $icoPath).Length, $entries.Count,
    (($entries | ForEach-Object { if ($_.Png) { "$($_.Size)p" } else { "$($_.Size)" } }) -join ', '))
