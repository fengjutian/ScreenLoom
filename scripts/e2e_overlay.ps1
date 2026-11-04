# 端到端验证脚本（Windows PowerShell 5.1）
#
# 覆盖三条链路：
#   A. 确定性截图：--sel x,y,w,h → 存盘 PNG 尺寸必须与请求的物理像素 1:1 相等
#   B. 跨显示器选区：选区横跨两块屏 → 同样校验尺寸（验证虚拟屏坐标合成没错位）
#   C. 交互模式：启动浮窗 → 合成拖拽 → 抓整块虚拟屏留证（人工看浮窗是否真画在屏幕上）
#
# C 段只做「留证」，不做硬断言：这台机器上有真实鼠标活动，
# 合成消息可能被真实输入插队。坐标正确性由 A/B 段客观断言。
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\screenloom\scripts\e2e_overlay.ps1

$ErrorActionPreference = 'Stop'

$Root      = 'D:\github\screenloom'
$Exe       = Join-Path $Root 'target\release\screenloom.exe'
$Manifest  = Join-Path $Root 'Cargo.toml'
$CfgPath   = Join-Path $Root 'e2e_config.txt'
$OutDir    = Join-Path $Root 'e2e_out'
$SaveDir   = Join-Path $env:USERPROFILE 'Pictures\Screenloom'

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
Get-Process screenloom -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

Add-Type -AssemblyName System.Drawing

