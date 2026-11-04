param(
  # 用 debug profile（增量 2s，比 release 的 7s 快，但运行较慢）
  [switch]$Debug,
  # 只编译一次并启动，不进入监视
  [switch]$NoWatch,
  # 启动后跑什么
  [ValidateSet('settings','resident')][string]$Mode = 'settings',
  # 主题覆盖文件（改了会被应用热重载，不用重编译）
  [string]$Theme
)

$ErrorActionPreference = 'Continue'

$root = Split-Path -Parent $PSScriptRoot      # ...\screenloom
Set-Location $root

if (-not $Theme) { $Theme = Join-Path $root 'theme.dev.json' }
$profile  = if ($Debug) { 'dev' } else { 'release' }
$exe      = Join-Path $root "target\$profile\screenloom.exe"
$manifest = Join-Path $root 'Cargo.toml'

$watchDirs  = @('screenloom-core\src','screenloom-capture\src','screenloom-ui\src','screenloom-app\src','screenloom-app\assets')
$watchFiles = @('Cargo.toml','screenloom-core\Cargo.toml','screenloom-capture\Cargo.toml','screenloom-ui\Cargo.toml','screenloom-app\Cargo.toml','screenloom-app\build.rs')

# 事件日志（事后可核验监视循环干了什么）
$DevLog = "$env:TEMP\screenloom_dev.log"
function Log-Msg([string]$m) {
  $line = "[$((Get-Date).ToString('HH:mm:ss'))] $m"
  Write-Host $line
  Add-Content -Path $DevLog -Value $line -Encoding UTF8
}

function Get-Fingerprint {
  $parts = @()
  foreach ($d in $watchDirs) {
    if (-not (Test-Path $d)) { continue }
    foreach ($f in Get-ChildItem -Path $d -Recurse -File) {
      $parts += "$($f.FullName)|$($f.LastWriteTimeUtc.Ticks)|$($f.Length)"
    }
  }
  foreach ($f in $watchFiles) {
    if (Test-Path $f) {
      $i = Get-Item $f
      $parts += "$($i.FullName)|$($i.LastWriteTimeUtc.Ticks)|$($i.Length)"
    }
  }
  return (($parts | Sort-Object) -join "`n")
}

function Stop-App {
  Get-Process -Name 'screenloom' -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Milliseconds 300
}

function Build-App {
  $log = "$env:TEMP\screenloom_dev_build.log"
  if ($Debug) {
    & cargo build --manifest-path $manifest -p screenloom-app --bins *>&1 | Out-File -FilePath $log -Encoding UTF8
  } else {
    & cargo build --manifest-path $manifest --release -p screenloom-app --bins *>&1 | Out-File -FilePath $log -Encoding UTF8
  }
  if ($LASTEXITCODE -eq 0) {
    $warn = @(Select-String -Path $log -Pattern '^warning' -ErrorAction SilentlyContinue).Count
    if ($warn -gt 0) { Log-Msg "  编译 OK（$warn 个警告）" } else { Log-Msg "  编译 OK" }
    return $true
  }
  Log-Msg "  编译失败："
  Get-Content $log -Tail 30 -Encoding UTF8 | ForEach-Object { Log-Msg "    $_" }
  return $false
}

function Start-App {
  if (-not (Test-Path $exe)) { return }
  $args = @()
  if ($Mode -eq 'settings') { $args += '--settings' }
  if (Test-Path $Theme) { $args += @('--theme', $Theme) }
  Start-Process -FilePath $exe -ArgumentList $args -WindowStyle Minimized | Out-Null
  Log-Msg "  已启动: $exe $($args -join ' ')"
}

function Get-ThemeStamp {
  if (Test-Path $Theme) { return (Get-Item $Theme).LastWriteTimeUtc.Ticks }
  return $null
}

"=== screenloom 开发模式 ==="
"profile : $profile"
"模式    : $Mode"
"主题    : $Theme$(if (Test-Path $Theme) { '' } else { '（不存在，将只用内置默认主题）' })"
"监视    : $($watchDirs -join ', ') + Cargo.toml/build.rs"
"事件日志: $DevLog"
""
Set-Content -Path $DevLog -Value "" -Encoding UTF8

Stop-App
if (-not (Build-App)) { "首次编译失败，退出。"; exit 1 }
Start-App

if ($NoWatch) { exit 0 }

""
"进入监视模式（Ctrl+C 退出）"
$last       = Get-Fingerprint
$themeStamp = Get-ThemeStamp

while ($true) {
  Start-Sleep -Milliseconds 400

  # 主题文件：应用自己会热重载，**不需要**重编译
  $ts = Get-ThemeStamp
  if ($null -ne $ts -and $null -ne $themeStamp -and $ts -ne $themeStamp) {
    Log-Msg "主题文件变了 -> 应用会自行热重载（无需重编译）"
  }
  $themeStamp = $ts

  $now = Get-Fingerprint
  if ($now -ne $last) {
    $last = $now
    # 等编辑器把文件写完
    Start-Sleep -Milliseconds 300
    Log-Msg "源码有改动，重新编译..."
    Stop-App
    if (Build-App) { Start-App } else { Log-Msg "  保留旧版本运行，修复后再保存会自动重试" }
  }
}
