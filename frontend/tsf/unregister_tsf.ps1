# unregister_tsf.ps1 — 注销 Voime TSF 文本服务（自提权）
$ErrorActionPreference = 'Stop'
$Candidates = @(
    (Join-Path $PSScriptRoot 'voime_tsf.dll'),
    (Join-Path $PSScriptRoot '..\build\voime_tsf.dll'),
    (Join-Path (Get-Location) 'voime_tsf.dll')
)
$Dll = $null
foreach ($c in $Candidates) {
    if ($c -and (Test-Path $c)) { $Dll = (Resolve-Path $c).Path; break }
}
if (!$Dll) {
    Write-Host '[unregister] 未找到 voime_tsf.dll，尝试按已注册路径注销（可能失败）'
    $Dll = 'voime_tsf.dll'
}
Write-Host "[unregister] regsvr32 /u /s $Dll (elevated)"
$p = Start-Process regsvr32 -Verb RunAs -ArgumentList '/u','/s', "`"$Dll`"" -Wait -PassThru
if ($p.ExitCode -eq 0) { Write-Host '[unregister] OK' }
else { Write-Host "[unregister] 失败 ExitCode=$($p.ExitCode)"; exit 1 }
