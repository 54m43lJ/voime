# register_tsf.ps1 — 注册 Voime TSF 文本服务（自提权）
# Profile 注册写 HKLM\SOFTWARE\Microsoft\CTF\TIP，必须管理员（P5-01/P5-07）。
# 可在仓库 tsf\ 目录（自动找 ..\build）或安装目录（voime_tsf.dll 同目录）运行。
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
if (!$Dll) { throw '找不到 voime_tsf.dll（请在安装目录或仓库 frontend\tsf 下运行）' }
Write-Host "[register] regsvr32 /s $Dll (elevated)"
$p = Start-Process regsvr32 -Verb RunAs -ArgumentList '/s', "`"$Dll`"" -Wait -PassThru
if ($p.ExitCode -eq 0) {
    Write-Host '[register] OK：语言栏应出现 "Voime 语音输入"（ctfmon 缓存数秒后生效）'
} else {
    Write-Host "[register] 失败 ExitCode=$($p.ExitCode)"; exit 1
}
