$ErrorActionPreference = 'Continue'

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class TrayProbe2 {
  [DllImport("user32.dll")] public static extern IntPtr SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  public const uint LD=0x02, LU=0x04;
  public static void Click(int x,int y){ SetCursorPos(x,y); System.Threading.Thread.Sleep(200); mouse_event(LD,0,0,0,IntPtr.Zero); System.Threading.Thread.Sleep(80); mouse_event(LU,0,0,0,IntPtr.Zero); }
}
"@
[void][TrayProbe2]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -AssemblyName System.Drawing

$out = 'D:\github\screenloom\e2e_out\tray_overflow_zoom.png'

[TrayProbe2]::Click(1507, 1050)
Start-Sleep -Milliseconds 1500

# 浮层 rect = (1361,777)-(1654,1020)，我们只要左下角那一列
$sx = 1361; $sy = 777; $sw = 293; $sh = 243
$bmp = New-Object System.Drawing.Bitmap $sw, $sh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($sx, $sy, 0, 0, $bmp.Size)
$g.Dispose()

# 放大 3 倍
$dst = New-Object System.Drawing.Bitmap ($sw*3), ($sh*3)
$g2 = [System.Drawing.Graphics]::FromImage($dst)
$g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$g2.DrawImage($bmp, 0, 0, $dst.Width, $dst.Height)
$g2.Dispose()
$dst.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$dst.Dispose(); $bmp.Dispose()
"saved: $out"

[TrayProbe2]::Click(700, 400)
