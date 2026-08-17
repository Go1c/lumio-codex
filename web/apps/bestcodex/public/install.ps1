# BestCodex installer for Windows — https://bestcodex.app
#
#   irm https://bestcodex.app/install.ps1 | iex
#
# 只做三件事：取当前版本的安装器、按 SHA256 校验、运行它。
# 不需要管理员权限（安装器装到 %LOCALAPPDATA%），不写任何配置。
#
# 内测包尚未签名，SmartScreen 可能提示「未识别的应用」。校验和对得上就说明文件与
# 构建产物一致；是否继续由你决定。
#
# 环境变量：
#   BESTCODEX_MANIFEST_URL  覆盖版本指针（默认 S3）
#   BESTCODEX_DRY_RUN=1     只解析并打印将要做什么，不下载不安装

$ErrorActionPreference = 'Stop'
# Windows PowerShell 5.1 的 Invoke-WebRequest 会为进度条逐字节重绘，几十 MB 能拖到几分钟。
$ProgressPreference = 'SilentlyContinue'

$manifestUrl = if ($env:BESTCODEX_MANIFEST_URL) {
  $env:BESTCODEX_MANIFEST_URL
} else {
  'https://s3.lumio.games/lumio-codex/releases/latest-internal.json'
}

function Fail($message) {
  Write-Error "install.ps1: $message"
  exit 1
}

if (-not [Environment]::Is64BitOperatingSystem) {
  Fail '只提供 64 位版本'
}

Write-Host "→ 读取版本指针 $manifestUrl"
try {
  $manifest = Invoke-RestMethod -Uri $manifestUrl -TimeoutSec 30
} catch {
  Fail "读不到版本指针，检查网络后重试：$_"
}

$asset = $manifest.assets | Where-Object { $_.name -like '*windows-x64-setup-internal-unsigned.exe' } | Select-Object -First 1
if (-not $asset) { Fail '指针里没有 Windows 安装器' }

$version = if ($manifest.PSObject.Properties['version']) { $manifest.version } else { '未知' }
$sumsUrl = ($asset.url -replace '/[^/]+$', '/SHA256SUMS.txt')

Write-Host "→ 版本 $version（内测版，未签名）"
Write-Host "→ 安装器 $($asset.name)"

if ($env:BESTCODEX_DRY_RUN -eq '1') {
  Write-Host '（BESTCODEX_DRY_RUN=1，到此为止，什么都没动）'
  exit 0
}

$workDir = Join-Path $env:TEMP ("bestcodex-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $workDir | Out-Null
$exePath = Join-Path $workDir $asset.name

try {
  Write-Host '→ 下载'
  Invoke-WebRequest -Uri $asset.url -OutFile $exePath -TimeoutSec 900

  Write-Host '→ 校验 SHA256'
  # 校验失败一律中止：宁可不装，也不能运行来源不明的安装器。
  $sums = (Invoke-WebRequest -Uri $sumsUrl -TimeoutSec 30).Content -split "`n"
  $line = $sums | Where-Object { $_ -match [Regex]::Escape($asset.name) } | Select-Object -First 1
  if (-not $line) { Fail "取不到校验和（$sumsUrl），中止" }
  $expected = ($line -split '\s+')[0]
  $actual = (Get-FileHash -Path $exePath -Algorithm SHA256).Hash
  if ($expected.ToLower() -ne $actual.ToLower()) {
    Fail "校验和不匹配，中止。期望 $expected，实际 $actual"
  }

  Write-Host '→ 运行安装器'
  # /S 是 NSIS 的静默安装。个别环境下静默会被拒，那就退回交互式，让你自己点完。
  $process = Start-Process -FilePath $exePath -ArgumentList '/S' -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    Write-Host "→ 静默安装返回 $($process.ExitCode)，改为交互式"
    Start-Process -FilePath $exePath -Wait
  }
} finally {
  Remove-Item -Recurse -Force $workDir -ErrorAction SilentlyContinue
}

$installed = Join-Path $env:LOCALAPPDATA 'Programs\Lumio Codex'
if (Test-Path $installed) {
  Write-Host ''
  Write-Host "装好了：$installed"
} else {
  Write-Host ''
  Write-Host '安装器已退出，但没找到安装目录。请从开始菜单确认，或到 https://bestcodex.app 手动下载。'
}
Write-Host '打开 BestCodex，在应用内登录一次，连接与本机配置会自动写好。'
Write-Host '官方 Codex 应用不捆绑在这里，需要时另行安装。'
Write-Host '遇到问题看 https://bestcodex.app/help/install'
