$ErrorActionPreference = 'Continue'

Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class EnumKids {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
  public static System.Collections.Generic.List<string> Kids(IntPtr parent) {
    var res = new System.Collections.Generic.List<string>();
    RECT pr; GetWindowRect(parent, out pr);
    res.Add("PARENT rect=(" + pr.L + "," + pr.T + ")-(" + pr.R + "," + pr.B + ")");
    EnumChildWindows(parent, (h,l) => {
      var sb = new System.Text.StringBuilder(256); GetClassNameW(h, sb, 256);
      var tb = new System.Text.StringBuilder(512); GetWindowTextW(h, tb, 512);
      RECT r; GetWindowRect(h, out r);
      res.Add(String.Format("{0,-12} text='{1}' screen=({2},{3})-({4},{5}) size={6}x{7}",
        sb, tb, r.L, r.T, r.R, r.B, r.R-r.L, r.B-r.T));
      return true;
    }, IntPtr.Zero);
    return res;
  }
}
"@

$h = [IntPtr]::Zero
$cb = [EnumKids+EnumProc]{
  param($wh, $l)
  $sb = New-Object System.Text.StringBuilder 256
  [void][EnumKids]::GetClassNameW($wh, $sb, 256)
  if ($sb.ToString() -eq 'ScreenloomSettingsWindow') {
    $r = New-Object EnumKids+RECT
    [void][EnumKids]::GetWindowRect($wh, [ref]$r)
    if ($r.L -gt -30000) { $script:h = $wh; return $false }
  }
  return $true
}
[void][EnumKids]::EnumWindows($cb, [IntPtr]::Zero)
$h = $script:h
if ($h -eq [IntPtr]::Zero) { "找不到可见的设置窗口"; exit }
[EnumKids]::Kids($h) | Out-String -Width 220
