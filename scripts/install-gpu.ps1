param(
    [switch]$Qwen3,
    [ValidateSet('vulkan', 'cuda')]
    [string]$QwenRuntime = 'vulkan',
    [ValidateSet('cpu', 'qwen3')]
    [string]$Backend = '',
    [switch]$NoModels,
    [string]$ModelDir = (Join-Path $env:LOCALAPPDATA 'voime\models'),
    [string]$SourceDir = '',
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\Voime'),
    [ValidateSet('user', 'system')]
    [string]$LlvmScope = 'user',
    [string]$LlvmVersion = '23.1.5',
    [switch]$NoLlvmInstall,
    [switch]$Yes,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$BinDir = $Prefix

function Find-Python {
    $python = Get-Command python3 -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $python) {
        $python = Get-Command python -ErrorAction SilentlyContinue | Select-Object -First 1
    }
    if (-not $python) {
        throw 'python3 is required to install GPU models; install Python or pass -NoModels'
    }
    return $python.Source
}

function Find-Libclang {
    $candidates = @()
    if ($env:LIBCLANG_PATH) { $candidates += $env:LIBCLANG_PATH }
    if ($env:LLVM_HOME) { $candidates += (Join-Path $env:LLVM_HOME 'bin') }
    $candidates += (Join-Path $env:LOCALAPPDATA 'Programs\LLVM\bin')
    $candidates += (Join-Path $env:LOCALAPPDATA 'LLVM\bin')
    foreach ($root in @($env:ProgramFiles, ${env:ProgramFiles(x86)})) {
        if ($root) {
            $candidates += (Join-Path $root 'LLVM\bin')
            foreach ($edition in @('Community', 'BuildTools', 'Professional', 'Enterprise')) {
                $vswhere = Join-Path $root "Microsoft Visual Studio\2022\$edition\VC\Tools\Llvm\x64\bin"
                $candidates += $vswhere
            }
        }
    }
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path (Join-Path $candidate 'libclang.dll'))) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Find-VulkanSdk {
    $candidates = @()
    if ($env:VULKAN_SDK) { $candidates += $env:VULKAN_SDK }
    foreach ($root in @('C:\VulkanSDK', (Join-Path $env:ProgramFiles 'VulkanSDK'))) {
        if (Test-Path $root) {
            foreach ($version in Get-ChildItem $root -Directory | Sort-Object Name -Descending) {
                $candidates += $version.FullName
            }
        }
    }
    foreach ($candidate in $candidates) {
        if ($candidate -and
            (Test-Path (Join-Path $candidate 'Bin\glslc.exe')) -and
            (Test-Path (Join-Path $candidate 'Include\vulkan\vulkan.h'))) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Find-Cuda {
    $candidates = @()
    if ($env:CUDA_PATH) { $candidates += $env:CUDA_PATH }
    $root = Join-Path $env:ProgramFiles 'NVIDIA GPU Computing Toolkit\CUDA'
    if (Test-Path $root) {
        foreach ($version in Get-ChildItem $root -Directory | Sort-Object Name -Descending) {
            $candidates += $version.FullName
        }
    }
    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path (Join-Path $candidate 'bin\nvcc.exe'))) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Get-LlvmMsi {
    param([string]$Version)
    $hashes = @{
        '23.1.5' = '288ab326377157a9e02e6fb9724914cc2b55963882d493a405f31dc66e837c12'
    }
    if (-not $hashes.ContainsKey($Version)) {
        throw "Unsupported LLVM version '$Version'; install LLVM manually or update the script."
    }
    $msi = Join-Path $env:TEMP "LLVM-$Version-win64.msi"
    if (-not (Test-Path $msi) -or (Get-Item $msi).Length -eq 0) {
        $url = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$Version/LLVM-$Version-win64.msi"
        Write-Host "Downloading LLVM $Version (about 640 MB)..."
        curl.exe -L --retry 3 --output $msi $url
        if ($LASTEXITCODE -ne 0) {
            Remove-Item -Force $msi -ErrorAction SilentlyContinue
            throw "LLVM download failed: $url"
        }
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $msi).Hash.ToLowerInvariant()
    if ($actual -ne $hashes[$Version]) {
        Remove-Item -Force $msi -ErrorAction SilentlyContinue
        throw "LLVM MSI SHA-256 mismatch; remove $msi and retry."
    }
    return $msi
}

function Install-LlvmUser {
    param([string]$Version)
    $msi = Get-LlvmMsi $Version
    $stage = Join-Path $env:TEMP "voime-llvm-$PID"
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    try {
        Write-Host 'Extracting LLVM to the user directory...'
        $process = Start-Process msiexec.exe -ArgumentList @(
            '/a', "`"$msi`"", '/qn', "TARGETDIR=`"$stage`""
        ) -Wait -PassThru
        if ($process.ExitCode -ne 0) {
            throw "LLVM MSI extraction failed with exit code $($process.ExitCode)"
        }
        $source = Join-Path $stage 'LLVM'
        if (-not (Test-Path (Join-Path $source 'bin\libclang.dll'))) {
            throw 'LLVM MSI does not contain bin\libclang.dll'
        }
        $target = Join-Path $env:LOCALAPPDATA 'Programs\LLVM'
        Remove-Item -Recurse -Force $target -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force (Join-Path $target 'bin') | Out-Null
        New-Item -ItemType Directory -Force (Join-Path $target 'lib') | Out-Null
        Copy-Item (Join-Path $source 'bin\libclang.dll') (Join-Path $target 'bin')
        $clang = Join-Path $source 'lib\clang'
        if (Test-Path $clang) {
            Copy-Item -Recurse $clang (Join-Path $target 'lib\clang')
        }
        Write-Host "LLVM libclang installed to $target"
        return (Join-Path $target 'bin')
    } finally {
        Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
    }
}

function Install-LlvmSystem {
    param([string]$Version)
    $winget = Get-Command winget.exe -ErrorAction SilentlyContinue
    try {
        if ($winget) {
            Write-Host 'Installing LLVM system-wide via winget (UAC prompt expected)...'
            $process = Start-Process -FilePath $winget.Source -ArgumentList @(
                'install', '--id', 'LLVM.LLVM', '--exact', '--silent',
                '--accept-package-agreements', '--accept-source-agreements'
            ) -Verb RunAs -Wait -PassThru
        } else {
            $msi = Get-LlvmMsi $Version
            Write-Host 'Installing LLVM system-wide via msiexec (UAC prompt expected)...'
            $process = Start-Process msiexec.exe -ArgumentList @(
                '/i', "`"$msi`"", '/qn'
            ) -Verb RunAs -Wait -PassThru
        }
    } catch {
        throw "LLVM system install was cancelled or failed: $($_.Exception.Message)"
    }
    if ($process.ExitCode -ne 0) {
        throw "LLVM system install failed with exit code $($process.ExitCode)"
    }
    $candidate = Join-Path $env:ProgramFiles 'LLVM\bin'
    if (Test-Path (Join-Path $candidate 'libclang.dll')) {
        return $candidate
    }
    return $null
}

if ($Uninstall) {
    $RuntimeDir = Join-Path $Prefix 'lib\voime'
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $BinDir 'voime-gpu-qwen3.exe')
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $RuntimeDir 'qwen3')
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue (Join-Path $ModelDir 'qwen3-asr-1.7b')
    Write-Host 'GPU backend removed; final.backend is unchanged.'
    exit 0
}

if (-not (Test-Path (Join-Path $BinDir 'voimed.exe')) -and
    -not (Test-Path (Join-Path $env:LOCALAPPDATA 'Programs\Voime\voimed.exe'))) {
    throw 'core install not found; install core before the GPU module'
}

$installModels = -not $NoModels
$python = $null
if ($installModels) { $python = Find-Python }

$prebuilt = if ([string]::IsNullOrEmpty($SourceDir)) { '' } else { Join-Path $SourceDir 'bin\voime-gpu-qwen3.exe' }
if (-not [string]::IsNullOrEmpty($prebuilt) -and (Test-Path $prebuilt)) {
    Write-Host 'Installing precompiled Qwen worker...'
    Copy-Item -Force $prebuilt $BinDir
} else {
    $libclang = Find-Libclang
    if (-not $libclang) {
        if ($NoLlvmInstall) {
            throw @'
libclang.dll not found; building the Qwen worker from source requires LLVM.
Install it with:
  winget install --id LLVM.LLVM --exact
or set LIBCLANG_PATH to the directory containing libclang.dll.
Alternatively pass -SourceDir with bin\voime-gpu-qwen3.exe to install a prebuilt worker.
'@
        }
        Write-Host "libclang.dll not found; installing LLVM $LlvmVersion ($LlvmScope)..."
        if ($LlvmScope -eq 'system') {
            $libclang = Install-LlvmSystem $LlvmVersion
        } else {
            $libclang = Install-LlvmUser $LlvmVersion
        }
        if (-not $libclang) {
            $libclang = Find-Libclang
        }
        if (-not $libclang) {
            throw 'LLVM installation completed but libclang.dll was not found.'
        }
    }
    $env:LIBCLANG_PATH = $libclang
    Write-Host "Using libclang: $libclang"

    if ($QwenRuntime -eq 'vulkan') {
        $vulkan = Find-VulkanSdk
        if (-not $vulkan) {
            throw @'
Vulkan SDK not found; building the Qwen worker with the Vulkan backend requires glslc and Vulkan headers.
Install it with:
  winget install --id KhronosGroup.VulkanSDK --exact
or set VULKAN_SDK to the SDK directory (contains Bin\glslc.exe).
Alternatively pass -SourceDir with bin\voime-gpu-qwen3.exe to install a prebuilt worker.
'@
        }
        $env:VULKAN_SDK = $vulkan
        $env:PATH = (Join-Path $vulkan 'Bin') + ';' + $env:PATH
        Write-Host "Using Vulkan SDK: $vulkan"
    } elseif ($QwenRuntime -eq 'cuda') {
        $cuda = Find-Cuda
        if (-not $cuda) {
            throw @'
CUDA Toolkit not found; building the Qwen worker with the CUDA backend requires nvcc.
Install the CUDA Toolkit or set CUDA_PATH to the toolkit directory.
Alternatively pass -SourceDir with bin\voime-gpu-qwen3.exe to install a prebuilt worker.
'@
        }
        $env:CUDA_PATH = $cuda
        $env:PATH = (Join-Path $cuda 'bin') + ';' + $env:PATH
        Write-Host "Using CUDA Toolkit: $cuda"
    }

    $env:CFLAGS = ('/utf-8 ' + $env:CFLAGS).Trim()
    $env:CXXFLAGS = ('/utf-8 ' + $env:CXXFLAGS).Trim()

    Write-Host "Building Qwen worker ($QwenRuntime)..."
    Push-Location (Join-Path $Root 'crates\voime-gpu-qwen3')
    try {
        cargo build --release --features $QwenRuntime
        if ($LASTEXITCODE -ne 0) { throw 'Qwen worker build failed' }
    } finally { Pop-Location }
    Copy-Item -Force (Join-Path $Root 'crates\voime-gpu-qwen3\target\release\voime-gpu-qwen3.exe') $BinDir
}

if ($installModels) {
    $modelArgs = @(
        (Join-Path $Root 'scripts\install-gpu-models.py'),
        '--model-root', $ModelDir,
        '--qwen3'
    )
    if (-not [string]::IsNullOrEmpty($SourceDir)) {
        $resolvedSourceDir = (Resolve-Path -LiteralPath $SourceDir).Path
        $modelArgs += @('--source-dir', $resolvedSourceDir, '--no-download')
    }
    if ($Yes) { $modelArgs += '--yes' }
    & $python @modelArgs
    if ($LASTEXITCODE -ne 0) { throw 'GPU model installation failed' }
}

function Set-FinalBackend {
    param([string]$Config, [string]$Backend)
    $lines = @()
    if (Test-Path $Config) { $lines = Get-Content $Config }
    $out = @()
    $inFinal = $false
    $written = $false
    $hasFinal = $false
    foreach ($line in $lines) {
        if ($line -match '^\s*\[final\]\s*$') {
            if ($inFinal -and -not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            $inFinal = $true
            $hasFinal = $true
            $out += $line
            continue
        }
        if ($line -match '^\s*\[') {
            if ($inFinal -and -not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            $inFinal = $false
            $out += $line
            continue
        }
        if ($inFinal -and $line -match '^\s*backend\s*=') {
            if (-not $written) {
                $out += "backend = `"$Backend`""
                $written = $true
            }
            continue
        }
        $out += $line
    }
    if ($inFinal -and -not $written) {
        $out += "backend = `"$Backend`""
        $written = $true
    }
    if (-not $hasFinal) {
        $out += '[final]'
        $out += "backend = `"$Backend`""
    }
    $temporary = "$Config.tmp"
    Set-Content -Path $temporary -Value $out -Encoding UTF8
    Move-Item -Force $temporary $Config
}

if (-not [string]::IsNullOrEmpty($Backend)) {
    $configDir = Join-Path $env:APPDATA 'voime'
    New-Item -ItemType Directory -Force -Path $configDir | Out-Null
    $config = Join-Path $configDir 'config.toml'
    Set-FinalBackend -Config $config -Backend $Backend
    Write-Host "final.backend = $Backend (restart voimed to apply)"
}

Write-Host "GPU module installed. Run: $(Join-Path $BinDir 'voime.exe') doctor"
