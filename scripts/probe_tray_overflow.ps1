$ErrorActionPreference = 'Continue'

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class TrayProbe {
  [DllImport("user32.dll")] public static extern IntPtr SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, System.Text.StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc p, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
  public const uint LD=0x02, LU=0x04;
  public static void Click(int x,int y){ SetCursorPos(x,y); System.Threading.Thread.Sleep(200); mouse_event(LD,0,0,0,IntPtr.Zero); System.Threading.Thread.Sleep(80); mouse_event(LU,0,0,0,IntPtr.Zero); }
  public static System.Collections.Generic.List<string> Dumps() {
    var res = new System.Collections.Generic.List<string>();
    EnumWindows((h,l) => {
      var sb = new System.Text.StringBuilder(256); GetClassNameW(h, sb, 256);
      var c = sb.ToString();
      if (c.Contains("Overflow") || c.Contains("NotifyIcon") || c.Contains("TrayNotify")) {
        RECT r; GetWindowRect(h, out r);
        res.Add("WIN class=" + c + " rect=(" + r.L + "," + r.T + ")-(" + r.R + "," + r.B + ")");
        EnumChildWindows(h, (ch,cl) => {
          var sb2 = new System.Text.StringBuilder(256); GetClassNameW(ch, sb2, 256);
          RECT r2; GetWindowRect(ch, out r2);
          res.Add("   child class=" + sb2 + " rect=(" + r2.L + "," + r2.T + ")-(" + r2.R + "," + r2.B + ")");
          return true;
        }, IntPtr.Zero);
      }
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@

# DPI 感知（必须在建窗口前）
[void][TrayProbe]::SetProcessDpiAwarenessContext([IntPtr](-4))

Add-Type -AssemblyName System.Drawing

$out = 'D:\github\screenloom\e2e_out\tray_overflow.png'

# 1) 点 chevron（物理坐标 1507,1050）
[TrayProbe]::Click(1507, 1050)
Start-Sleep -Milliseconds 1500

"== 溢出窗口 =="
[TrayProbe]::Dumps() | Out-String -Width 200

# 2) 抓 chevron 上方 420x340
$sx = 1480 - 420
$sy = 1080 - 340
$bmp = New-Object System.Drawing.Bitmap 420, 340
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($sx, $sy, 0, 0, $bmp.Size)
$g.Dispose()
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
"saved: $out"

# 3) 点别处收起
[TrayProbe]::Click(700, 400)
