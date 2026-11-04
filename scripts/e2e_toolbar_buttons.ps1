# 工具栏新增按钮的端到端验证（Windows PowerShell 5.1）
#
# 覆盖三条链路，每条都有**外部可证伪**的判据：
#   A. 工具栏里真的多出「提取文字」「滚动截图」两个按钮
#      判据：app 自己的几何自省输出（stderr，需 SCREENLOOM_DEBUG_TOOLBAR=1）里有这两项
#   B. 点「提取文字」→ 本地 OCR 真的跑通
#      判据：**剪贴板**里出现屏幕上那段文字（不是"看起来对"，是能读出来的字）
#   C. 点「滚动截图」→ 真的进入滚动拼接
#      判据：出现 class=ScreenshotLongShotHint 的提示窗；按 Esc 结束后落盘一张新 PNG
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\screenloom\scripts\e2e_toolbar_buttons.ps1

$ErrorActionPreference = 'Stop'

$Root    = 'D:\github\screenloom'
$Exe     = Join-Path $Root 'target\release\screenloom.exe'
$CfgPath = Join-Path $Root 'e2e_config.txt'
$OutDir  = Join-Path $Root 'probes\e2e_out'
$SaveDir = Join-Path $env:USERPROFILE 'Pictures\Screenloom'
$ErrLog  = Join-Path $OutDir 'toolbar_app.err.log'

. (Join-Path $Root 'scripts\_shotlib.ps1')

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
Get-Process screenloom -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

# ---------------- Win32 消息投递 ----------------
if (-not ('Msg' -as [type])) {
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Msg {
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
}
"@
}
$WM_MOUSEMOVE   = 0x0200
$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP   = 0x0202
$WM_KEYDOWN     = 0x0100
$WM_KEYUP       = 0x0101

