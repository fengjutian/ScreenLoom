$ErrorActionPreference = 'Continue'

# 直接用 Win32 消息驱动设置窗口：
# 找「保存」按钮 → BM_CLICK → 校验配置文件
#   注意：设置窗口的按钮是**自绘窗口类** ScreenloomFlatButton（不是系统 BUTTON），
#   但它照样响应 BM_CLICK。
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class SettingsDrive {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, string lp);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetWindowTextW(IntPtr h, string s);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")]
  public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll", EntryPoint="SendMessageW")]
  public static extern IntPtr SendMessageRaw(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  public static string MsgText(IntPtr h) {
    var sb = new StringBuilder(1024);
    SendMessageStr(h, 0x000D, (IntPtr)1024, sb); // WM_GETTEXT
    return sb.ToString();
  }
  // 跨进程写 EDIT 内容：必须用显式 native buffer，
  // user32 会对 WM_SETTEXT 做跨进程字符串编组。
  public static bool SetEditText(IntPtr h, string s) {
    IntPtr buf = Marshal.StringToHGlobalUni(s);
    try {
      SendMessageRaw(h, 0x000C, IntPtr.Zero, buf); // WM_SETTEXT
      return true;
    } finally {
      Marshal.FreeHGlobal(buf);
    }
  }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
  public static IntPtr SettingsWindow() {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h,l) => {
      var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
      if (sb.ToString() == "ScreenloomSettingsWindow") { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static System.Collections.Generic.List<IntPtr> Kids(IntPtr parent, string cls) {
    var res = new System.Collections.Generic.List<IntPtr>();
    EnumChildWindows(parent, (h,l) => {
      var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
      if (sb.ToString() == cls) res.Add(h);
      return true;
    }, IntPtr.Zero);
    return res;
  }
  public static string TextOf(IntPtr h) {
    var sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString();
  }
}
"@

$cfg = "$env:TEMP\screenloom_e2e_cfg.txt"
if (Test-Path $cfg) { Remove-Item $cfg -Force }

$p = Start-Process -FilePath 'D:\github\screenloom\target\release\screenloom.exe' `
     -ArgumentList '--config', $cfg, '--settings' -PassThru
Start-Sleep -Seconds 2

$win = [SettingsDrive]::SettingsWindow()
if ($win -eq [IntPtr]::Zero) { "找不到设置窗口"; Stop-Process -Id $p.Id -Force; exit 1 }
"设置窗口句柄: $win"

$edits = [SettingsDrive]::Kids($win, 'Edit')
"EDIT 控件数: $($edits.Count)"
if ($edits.Count -lt 1) { "没有 EDIT 控件"; Stop-Process -Id $p.Id -Force; exit 1 }

# 第一个 EDIT = 截图热键
$hk = $edits.Item(0)
"原热键(窗口标题): '$([SettingsDrive]::TextOf($hk))'"
"原热键(WM_GETTEXT): '$([SettingsDrive]::MsgText($hk))'"
# 跨进程改 EDIT 内容：必须用 native buffer 的 WM_SETTEXT
# （SetWindowTextW 跨进程只改 caption，不改 EDIT 内容）
[void][SettingsDrive]::SetEditText($hk, 'Ctrl+Shift+F12')
Start-Sleep -Milliseconds 300
"新热键(WM_GETTEXT): '$([SettingsDrive]::MsgText($hk))'"

# 第三个 EDIT = 长截图最大长度，改成 8888
if ($edits.Count -ge 3) {
  $lm = $edits.Item(2)
  "原长截图上限(WM_GETTEXT): '$([SettingsDrive]::MsgText($lm))'"
  [void][SettingsDrive]::SetEditText($lm, '8888')
  "新长截图上限(WM_GETTEXT): '$([SettingsDrive]::MsgText($lm))'"
}

# 找「保存」按钮（自绘按钮窗口类）
$btns = [SettingsDrive]::Kids($win, 'ScreenloomFlatButton')
$save = [IntPtr]::Zero
foreach ($b in $btns) {
  if ([SettingsDrive]::TextOf($b) -eq '保存') { $save = $b; break }
}
if ($save -eq [IntPtr]::Zero) { "找不到保存按钮"; Stop-Process -Id $p.Id -Force; exit 1 }
"点「保存」..."
[void][SettingsDrive]::SendMessageW($save, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)  # BM_CLICK
Start-Sleep -Seconds 2

"进程已退出: $($p.HasExited)"
"--- 配置文件 ($cfg) ---"
if (Test-Path $cfg) { Get-Content $cfg | Out-String } else { "配置文件不存在！" }
