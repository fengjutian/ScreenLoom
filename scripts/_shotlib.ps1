# 共享抓屏助手：Win32 定位窗口 + 真实屏幕抓取 + 精确颜色计数。
# 被 capture_settings_theme.ps1 / capture_theme_hotreload.ps1 点源（dot-source）使用。
#
# 判据全部是**二值、可外部复核**的：
#   - 抓图尺寸 == GetWindowRect 尺寸  -> 证明 1:1 物理像素（没被 DPI 虚拟化）
#   - 目标窗口/弹出窗口真的存在       -> 证明不是"看起来对"
#   - 位图里存在**只有该界面才有**的特征色像素数 -> 证明真画上去了

Add-Type -AssemblyName System.Drawing

if (-not ('Shot' -as [type])) {
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Shot {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }

  public static IntPtr Find(string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      StringBuilder sb = new StringBuilder(256);
      GetClassNameW(h, sb, 256);
      if (sb.ToString() == cls) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static string Title(IntPtr h) {
    StringBuilder sb = new StringBuilder(512);
    GetWindowTextW(h, sb, 512);
    return sb.ToString();
  }
  // 找**可见**且标题包含 needle 的窗口（标题忽略大小写）
  public static IntPtr FindByTitle(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      if (!IsWindowVisible(h)) return true;
      StringBuilder sb = new StringBuilder(512);
      GetWindowTextW(h, sb, 512);
      if (sb.ToString().IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  // 窗口客户区左上角在**屏幕**坐标里的位置
  public static POINT ClientOrigin(IntPtr h) {
    POINT p; p.X = 0; p.Y = 0;
    ClientToScreen(h, ref p);
    return p;
  }
  public static void Click(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(150);
    mouse_event(0x0002, 0, 0, 0, IntPtr.Zero); // LEFTDOWN
    System.Threading.Thread.Sleep(60);
    mouse_event(0x0004, 0, 0, 0, IntPtr.Zero); // LEFTUP
  }
  // 精确点击：先把光标移到目标点并**确认它真的在那儿**，再点击。
  //
  // 为什么不能只用 SetCursorPos + mouse_event：
  // SetCursorPos 是异步生效的，紧接着的 mouse_event 可能用**旧位置**投递，
  // 实测点击落在按钮左边 63 像素（表现为"HUD 被拖走"而不是点中按钮）。
  // 先确认光标到位，就把这一类漂移挡掉了。
  //
  // 也不用 SendInput 的绝对坐标：多显示器且虚拟屏原点在负数时归一化不可靠。
  public static void ClickExact(int x, int y) {
    for (int i = 0; i < 6; i++) {
      SetCursorPos(x, y);
      System.Threading.Thread.Sleep(120);
      POINT p; GetCursorPos(out p);
      if (p.X == x && p.Y == y) break;
    }
    mouse_event(0x0002, 0, 0, 0, IntPtr.Zero); // LEFTDOWN
    System.Threading.Thread.Sleep(60);
    mouse_event(0x0004, 0, 0, 0, IntPtr.Zero); // LEFTUP
  }
  public static string CursorPos() {
    POINT p; GetCursorPos(out p);
    return p.X + "," + p.Y;
  }
  // 往窗口的**客户区**坐标发一次点击（WM_LBUTTONDOWN + WM_LBUTTONUP）。
  //
  // 为什么不用真实鼠标：这台机器上可能同时有别的自动化在驱动鼠标，
  // 实测 SetCursorPos 之后光标会被抢走（目标 (1861,956) 实际落在
  // (1214,1026)），点击因此打在按钮外面 —— HUD 会把它当成"拖动窗口"，
  // 测试随机失败。窗口消息是确定性的，而且 HUD 的按钮逻辑本来就是
  // "拿客户区坐标判断点在不在按钮矩形里"，这条路径才是被测对象。
  public static void ClickClient(IntPtr h, int x, int y) {
    IntPtr lp = (IntPtr)((y << 16) | (x & 0xFFFF));
    PostMessageW(h, 0x0201, (IntPtr)1, lp);  // WM_LBUTTONDOWN
    System.Threading.Thread.Sleep(40);
    PostMessageW(h, 0x0202, (IntPtr)0, lp);  // WM_LBUTTONUP
  }
  // 真实鼠标拖拽：比 PostMessage 可靠 —— 合成消息会被**真实鼠标活动**插队，
  // 真实输入则只有这一个来源。
  public static void Drag(int x0, int y0, int x1, int y1) {
    SetCursorPos(x0, y0);
    System.Threading.Thread.Sleep(250);
    mouse_event(0x0002, 0, 0, 0, IntPtr.Zero); // LEFTDOWN
    System.Threading.Thread.Sleep(250);
    SetCursorPos(x1, y1);
    System.Threading.Thread.Sleep(300);
    mouse_event(0x0004, 0, 0, 0, IntPtr.Zero); // LEFTUP
    System.Threading.Thread.Sleep(250);
  }
  // 数一下 32bpp BGRA 里精确等于 (r,g,b) 的像素数
  public static int CountColor(byte[] buf, int r, int g, int b) {
    int n = 0;
    for (int i = 0; i + 3 < buf.Length; i += 4) {
      if (buf[i + 2] == r && buf[i + 1] == g && buf[i] == b) n++;
    }
    return n;
  }
}
"@
}

# 进程 DPI 感知（PER_MONITOR_AWARE_V2 = -4）。
# **必须在任何窗口创建之前**调用 —— 否则本进程是 DPI-unaware，
# GetWindowRect 返回的是**虚拟化**坐标（主屏 1.25 时窗口 818 会被报成 654），
# CopyFromScreen 抓到的也是缩小的图 → 证明不了 1:1 物理像素。
[void][Shot]::SetProcessDpiAwarenessContext([IntPtr](-4))

function Get-Px([int]$v, [double]$k) {
  return [int][Math]::Round($v * $k, [MidpointRounding]::AwayFromZero)
}

function Save-Screen([int]$x, [int]$y, [int]$w, [int]$h, [string]$path) {
  $bmp = New-Object System.Drawing.Bitmap($w, $h, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($x, $y, 0, 0, (New-Object System.Drawing.Size($w, $h)), [System.Drawing.CopyPixelOperation]::SourceCopy)
  $g.Dispose()
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  $bmp.Dispose()
}

function Get-ColorCount([string]$path, [int]$r, [int]$g, [int]$b) {
  $bmp = New-Object System.Drawing.Bitmap($path)
  $b32 = New-Object System.Drawing.Bitmap($bmp.Width, $bmp.Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gg = [System.Drawing.Graphics]::FromImage($b32)
  $gg.DrawImage($bmp, 0, 0)
  $gg.Dispose(); $bmp.Dispose()
  $rect = New-Object System.Drawing.Rectangle(0, 0, $b32.Width, $b32.Height)
  $data = $b32.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $len = [Math]::Abs($data.Stride) * $data.Height
  $buf = New-Object byte[] $len
  [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $buf, 0, $len)
  $b32.UnlockBits($data)
  $n = [Shot]::CountColor($buf, $r, $g, $b)
  $b32.Dispose()
  return $n
}

function Get-WindowRect([IntPtr]$h) {
  $r = New-Object Shot+RECT
  [void][Shot]::GetWindowRect($h, [ref]$r)
  return $r
}

function Set-TopMost([IntPtr]$h) {
  # HWND_TOPMOST = -1；SetForegroundWindow 会吃前台锁，置顶才稳
  [void][Shot]::SetWindowPos($h, [IntPtr](-1), 0, 0, 0, 0, 0x0001 -bor 0x0002 -bor 0x0040)
}

# 该屏幕坐标上最顶层的窗口是不是我们想要的那个。
# 用来确认「点击不会落到别的窗口上」—— 窗口刚创建 / 刚置顶时会有竞态，
# 这时候点下去可能打到背后的 IDE，下拉框自然弹不出来。
function Test-HitWindow([IntPtr]$h, [int]$x, [int]$y) {
  $p = New-Object Shot+POINT
  $p.X = $x; $p.Y = $y
  return ([Shot]::WindowFromPoint($p) -eq $h)
}

# 证明抓到的是 1:1 物理像素：
# 客户区宽（物理）应该 == 逻辑宽 * (窗口 DPI / 96)。
# 若本进程是 DPI-unaware，客户区宽会被虚拟化（偏小），两边就对不上 → 直接判失败。
function Test-PhysicalScale([IntPtr]$h, [double]$logicalClientW) {
  $cr = New-Object Shot+RECT
  [void][Shot]::GetClientRect($h, [ref]$cr)
  $cw = $cr.R - $cr.L
  $dpi = [Shot]::GetDpiForWindow($h)
  $kDpi = $dpi / 96.0
  $kGeom = $cw / $logicalClientW
  [pscustomobject]@{
    ClientW = $cw
    Dpi     = $dpi
    KDpi    = $kDpi
    KGeom   = $kGeom
    Ok      = ([Math]::Abs($kDpi - $kGeom) -lt 0.01)
  }
}

function Start-ScreenloomApp([string]$exe, [string]$cfg, [string[]]$extra) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $exe
  $psi.Arguments = "--config `"$cfg`" " + ($extra -join ' ')
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  return [System.Diagnostics.Process]::Start($psi)
}