# 显示器枚举（要**物理**坐标，所以脚本进程必须先切到 Per-Monitor V2）
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class Disp {
  public delegate bool MonitorEnumProc(IntPtr h, IntPtr d);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc cb, IntPtr data);
  [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr h, ref MONITORINFO mi);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO { public int cbSize; public RECT rcMonitor; public RECT rcWork; public int dwFlags; }
  public static List<RECT> Monitors() {
    var list = new List<RECT>();
    EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, delegate(IntPtr h, IntPtr d) {
      MONITORINFO mi = new MONITORINFO();
      mi.cbSize = Marshal.SizeOf(typeof(MONITORINFO));
      if (GetMonitorInfo(h, ref mi)) list.Add(mi.rcMonitor);
      return true;
    }, IntPtr.Zero);
    return list;
  }
}
"@
[void][Disp]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Get-LatestShot {
  Get-ChildItem $SaveDir -Filter '*.png' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

function Test-Region([int]$x, [int]$y, [int]$w, [int]$h, [string]$label) {
  Write-Host "  → $label : 请求 ${x},${y} ${w}x${h}"
  $before = Get-LatestShot
  $out = & $Exe --sel "$x,$y,$w,$h" --config $CfgPath 2>&1 | Out-String
  if ($LASTEXITCODE -ne 0) { Write-Host "    !! 退出码 $LASTEXITCODE"; Write-Host $out; return $false }
  $shot = Get-LatestShot
  if (-not $shot) { Write-Host '    !! 没有产生存盘文件'; return $false }
  if ($shot.LastWriteTime -le $before.LastWriteTime) { Write-Host '    !! 存盘文件没有更新'; return $false }

  $bmp = [System.Drawing.Image]::FromFile($shot.FullName)
  $gotW = $bmp.Width; $gotH = $bmp.Height
  $bmp.Dispose()

  # 非空白校验：纯黑/纯白的比例不能是 100%
  $b2 = New-Object System.Drawing.Bitmap $shot.FullName
  $samples = 0; $nonBlank = 0
  for ($sy = 0; $sy -lt $b2.Height; $sy += 17) {
    for ($sx = 0; $sx -lt $b2.Width; $sx += 17) {
      $c = $b2.GetPixel($sx, $sy); $samples++
      if (-not ($c.R -eq $c.G -and $c.G -eq $c.B -and ($c.R -eq 0 -or $c.R -eq 255))) { $nonBlank++ }
    }
  }
  $b2.Dispose()

  Write-Host "    存盘: $($shot.FullName)  尺寸 ${gotW}x${gotH}  非空白采样 $nonBlank/$samples"
  if ($gotW -ne $w -or $gotH -ne $h) { Write-Host "    !! 尺寸不匹配（期望 ${w}x${h}）"; return $false }
  if ($samples -gt 0 -and $nonBlank -eq 0) { Write-Host '    !! 画面是纯色，疑似抓到空屏'; return $false }
  Write-Host '    OK：尺寸与物理像素 1:1，画面非空'
  return $true
}

Write-Host '=== A. 单屏选区 ==='
$okA = Test-Region 100 200 800 600 '主屏左上角'

Write-Host ''
Write-Host '=== B. 跨显示器选区（横跨两块屏）==='
# 跨屏断言的**硬件前提**是「至少两块屏」。只有一块屏时这项没法做，
# 应报 SKIP 而不是 FAIL —— 否则拔掉副屏就变成"代码坏了"，误导。
$mons = [Disp]::Monitors()
Write-Host "  检测到 $($mons.Count) 块显示器"
$okB = $null
if ($mons.Count -ge 2) {
  # 取前两块屏的并集，往里缩 100px 做选区（保证完全落在虚拟屏内）
  $a = $mons[0]; $b = $mons[1]
  $x0 = [Math]::Min($a.L, $b.L) + 100
  $y0 = [Math]::Min($a.T, $b.T) + 100
  $x1 = [Math]::Max($a.R, $b.R) - 100
  $y1 = [Math]::Max($a.B, $b.B) - 100
  $okB = Test-Region $x0 $y0 ($x1 - $x0) ($y1 - $y0) "横跨第 1、2 块屏"
} else {
  Write-Host '  -- SKIP：当前只有 1 块显示器，无法做跨屏断言（不是代码问题）'
}

Write-Host ''
Write-Host '=== C. 交互模式（浮窗渲染留证）==='
$src = @'
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
'@
Add-Type -TypeDefinition $src -Language CSharp

$WM_LBUTTONDOWN = 0x0201; $WM_LBUTTONUP = 0x0202; $WM_MOUSEMOVE = 0x0200
function Make-LParam([int]$x, [int]$y) {
  return [IntPtr](($y -band 0xFFFF) -shl 16 -bor ($x -band 0xFFFF))
}

$proc = Start-Process -FilePath $Exe -ArgumentList @('--once', '--config', $CfgPath) `
  -PassThru -RedirectStandardOutput (Join-Path $OutDir 'app.out.log') `
  -RedirectStandardError  (Join-Path $OutDir 'app.err.log')
$hwnd = [IntPtr]::Zero
$deadline = (Get-Date).AddSeconds(25)
while ((Get-Date) -lt $deadline -and $hwnd -eq [IntPtr]::Zero) {
  $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
  if ($p -and $p.MainWindowHandle -ne 0 -and $p.MainWindowTitle -eq 'screenloom overlay') { $hwnd = $p.MainWindowHandle }
  Start-Sleep -Milliseconds 250
}
if ($hwnd -eq [IntPtr]::Zero) {
  Write-Host '  !! 没等到浮窗窗口'
} else {
  Write-Host "  浮窗句柄 = $hwnd"
  [void][W]::SetForegroundWindow($hwnd)
  Start-Sleep -Milliseconds 600
  # 浮窗铺满虚拟屏，客户区 (0,0) = 虚拟屏左上角
  [void][W]::PostMessageW($hwnd, $WM_LBUTTONDOWN, [IntPtr]1, (Make-LParam 1400 300))
  Start-Sleep -Milliseconds 150
  [void][W]::PostMessageW($hwnd, $WM_MOUSEMOVE, [IntPtr]0, (Make-LParam 2600 1100))
  Start-Sleep -Milliseconds 150
  [void][W]::PostMessageW($hwnd, $WM_LBUTTONUP, [IntPtr]0, (Make-LParam 2600 1100))
  Start-Sleep -Milliseconds 700
  & cargo run --quiet --manifest-path $Manifest -p screenloom-capture --example capture_virtual -- `
      (Join-Path $OutDir 'screen_with_selection.png') | Out-Null
  Write-Host "  留证: $(Join-Path $OutDir 'screen_with_selection.png')（选区边框 + 区外压暗应可见）"
  # 收尾：ESC 取消
  [void][W]::PostMessageW($hwnd, 0x0100, [IntPtr]0x1B, [IntPtr]0)
  Start-Sleep -Milliseconds 800
  if (-not $proc.HasExited) { $proc | Stop-Process -Force -ErrorAction SilentlyContinue }
}

Write-Host ''
Write-Host '=== 汇总 ==='
Write-Host "  A 单屏选区:      $(if ($okA) { 'PASS' } else { 'FAIL' })"
Write-Host "  B 跨屏选区:      $(if ($null -eq $okB) { 'SKIP (显示器不足 2 块)' } elseif ($okB) { 'PASS' } else { 'FAIL' })"
if (-not $okA) { exit 1 }
if ($null -ne $okB -and -not $okB) { exit 1 }
exit 0
