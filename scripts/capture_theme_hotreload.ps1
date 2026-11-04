$ErrorActionPreference = 'Stop'

# 验证「主题热重载」：改了 JSON 覆盖文件后，设置窗口必须**不重启**就换配色。
#
# 判据（二值、可外部复核）：
#   1) 首帧用颜色 A  -> 图里有 A 的像素
#   2) 改写 JSON 为颜色 B（不重启进程）
#   3) 等定时器（400ms 一跳）-> 图里有 B 的像素，且 A 的像素几乎归零
#   4) 全程进程没重启（同一个 PID）
#
# 任一条不过就 exit 1。

. "$PSScriptRoot\_shotlib.ps1"

$exe    = 'D:\github\screenloom\target\release\screenloom.exe'
$outDir = 'D:\github\screenloom\probes\e2e_out'
if (-not (Test-Path $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
$cfg    = "$env:TEMP\screenloom_hotreload_cfg.txt"
$theme  = "$env:TEMP\screenloom_hotreload_theme.json"

# A = 红 (255,59,48)   B = 蓝 (10,132,255)
$A = @(255, 59, 48)
$B = @(10, 132, 255)

function Write-Theme([int[]]$accent) {
  $hex = '{0:X2}{1:X2}{2:X2}' -f $accent[0], $accent[1], $accent[2]
  $json = '{ "accent": "#' + $hex + '" }'
  # 必须**不带 BOM** —— serde_json 不认 BOM
  [System.IO.File]::WriteAllText($theme, $json, (New-Object System.Text.UTF8Encoding($false)))
}

Get-Process -Name 'screenloom' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400
if (Test-Path $theme) { Remove-Item $theme -Force }

Write-Theme $A
$proc = Start-ScreenloomApp $exe $cfg @('--settings', '--theme', $theme)

$win = [IntPtr]::Zero
for ($i = 0; $i -lt 50 -and $win -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 200
  $win = [Shot]::Find('ScreenloomSettingsWindow')
}
if ($win -eq [IntPtr]::Zero) { "FAIL: 找不到设置窗口"; Stop-Process -Id $proc.Id -Force; exit 1 }
$pid0 = $proc.Id
"设置窗口句柄: $win   PID: $pid0"

[void][Shot]::ShowWindow($win, 5)
Set-TopMost $win
Start-Sleep -Milliseconds 900

$wr = Get-WindowRect $win
$ww = $wr.R - $wr.L
$wh = $wr.B - $wr.T
"整窗(物理): $($wr.L),$($wr.T) ${ww}x${wh}"

# 证明是 1:1 物理像素（否则颜色计数可能被缩放/重采样影响）
$phys = Test-PhysicalScale $win 640
"客户区宽: $($phys.ClientW)  DPI: $($phys.Dpi)  k(DPI)=$([Math]::Round($phys.KDpi,4))  k(几何)=$([Math]::Round($phys.KGeom,4))"
if (-not $phys.Ok) {
  "FAIL: 客户区宽与 DPI 不自洽 —— 抓屏坐标被 DPI 虚拟化了"
  Stop-Process -Id $proc.Id -Force; exit 1
}

# ---- 首帧：应该是颜色 A
Save-Screen $wr.L $wr.T $ww $wh "$outDir\theme_reload_before.png"
$aBefore = Get-ColorCount "$outDir\theme_reload_before.png" $A[0] $A[1] $A[2]
$bBefore = Get-ColorCount "$outDir\theme_reload_before.png" $B[0] $B[1] $B[2]
"改之前: A($($A -join ','))=$aBefore   B($($B -join ','))=$bBefore"
if ($aBefore -lt 200) { "FAIL: 首帧没有颜色 A —— 主题覆盖根本没生效"; Stop-Process -Id $proc.Id -Force; exit 1 }

# ---- 改写主题文件（**不重启进程**）
Write-Theme $B
"已改写主题文件，等待热重载..."
Start-Sleep -Milliseconds 1500

# ---- 之后：应该是颜色 B
Save-Screen $wr.L $wr.T $ww $wh "$outDir\theme_reload_after.png"
$aAfter = Get-ColorCount "$outDir\theme_reload_after.png" $A[0] $A[1] $A[2]
$bAfter = Get-ColorCount "$outDir\theme_reload_after.png" $B[0] $B[1] $B[2]
"改之后: A=$aAfter   B=$bAfter"

if ($proc.HasExited) { "FAIL: 进程退出了（热重载不该重启进程）"; exit 1 }
if ($proc.Id -ne $pid0) { "FAIL: PID 变了，说明重启了进程而不是热重载"; Stop-Process -Id $proc.Id -Force; exit 1 }

Stop-Process -Id $proc.Id -Force

if ($bAfter -lt 200) { "FAIL: 改完文件后没出现颜色 B —— 热重载没生效"; exit 1 }
if ($aAfter -gt 50)   { "FAIL: 颜色 A 还残留 $($aAfter) px —— 旧配色没被替换"; exit 1 }

"=== 汇总 ==="
"热重载生效：同一 PID ($pid0) 内，A($aBefore px) -> B($bAfter px)"
"留证: $outDir\theme_reload_before.png / theme_reload_after.png"
"ALL PASS"
exit 0
