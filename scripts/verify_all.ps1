$ErrorActionPreference = 'Continue'

# 一条命令跑完全部验证：格式 / 静态检查 / 编译 / 单测 / 5 个端到端。
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\screenloom\scripts\verify_all.ps1
#
# 每一项独立判定，任何一项失败都记进汇总；**退出码非 0 表示有项目失败**
# （SKIP 不算失败 —— 那是硬件前提不具备，不是代码问题）。

$Root = Split-Path -Parent $PSScriptRoot
$mf   = Join-Path $Root 'Cargo.toml'
$sc   = $PSScriptRoot

$results = @()
function Add-Result([string]$name, [bool]$ok, [string]$note) {
  $script:results += [pscustomobject]@{ 项目 = $name; 结果 = $(if ($ok) { 'PASS' } else { 'FAIL' }); 说明 = $note }
}

Get-Process -Name 'screenloom' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

# ---- 1. 格式 ----
$fmtOut = & cargo fmt --manifest-path $mf --all -- --check 2>&1 | Out-String
Add-Result 'cargo fmt --check' ($LASTEXITCODE -eq 0) $(if ($LASTEXITCODE -eq 0) { '无格式差异' } else { '有格式差异（cargo fmt）' })

# ---- 2. 静态检查 ----
$clippyOut = & cargo clippy --manifest-path $mf --workspace --all-targets 2>&1 | Out-String
$clippyBad = @([regex]::Matches($clippyOut, '(?m)^(warning|error)(: |\[)')).Count
Add-Result 'cargo clippy' ($clippyBad -eq 0) "$clippyBad 条 warning/error"

# ---- 3. 编译 ----
$buildOut = & cargo build --manifest-path $mf --release -p screenloom-app --bins --examples 2>&1 | Out-String
$buildOk = $LASTEXITCODE -eq 0
$buildWarn = @([regex]::Matches($buildOut, '(?m)^warning')).Count
Add-Result 'release 构建' $buildOk $(if ($buildOk -and $buildWarn -eq 0) { '零警告' } else { "退出码 $LASTEXITCODE / $buildWarn 警告" })

# ---- 4. 单测 ----
$testOut = & cargo test --manifest-path $mf --workspace 2>&1 | Out-String
$testOk = $LASTEXITCODE -eq 0
$passed = ([regex]::Matches($testOut, 'test result: ok\. (\d+) passed')).Groups | Where-Object { $_.Name -eq '1' } | ForEach-Object { [int]$_.Value } | Measure-Object -Sum
Add-Result 'cargo test --workspace' $testOk "$($passed.Sum) 个测试通过"

# ---- 5..9. 端到端 ----
$e2e = @(
  @{ n = 'E2E 抓屏/浮窗'; f = 'e2e_overlay.ps1' },
  @{ n = 'E2E 设置保存';   f = 'e2e_settings.ps1' },
  @{ n = '深色外观留证';  f = 'capture_settings_theme.ps1' },
  @{ n = '主题热重载';    f = 'capture_theme_hotreload.ps1' },
  @{ n = 'E2E 工具栏按钮'; f = 'e2e_toolbar_buttons.ps1' },
  @{ n = 'E2E 录屏链路';   f = 'e2e_record.ps1' }
)
foreach ($e in $e2e) {
  $out = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $sc $e.f) 2>&1 | Out-String
  $code = $LASTEXITCODE
  $note = if ($code -ne 0) { "退出码 $code" } elseif ($out -match 'SKIP') { '通过（含 SKIP 项：硬件前提不具备）' } else { '通过' }
  Add-Result $e.n ($code -eq 0) $note
}

Write-Host ''
Write-Host '=== 验证汇总 ==='
$results | Format-Table -AutoSize | Out-String -Width 200 | Write-Host
$failed = @($results | Where-Object { $_.结果 -eq 'FAIL' })
if ($failed.Count -gt 0) {
  Write-Host "FAILED: $($failed.Count) 项"
  exit 1
}
Write-Host 'ALL PASS'
exit 0
