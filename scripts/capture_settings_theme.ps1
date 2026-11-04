$ErrorActionPreference = 'Stop'

# 抓设置窗口的**真实屏幕**留证（不是 PrintWindow）。
# 判据是二值的、可外部复核的：
#   1) 抓到的位图尺寸 == GetWindowRect 尺寸，且客户区宽与 DPI 自洽 -> 1:1 物理像素
#   2) 出现 ScreenloomComboPopup 顶层窗口                             -> 下拉真的弹出了
#   3) 位图里存在菜单底色 #232830 精确像素                         -> 菜单真被画进了截图
#   4) 弹出窗口高度 == 3*item_h + 8                                -> 点中的是「复制格式」那个 3 项下拉

. "$PSScriptRoot\_shotlib.ps1"

$exe    = 'D:\github\screenloom\target\release\screenloom.exe'
$outDir = 'D:\github\screenloom\e2e_out'
$cfg    = "$env:TEMP\screenloom_shot_cfg.txt"

Get-Process -Name 'screenloom' -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

$proc = Start-ScreenloomApp $exe $cfg @('--settings')

# ---- 1. 等设置窗口
$win = [IntPtr]::Zero
for ($i = 0; $i -lt 50 -and $win -eq [IntPtr]::Zero; $i++) {
  Start-Sleep -Milliseconds 200
  $win = [Shot]::Find('ScreenloomSettingsWindow')
}
if ($win -eq [IntPtr]::Zero) { "FAIL: 找不到设置窗口"; Stop-Process -Id $proc.Id -Force; exit 1 }
"设置窗口句柄: $win"

[void][Shot]::ShowWindow($win, 5)          # SW_SHOW
Set-TopMost $win
[void][Shot]::SetForegroundWindow($win)
Start-Sleep -Milliseconds 800

$wr = Get-WindowRect $win
$ww = $wr.R - $wr.L
$wh = $wr.B - $wr.T
"整窗(物理): $($wr.L),$($wr.T) ${ww}x${wh}"

# ---- 2. 证明是 1:1 物理像素（不是 DPI 虚拟化后的缩小图）
$phys = Test-PhysicalScale $win 640
"客户区宽: $($phys.ClientW)  DPI: $($phys.Dpi)  k(DPI)=$([Math]::Round($phys.KDpi,4))  k(几何)=$([Math]::Round($phys.KGeom,4))"
if (-not $phys.Ok) {
  "FAIL: 客户区宽与 DPI 不自洽 —— 抓屏坐标被 DPI 虚拟化了"
  Stop-Process -Id $proc.Id -Force; exit 1
}

# ---- 3. 抓整窗（真实屏幕）
Save-Screen $wr.L $wr.T $ww $wh "$outDir\settings_window.png"
$p = New-Object System.Drawing.Bitmap("$outDir\settings_window.png")
$bw = $p.Width; $bh = $p.Height; $p.Dispose()
"抓图尺寸: ${bw}x${bh}"
if ($bw -ne $ww -or $bh -ne $wh) {
  "FAIL: 抓图尺寸与窗口尺寸不等 —— DPI 虚拟化 / 坐标算错了"
  Stop-Process -Id $proc.Id -Force; exit 1
}
$green = Get-ColorCount "$outDir\settings_window.png" 0x07 0xC1 0x60
"绿色(微信绿)像素数: $green"
if ($green -lt 50) { "FAIL: 窗口里几乎找不到微信绿 —— 抓到的可能不是设置窗口"; Stop-Process -Id $proc.Id -Force; exit 1 }

