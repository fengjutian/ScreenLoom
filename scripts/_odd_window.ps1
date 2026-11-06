# 造一个**奇数尺寸**的窗口，供录屏回归用（scripts/e2e_record.ps1 的 D 用例）。
#
# 为什么需要它：H.264/NV12 的色度是 2×2 降采样，宽高必须是偶数。
# 奇数尺寸的窗口（521x425 这种）非常常见，之前会**直接录不了**：
# 捕获层报错 → 降级 GDI → 编码器再报错 → 零产物。
#
# 本脚本在 stdout 打印窗口的**物理像素**尺寸，方便 e2e 断言与排查。
#
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File scripts\_odd_window.ps1

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class OddWin {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
}
'@

$f = New-Object System.Windows.Forms.Form
$f.Text = 'ScreenloomOddProbe'
$f.FormBorderStyle = 'FixedToolWindow'
$f.StartPosition = 'Manual'
$f.Location = New-Object System.Drawing.Point(40, 40)
# 故意用奇数：401x301（逻辑像素）
$f.ClientSize = New-Object System.Drawing.Size(401, 301)
$f.BackColor = [System.Drawing.Color]::FromArgb(20, 120, 200)

$lbl = New-Object System.Windows.Forms.Label
$lbl.Text = 'ODD SIZE PROBE'
$lbl.Font = New-Object System.Drawing.Font('Segoe UI', 20)
$lbl.ForeColor = [System.Drawing.Color]::White
$lbl.AutoSize = $true
$lbl.Location = New-Object System.Drawing.Point(20, 20)
$f.Controls.Add($lbl)

$f.Show()

# 打印物理像素尺寸：客户端区 + 窗口矩形
$h = $f.Handle
$cr = New-Object OddWin+RECT
$wr = New-Object OddWin+RECT
[void][OddWin]::GetClientRect($h, [ref]$cr)
[void][OddWin]::GetWindowRect($h, [ref]$wr)
$cw = $cr.R - $cr.L
$ch = $cr.B - $cr.T
Write-Host ("ODDWIN client={0}x{1} window={2}x{3}" -f $cw, $ch, ($wr.R - $wr.L), ($wr.B - $wr.T))
[Console]::Out.Flush()

[System.Windows.Forms.Application]::Run($f)
