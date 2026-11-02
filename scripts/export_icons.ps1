[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$Svg = Join-Path $Root 'assets\icons\voime.svg'
$OutDir = Split-Path -Parent $Svg
$Work = Join-Path $env:TEMP 'voime-icon-export'
$Sizes = @(16, 24, 32, 48, 64, 128, 256)

$browser = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Microsoft\Edge\Application\msedge.exe'),
    (Join-Path $env:ProgramFiles 'Microsoft\Edge\Application\msedge.exe'),
    (Join-Path $env:ProgramFiles 'Google\Chrome\Application\chrome.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Google\Chrome\Application\chrome.exe')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
if (-not $browser) {
    throw 'export_icons: Edge or Chrome is required to rasterize the SVG'
}
if (-not (Test-Path -LiteralPath $Svg)) {
    throw "export_icons: missing $Svg"
}

Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Work | Out-Null
$html = Join-Path $Work 'icon.html'
$svgUri = ([uri]$Svg).AbsoluteUri
$htmlUri = ([uri]$html).AbsoluteUri

$images = @()
foreach ($size in $Sizes) {
    @"
<!doctype html><html><head><style>html,body{margin:0;padding:0;background:transparent;overflow:hidden}img{display:block}</style></head>
<body><img src="$svgUri" width="$size" height="$size"></body></html>
"@ | Set-Content -LiteralPath $html -Encoding UTF8
    $png = Join-Path $Work "voime-$size.png"
    $arguments = @(
        '--headless',
        '--disable-gpu',
        '--hide-scrollbars',
        '--force-device-scale-factor=1',
        '--default-background-color=00000000',
        "--window-size=$size,$size",
        "--screenshot=$png",
        $htmlUri
    )
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'SilentlyContinue'
    & $browser @arguments 2>$null | Out-Null
    $ErrorActionPreference = $previous
    for ($attempt = 0; $attempt -lt 50 -and -not (Test-Path -LiteralPath $png); $attempt++) {
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath $png)) {
        throw "export_icons: failed to render ${size}x${size}"
    }
    $images += $png
}

Copy-Item -LiteralPath (Join-Path $Work 'voime-32.png') -Destination (Join-Path $OutDir 'voime-32.png') -Force
Copy-Item -LiteralPath (Join-Path $Work 'voime-256.png') -Destination (Join-Path $OutDir 'voime-256.png') -Force

$entries = New-Object System.Collections.Generic.List[byte]
$payload = New-Object System.Collections.Generic.List[byte]
$offset = 6 + 16 * $images.Count
foreach ($path in $images) {
    Add-Type -AssemblyName System.Drawing
    $image = [System.Drawing.Image]::FromFile($path)
    $width = $image.Width
    $height = $image.Height
    $image.Dispose()
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $widthByte = if ($width -ge 256) { 0 } else { $width }
    $heightByte = if ($height -ge 256) { 0 } else { $height }
    $entries.Add([byte]$widthByte)
    $entries.Add([byte]$heightByte)
    $entries.Add([byte]0)
    $entries.Add([byte]0)
    $entries.AddRange([BitConverter]::GetBytes([uint16]1))
    $entries.AddRange([BitConverter]::GetBytes([uint16]32))
    $entries.AddRange([BitConverter]::GetBytes([uint32]$bytes.Length))
    $entries.AddRange([BitConverter]::GetBytes([uint32]$offset))
    $payload.AddRange($bytes)
    $offset += $bytes.Length
}

$header = [byte[]]@(0, 0, 1, 0, [byte]$images.Count, 0)
$ico = Join-Path $OutDir 'voime.ico'
[System.IO.File]::WriteAllBytes($ico, $header + $entries.ToArray() + $payload.ToArray())
Write-Host "export_icons: wrote $ico"