# ---- 4. 复刻 Rust 侧 compute_layout，算出「复制到剪贴板的格式」下拉框的位置
$k = $phys.KGeom
$pad   = Get-Px 20  $k
$ppx   = Get-Px 18  $k
$panelW = (Get-Px 640 $k) - 2 * $pad
$innerX = $pad + $ppx
$innerW = $panelW - 2 * $ppx
$labelW = Get-Px 196 $k
$gap    = Get-Px 16  $k
$ctrlX  = $innerX + $labelW + $gap
$ctrlW  = $innerW - $labelW - $gap
$rowH   = Get-Px 30  $k
$rowGap = Get-Px 6   $k
$fieldH = Get-Px 30  $k
$rowsY  = $pad + (Get-Px 10 $k) + (Get-Px 30 $k)
$row2Y  = $rowsY + 2 * ($rowH + $rowGap)
$comboY = $row2Y + [int](($rowH - $fieldH) / 2)
"k=$k  下拉框客户区矩形: $ctrlX,$comboY ${ctrlW}x${fieldH}"

$org = New-Object Shot+POINT
[void][Shot]::ClientToScreen($win, [ref]$org)
"客户区屏幕原点: $($org.X),$($org.Y)"

# 点下拉框左侧文字（别点右侧箭头）
$clickX = $org.X + $ctrlX + 40
$clickY = $org.Y + $comboY + [int]($fieldH / 2)
"点击坐标(物理): $clickX,$clickY"

# 等窗口真正在最前：刚创建 / 刚置顶有竞态，点下去可能打到背后的 IDE
for ($i = 0; $i -lt 30 -and -not (Test-HitWindow $win $clickX $clickY); $i++) {
  Set-TopMost $win
  Start-Sleep -Milliseconds 150
}
if (-not (Test-HitWindow $win $clickX $clickY)) {
  "FAIL: 点击位置最顶层的不是设置窗口 —— 会被别的窗口挡住"
  Stop-Process -Id $proc.Id -Force; exit 1
}

# 点击并等待弹出（最多重试 3 次）
$pop = [IntPtr]::Zero
for ($try = 1; $try -le 3 -and $pop -eq [IntPtr]::Zero; $try++) {
  [Shot]::Click($clickX, $clickY)
  for ($i = 0; $i -lt 20 -and $pop -eq [IntPtr]::Zero; $i++) {
    Start-Sleep -Milliseconds 150
    $pop = [Shot]::Find('ScreenloomComboPopup')
  }
  if ($pop -eq [IntPtr]::Zero) { "  第 $try 次点击没弹出，重试..." }
}
if ($pop -eq [IntPtr]::Zero) { "FAIL: 下拉菜单没有弹出（点偏了？）"; Stop-Process -Id $proc.Id -Force; exit 1 }
$pr = Get-WindowRect $pop
$pw = $pr.R - $pr.L
$ph = $pr.B - $pr.T
$wantH = 3 * (Get-Px 30 $k) + (Get-Px 8 $k)
"弹出窗口: $($pr.L),$($pr.T) ${pw}x${ph}   期望高度(3 项): $wantH"
if ([Math]::Abs($ph - $wantH) -gt 1) {
  "FAIL: 弹出高度不对，点中的可能不是「复制格式」下拉框"
  Stop-Process -Id $proc.Id -Force; exit 1
}

# ---- 6. 抓「窗口 + 弹出菜单」的并集区域
$ux = [Math]::Min($wr.L, $pr.L)
$uy = [Math]::Min($wr.T, $pr.T)
$ux2 = [Math]::Max($wr.R, $pr.R)
$uy2 = [Math]::Max($wr.B, $pr.B)
Save-Screen $ux $uy ($ux2 - $ux) ($uy2 - $uy) "$outDir\settings_combo.png"

# 判据：菜单底色 #232830 必须出现在像素里（这个色只有菜单用）
$menuPx = Get-ColorCount "$outDir\settings_combo.png" 0x23 0x28 0x30
"菜单底色(#232830)像素数: $menuPx"
if ($menuPx -lt 500) {
  "FAIL: 图里找不到菜单底色 —— 弹出菜单没被真正抓到"
  Stop-Process -Id $proc.Id -Force; exit 1
}

Stop-Process -Id $proc.Id -Force
"=== 汇总 ==="
"整窗留证: $outDir\settings_window.png  (${bw}x${bh})"
"下拉留证: $outDir\settings_combo.png  (菜单底色像素 $menuPx)"
"ALL PASS"
exit 0