function LParam([int]$x, [int]$y) {
  return [IntPtr](($y -band 0xFFFF) -shl 16 -bor ($x -band 0xFFFF))
}
function Click-At([IntPtr]$h, [int]$x, [int]$y) {
  [void][Msg]::PostMessageW($h, $WM_LBUTTONDOWN, [IntPtr]1, (LParam $x $y))
  Start-Sleep -Milliseconds 120
  [void][Msg]::PostMessageW($h, $WM_LBUTTONUP, [IntPtr]0, (LParam $x $y))
}
function Drag-At([IntPtr]$h, [int]$x0, [int]$y0, [int]$x1, [int]$y1) {
  [void][Msg]::PostMessageW($h, $WM_LBUTTONDOWN, [IntPtr]1, (LParam $x0 $y0))
  Start-Sleep -Milliseconds 120
  [void][Msg]::PostMessageW($h, $WM_MOUSEMOVE, [IntPtr]0, (LParam $x1 $y1))
  Start-Sleep -Milliseconds 120
  [void][Msg]::PostMessageW($h, $WM_LBUTTONUP, [IntPtr]0, (LParam $x1 $y1))
}
function Wait-Window([scriptblock]$probe, [int]$seconds = 25) {
  $end = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $end) {
    $v = & $probe
    if ($null -ne $v -and $v -ne [IntPtr]::Zero) { return $v }
    Start-Sleep -Milliseconds 250
  }
  return [IntPtr]::Zero
}
function Get-LatestShot {
  Get-ChildItem $SaveDir -Filter '*.png' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

# ================= 0. 造一个带文字的窗口作为 OCR 素材 =================
Write-Host '=== 0. 准备 OCR 素材（记事本里写一句话） ==='
$txtFile = Join-Path $env:TEMP 'screenloom_ocr_test.txt'
[System.IO.File]::WriteAllText($txtFile, "SCREENLOOM OCR TEST 12345`r`n", (New-Object System.Text.UTF8Encoding($false)))
Start-Process notepad -ArgumentList $txtFile | Out-Null
$npHwnd = Wait-Window { [Shot]::FindByTitle('screenloom_ocr_test') } 20
if ($npHwnd -eq [IntPtr]::Zero) { Write-Host '  !! 没能打开记事本窗口'; exit 1 }
Set-TopMost $npHwnd
[void][Shot]::SetForegroundWindow($npHwnd)
Start-Sleep -Milliseconds 900
$nr = Get-WindowRect $npHwnd
Write-Host "  记事本窗口: $($nr.L),$($nr.T) - $($nr.R),$($nr.B)"

# ================= 1. 启动 app（开几何自省） =================
Write-Host ''
Write-Host '=== 1. 启动 screenloom-app ==='
$env:SCREENLOOM_DEBUG_TOOLBAR = '1'
if (Test-Path $ErrLog) { Remove-Item $ErrLog -Force }
$proc = Start-Process -FilePath $Exe -ArgumentList @('--once', '--config', $CfgPath) -PassThru `
  -RedirectStandardOutput (Join-Path $OutDir 'toolbar_app.out.log') `
  -RedirectStandardError  $ErrLog
$ov = Wait-Window { [Shot]::FindByTitle('screenloom overlay') } 30
if ($ov -eq [IntPtr]::Zero) { Write-Host '  !! 没等到浮窗窗口'; exit 1 }
Write-Host "  浮窗句柄 = $ov"
Start-Sleep -Milliseconds 900

$clientOrigin = [Shot]::ClientOrigin($ov)
$cr = New-Object Shot+RECT
[void][Shot]::GetClientRect($ov, [ref]$cr)
$vw = $cr.R - $cr.L; $vh = $cr.B - $cr.T
Write-Host "  浮窗客户区 = $($vw)x$vh，客户区原点(屏幕) = $($clientOrigin.X),$($clientOrigin.Y)"

# ---- A. 工具栏几何自省 ----
$geo = @{}
if (Test-Path $ErrLog) {
  foreach ($line in (Get-Content $ErrLog)) {
    if ($line -match '^\[toolbar\] (\w+) phys=\((-?\d+),(-?\d+) (-?\d+)x(-?\d+)\)') {
      $geo[$matches[1]] = @{
        x = [int]$matches[2]; y = [int]$matches[3]
        w = [int]$matches[4]; h = [int]$matches[5]
      }
    }
  }
}
Write-Host ''
Write-Host '=== A. 工具栏新增按钮 ==='
$okA = $true
foreach ($k in @('ExtractText', 'LongShot')) {
  if ($geo.ContainsKey($k)) {
    $g = $geo[$k]
    Write-Host "  OK  $k phys=($($g.x),$($g.y) $($g.w)x$($g.h))"
  } else {
    Write-Host "  !! 工具栏里没有 $k"
    $okA = $false
  }
}
if (-not $okA) { exit 1 }

# ================= 2. 拖出选区（记事本那行字） =================
# 用**真实鼠标**拖拽：合成消息会被真实鼠标活动插队（这台机器上有人用鼠标）。
$cx0 = ($nr.L + 20) - $clientOrigin.X
$cy0 = ($nr.T + 60) - $clientOrigin.Y
$cx1 = ($nr.L + 600) - $clientOrigin.X
$cy1 = ($nr.T + 200) - $clientOrigin.Y
Write-Host ''
Write-Host "=== 2. 拖出选区 ($cx0,$cy0)-($cx1,$cy1) 并进入编辑态 ==="
[Shot]::Drag(($cx0 + $clientOrigin.X), ($cy0 + $clientOrigin.Y), ($cx1 + $clientOrigin.X), ($cy1 + $clientOrigin.Y))
Start-Sleep -Milliseconds 600
[void][Msg]::PostMessageW($ov, $WM_KEYDOWN, [IntPtr]0x0D, [IntPtr]0)
[void][Msg]::PostMessageW($ov, $WM_KEYUP, [IntPtr]0x0D, [IntPtr]0)
Start-Sleep -Milliseconds 900

# 选区自省（app 自己的输出）——确认选区真的落在那行字上
$selLine = (Get-Content $ErrLog -ErrorAction SilentlyContinue) |
  Where-Object { $_ -match '^\[selection\]' } | Select-Object -Last 1
if ($selLine) { Write-Host "  $selLine" }

Save-Screen $clientOrigin.X $clientOrigin.Y $vw $vh (Join-Path $OutDir 'toolbar_buttons.png')
Write-Host "  留证: $(Join-Path $OutDir 'toolbar_buttons.png')"

# ================= B. 点「提取文字」 =================
Write-Host ''
Write-Host '=== B. 点「提取文字」→ 本地 OCR ==='
try { Clear-Clipboard -ErrorAction Stop } catch { }
$e = $geo['ExtractText']
$ex = $e.x + [int]($e.w / 2)
$ey = $e.y + [int]($e.h / 2)
Write-Host "  点击坐标 ($ex,$ey)"
[Shot]::Click(($ex + $clientOrigin.X), ($ey + $clientOrigin.Y))

$deadline = (Get-Date).AddSeconds(30)
$clip = ''
while ((Get-Date) -lt $deadline) {
  Start-Sleep -Milliseconds 900
  try { $clip = Get-Clipboard -Raw -ErrorAction Stop } catch { $clip = '' }
  if ($null -ne $clip -and $clip.ToString().Trim().Length -gt 0) { break }
}
$clipText = if ($null -eq $clip) { '' } else { $clip.ToString() }
Write-Host "  剪贴板内容: [$($clipText -replace "`r?`n", ' / ')]"
Save-Screen $clientOrigin.X $clientOrigin.Y $vw $vh (Join-Path $OutDir 'ocr_panel.png')
Write-Host "  留证: $(Join-Path $OutDir 'ocr_panel.png')"
$okB = $clipText -match 'SCREENLOOM'
if ($okB) { Write-Host '  OK  OCR 读到了屏幕上的字' }
else { Write-Host '  !! 剪贴板里没有预期的文字' }

# ================= C. 点「滚动截图」 =================
Write-Host ''
Write-Host '=== C. 点「滚动截图」→ 滚动拼接 ==='
$beforeShot = Get-LatestShot
$l = $geo['LongShot']
$lx = $l.x + [int]($l.w / 2)
$ly = $l.y + [int]($l.h / 2)
Write-Host "  点击坐标 ($lx,$ly)"
[Shot]::Click(($lx + $clientOrigin.X), ($ly + $clientOrigin.Y))

$hint = Wait-Window { [Shot]::Find('ScreenshotLongShotHint') } 15
$okC = $false
if ($hint -ne [IntPtr]::Zero) {
  Write-Host "  OK  出现滚动截图提示窗 ($hint)"
  Save-Screen $clientOrigin.X $clientOrigin.Y $vw $vh (Join-Path $OutDir 'longshot_hint.png')
  Write-Host "  留证: $(Join-Path $OutDir 'longshot_hint.png')"
  # 按 Esc 结束长截图
  [void][Msg]::PostMessageW($hint, $WM_KEYDOWN, [IntPtr]0x1B, [IntPtr]0)
  [void][Msg]::PostMessageW($hint, $WM_KEYUP, [IntPtr]0x1B, [IntPtr]0)
  Start-Sleep -Milliseconds 2500
  $afterShot = Get-LatestShot
  if ($afterShot -and (!$beforeShot -or $afterShot.LastWriteTime -gt $beforeShot.LastWriteTime)) {
    Write-Host "  OK  长截图结果已落盘: $($afterShot.FullName)"
    $okC = $true
  } else {
    Write-Host '  !! 长截图没有产生新的存盘文件'
  }
} else {
  Write-Host '  !! 没有出现滚动截图提示窗'
}

# ================= 收尾 =================
Get-Process notepad -ErrorAction SilentlyContinue | Where-Object {
  $_.MainWindowTitle -like '*screenloom_ocr_test*'
} | Stop-Process -Force -ErrorAction SilentlyContinue
if (-not $proc.HasExited) { $proc | Stop-Process -Force -ErrorAction SilentlyContinue }
Remove-Item Env:\SCREENLOOM_DEBUG_TOOLBAR -ErrorAction SilentlyContinue

Write-Host ''
Write-Host '=== 汇总 ==='
Write-Host "  A 工具栏新增按钮: $(if ($okA) { 'PASS' } else { 'FAIL' })"
Write-Host "  B 提取文字(OCR): $(if ($okB) { 'PASS' } else { 'FAIL' })"
Write-Host "  C 滚动截图:      $(if ($okC) { 'PASS' } else { 'FAIL' })"
if (-not ($okA -and $okB -and $okC)) { exit 1 }
exit 0
