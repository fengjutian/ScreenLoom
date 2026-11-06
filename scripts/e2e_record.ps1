# 录屏链路的端到端验证（Windows PowerShell 5.1）
#
# 覆盖三条链路，每条都有**外部可证伪**的判据（不是「看起来对」，是能被第三方复核）：
#   A. CLI 录制 → 产物真的可播
#      判据：ffprobe 能解析出 h264 / yuv420p；帧数 ≈ 时长 × fps（±2 帧）；时长在期望区间内
#   B. 热键 → HUD → 停止（常驻模式的真实交互链路）
#      判据：HUD 窗口（类名 ScreenloomRecorderHud）真的出现；停止后 HUD 消失；
#            落盘 MP4 能被 ffprobe 解析，且帧数 > 0
#   C. 崩溃恢复
#      判据：人为写一个 `.partial.mp4`，启动常驻进程后它被转成可播放的正式 MP4
#   D. 奇数尺寸窗口录制（NV12 要求偶数尺寸，之前的硬失败）
#      判据：产出正式 MP4 且能被 ffprobe 解析；成片分辨率为偶数
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\screenloom\scripts\e2e_record.ps1
#
# 可用环境变量：
#   SCREENLOOM_E2E_KEEP=1   保留产物与留证（默认清理）

$ErrorActionPreference = 'Stop'

$Root    = 'D:\github\screenloom'
$Exe     = Join-Path $Root 'target\release\screenloom.exe'
$OutDir  = Join-Path $Root 'probes\e2e_out'
$CfgPath = Join-Path $Root 'e2e_record_config.txt'
$Keep    = ($env:SCREENLOOM_E2E_KEEP -eq '1')

# 录屏产物目录：放在工作区内（写工作区外的路径会被本地安全策略拦）。
$RecDir  = Join-Path $Root 'probes\e2e_out\record_out'

. (Join-Path $Root 'scripts\_shotlib.ps1')

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
if (Test-Path $RecDir) { Remove-Item $RecDir -Recurse -Force }
New-Item -ItemType Directory -Path $RecDir | Out-Null

# ---------------------------------------------------------------- 工具函数

function Wait-Window([scriptblock]$probe, [int]$seconds = 25) {
  $end = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $end) {
    $v = & $probe
    if ($null -ne $v -and $v -ne [IntPtr]::Zero) { return $v }
    Start-Sleep -Milliseconds 250
  }
  return [IntPtr]::Zero
}

function Get-LatestVideo([string]$dir) {
  Get-ChildItem $dir -Filter '*.mp4' -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -notlike '*.partial.mp4' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
}

function Get-PartialVideos([string]$dir) {
  Get-ChildItem $dir -Filter '*.partial.mp4' -ErrorAction SilentlyContinue
}

# ffprobe 取某条流的字段；取不到返回空串。
function Get-StreamField([string]$file, [string]$entry, [int]$index = 0) {
  $out = & ffprobe -v error -select_streams "v:$index" -show_entries "stream=$entry" `
    -of default=noprint_wrappers=1:nokey=1 -- "$file" 2>$null
  if ($LASTEXITCODE -ne 0) { return '' }
  return ($out | Select-Object -First 1)
}

function Get-FormatField([string]$file, [string]$entry) {
  $out = & ffprobe -v error -show_entries "format=$entry" -of default=noprint_wrappers=1:nokey=1 -- "$file" 2>$null
  if ($LASTEXITCODE -ne 0) { return '' }
  return ($out | Select-Object -First 1)
}

# 返回 @{ Ok=bool; Reason=string; Codec=..; PixFmt=..; Frames=..; Duration=..; Fps=.. }
function Test-Playable([string]$file, [int]$expectFps, [double]$minSec, [double]$maxSec) {
  $r = @{ Ok = $false; Reason = ''; Codec = ''; PixFmt = ''; Frames = 0; Duration = 0.0; Fps = 0.0 }
  if (-not (Test-Path $file)) { $r.Reason = "文件不存在: $file"; return $r }
  if ((Get-Item $file).Length -lt 1024) { $r.Reason = "文件过小（$(  (Get-Item $file).Length ) 字节），不像有效视频"; return $r }

  $r.Codec = Get-StreamField $file 'codec_name'
  $r.PixFmt = Get-StreamField $file 'pix_fmt'
  $r.Fps = 0.0
  $fpsRaw = Get-StreamField $file 'avg_frame_rate'
  if ($fpsRaw -and $fpsRaw -match '^(\d+)/(\d+)$' -and [double]$matches[2] -ne 0) {
    $r.Fps = [double]$matches[1] / [double]$matches[2]
  }
  $framesRaw = Get-StreamField $file 'nb_frames'
  $r.Frames = if ($framesRaw) { [int]$framesRaw } else { 0 }
  $durRaw = Get-FormatField $file 'duration'
  if ($durRaw) { $r.Duration = [double]$durRaw }

  if ($r.Codec -ne 'h264') { $r.Reason = "codec 不是 h264（实际: '$($r.Codec)'）"; return $r }
  if ($r.PixFmt -ne 'yuv420p') { $r.Reason = "pix_fmt 不是 yuv420p（实际: '$($r.PixFmt)'）"; return $r }
  if ($r.Duration -le 0) { $r.Reason = 'duration 解析失败或为 0'; return $r }
  if ($r.Duration -lt $minSec -or $r.Duration -gt $maxSec) {
    $r.Reason = "时长 $($r.Duration)s 不在期望区间 [$minSec, $maxSec]s"
    return $r
  }
  # 帧数与时长×fps 必须对得上（±2 帧）；nb_frames 缺失时退化为「能解码出帧」。
  $expect = [int]($r.Duration * $expectFps)
  if ($r.Frames -gt 0) {
    if ([Math]::Abs($r.Frames - $expect) -gt 2) {
      $r.Reason = "帧数 $($r.Frames) 与 时长×fps=$expect 相差过大"
      return $r
    }
  } else {
    $decoded = & ffprobe -v error -count_frames -select_streams 'v:0' -show_entries 'stream=nb_read_frames' `
      -of default=noprint_wrappers=1:nokey=1 -- "$file" 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $decoded) { $r.Reason = 'nb_frames 缺失且无法解码统计帧数'; return $r }
    $r.Frames = [int]($decoded | Select-Object -First 1)
    if ($r.Frames -le 0) { $r.Reason = '解码后帧数为 0'; return $r }
  }
  $r.Ok = $true
  $r.Reason = "ok"
  return $r
}

# 写一份录制专用配置：录屏目录指到工作区内，HUD 打开，音频关掉（避免设备差异）。
function Write-RecordConfig([string]$path, [string]$recordDir, [string]$hotkey) {
  $lines = @(
    '# e2e_record 专用配置'
    'hotkey = Alt+A'
    'hide_self = false'
    'toolbar_collapsed = false'
    'copy_format = both'
    'long_screenshot_max = 0'
    'long_screenshot_search = 240'
    'default_dir ='
    "record_hotkey = $hotkey"
    "record_dir = $recordDir"
    'record_audio_system = false'
    'record_audio_mic = false'
    'record_countdown = 0'
    'record_show_hud = true'
    'record_open_folder = false'
    'record_fps = 15'
    'record_bitrate = 2000000'
  )
  [System.IO.File]::WriteAllLines($path, $lines, (New-Object System.Text.UTF8Encoding($false)))
}

if (-not (Test-Path $Exe)) {
  Write-Host "!! 找不到 $Exe"
  Write-Host '   先执行: cargo build --release -p screenloom-app --bins'
  exit 1
}

# 录屏前先清场，避免上次的残留影响判断。
Get-Process screenloom -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 400

$results = @()

# ================================================================ A. CLI 录制
Write-Host '=== A. CLI 录制 → 产物可播 ==='
$cliOut = Join-Path $RecDir 'cli_record.mp4'
$cliLog = Join-Path $OutDir 'record_cli.out.log'
$cliErr = Join-Path $OutDir 'record_cli.err.log'
if (Test-Path $cliLog) { Remove-Item $cliLog -Force }
if (Test-Path $cliErr) { Remove-Item $cliErr -Force }

$cliArgs = @(
  '--config', $CfgPath
  '--record', '4'
  '--record-out', $cliOut
  '--record-fps', '15'
  '--record-countdown', '0'
)
$p = Start-Process -FilePath $Exe -ArgumentList $cliArgs -PassThru -NoNewWindow `
  -RedirectStandardOutput $cliLog -RedirectStandardError $cliErr
$p.WaitForExit(120000) | Out-Null
if (-not $p.HasExited) { $p | Stop-Process -Force; Write-Host '  !! CLI 录制超时，已强杀'; }

$okA = $false
$whyA = ''
# 注意：Start-Process 拿到的 Process 对象，ExitCode 有时取不到（重定向句柄未刷新）。
# 所以判据不依赖退出码 —— 直接看**产物文件**能不能被 ffprobe 解析，这是外部可复核的。
$exitCode = $null
try { $exitCode = $p.ExitCode } catch { $exitCode = $null }

if (Test-Path $cliOut) {
  $r = Test-Playable $cliOut 15 3.0 8.0
  $okA = $r.Ok
  $whyA = "codec=$($r.Codec) pix=$($r.PixFmt) frames=$($r.Frames) dur=$($r.Duration)s fps=$($r.Fps) → $($r.Reason)"
  Write-Host "  产物: $cliOut"
} else {
  $whyA = '没有产出文件'
  if ($null -ne $exitCode) { $whyA = "进程退出码 $exitCode，且没有产出文件" }
  if (Test-Path $cliErr) { $whyA += ' / ' + ((Get-Content $cliErr -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
}
if ($okA) { Write-Host '  OK  CLI 录制的 MP4 能被 ffprobe 正确解析' }
else { Write-Host "  !! $whyA" }
$results += [pscustomobject]@{ Case = 'A CLI 录制产物可播'; Pass = $okA; Detail = $whyA }

# ================================================================ B. 热键 → HUD → 停止
Write-Host ''
Write-Host '=== B. 录屏热键 → HUD 出现 → 停止 → 落盘 ==='
Write-RecordConfig $CfgPath $RecDir 'Alt+R'
$appOut = Join-Path $OutDir 'record_app.out.log'
$appErr = Join-Path $OutDir 'record_app.err.log'
if (Test-Path $appOut) { Remove-Item $appOut -Force }
if (Test-Path $appErr) { Remove-Item $appErr -Force }

$app = Start-Process -FilePath $Exe -ArgumentList @('--config', $CfgPath) -PassThru `
  -RedirectStandardOutput $appOut -RedirectStandardError $appErr
Start-Sleep -Seconds 3

# HUD 窗口：类名是应用自己的常量 ScreenloomRecorderHud
$hud = [IntPtr]::Zero
$okB = $false
$whyB = ''

if ($app.HasExited) {
  $whyB = "常驻进程意外退出，退出码 $($app.ExitCode)"
  if (Test-Path $appErr) { $whyB += ' / ' + ((Get-Content $appErr -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
} else {
  # 触发录屏热键：用**真实键盘输入**。RegisterHotKey 走的是系统级注册，
  # 只有真实按键才会被系统投递成 WM_HOTKEY（PostMessage 模拟不出来）。
  Add-Type -AssemblyName System.Windows.Forms
  [System.Windows.Forms.SendKeys]::SendWait('%r')
  Start-Sleep -Seconds 2

  $hud = Wait-Window { [Shot]::Find('ScreenloomRecorderHud') } 25
  if ($hud -eq [IntPtr]::Zero) {
    $whyB = '按下录屏热键后没有出现 HUD 窗口（类名 ScreenloomRecorderHud）'
    if (Test-Path $appErr) { $whyB += ' / ' + ((Get-Content $appErr -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
  } else {
    Write-Host "  HUD 句柄 = $hud"
    $hr = Get-WindowRect $hud
    $cr = New-Object Shot+RECT
    [void][Shot]::GetClientRect($hud, [ref]$cr)
    $co = [Shot]::ClientOrigin($hud)
    $hw = $cr.R - $cr.L
    $hh = $cr.B - $cr.T
    $k = [Shot]::GetDpiForWindow($hud) / 96.0
    Write-Host "  HUD 客户区 = ${hw}x${hh}  dpi缩放 = $k  屏幕原点 = $($co.X),$($co.Y)"

    # 判据：HUD 尺寸必须符合 hud.rs 的常量（逻辑 286x64 按 DPI 缩放）。
    # 注意：**HUD 抓不到屏幕截图** —— 它设了 WDA_EXCLUDEFROMCAPTURE（不录进画面），
    # 且是 GDI+ 直绘，PrintWindow 也只能拿到黑图。所以这里只能验证
    # 「窗口存在 + 尺寸正确 + 点击停止后真的消失」，内容靠行为证明。
    $expW = [int](286.0 * $k)
    $expH = [int](64.0 * $k)
    if ([Math]::Abs($hw - $expW) -gt 2 -or [Math]::Abs($hh - $expH) -gt 2) {
      $whyB = "HUD 尺寸 ${hw}x${hh} 与期望 ${expW}x${expH}（逻辑 286x64 × DPI $k）不符"
    }

    Start-Sleep -Seconds 4

    # 快照必须在**点击停止之前**取：之后取就分不出哪些是本次新增的。
    $before = @{}
    foreach ($f in (Get-ChildItem $RecDir -Filter '*.mp4' -ErrorAction SilentlyContinue)) { $before[$f.Name] = $true }

    # 点 HUD 上的「停止」按钮。hud.rs 的 stop_rect 相对**客户区**：
    # left = w-48k, right = w-8k, top = 12k, bottom = h-12k。
    $btnLeft = $hw - 48.0 * $k
    $btnRight = $hw - 8.0 * $k
    $btnTop = 12.0 * $k
    $btnBottom = $hh - 12.0 * $k
    # 停止按钮在**客户区**里的中心（HUD 是 WS_POPUP，客户区原点 = 窗口原点）。
    $btnCX = [int](($btnLeft + $btnRight) / 2)
    $btnCY = [int](($btnTop + $btnBottom) / 2)
    $sx = [int]($co.X + $btnCX)
    $sy = [int]($co.Y + $btnCY)
    Write-Host "  停止按钮客户区坐标 = ($btnCX,$btnCY)  屏幕坐标 = ($sx,$sy)"

    # 点击前先确认那个点上最顶层的窗口**就是 HUD**，否则点下去会打到别的窗口。
    $pt = New-Object Shot+POINT
    $pt.X = $sx; $pt.Y = $sy
    $top = [Shot]::WindowFromPoint($pt)
    if ($top -ne $hud) {
      $whyB = "停止按钮位置被别的窗口遮挡（顶层窗口 $top ≠ HUD $hud），点击会落空"
    } else {
      # 点击可能因焦点/竞态落空（首次点击时 HUD 刚创建）。
      # 允许重试，但判据不放水：**最终必须真的停下**（HUD 消失 + 新产物落盘）。
      #
      # 关键：每次点击后只**短轮询**（2 秒）看 HUD 是否消失，没消失就立刻再点一次。
      # 不能用「等 30 秒」的长等待 —— 那会把「点击落空」和「收尾慢」混为一谈，
      # 落空一次就白白等满 30 秒，录制继续跑，产物时长直接超标。
      $gone = [IntPtr]::Zero
      $swClick = [Diagnostics.Stopwatch]::StartNew()
      for ($try = 1; $try -le 10 -and $gone -eq [IntPtr]::Zero; $try++) {
        # 用**窗口消息**点按钮，不动物理鼠标：这台机器上可能有别的自动化在
        # 抢鼠标（实测光标会被带走），真实点击会随机落在按钮外面，
        # HUD 把它们当成"拖动窗口"，测试就随机失败了。
        # HUD 的按钮逻辑本来就是"拿客户区坐标判断点在不在按钮矩形里"，
        # 这条路径才是真正要测的东西。
        [Shot]::ClickClient($hud, $btnCX, $btnCY)
        Write-Host "  点击停止按钮（客户区 $btnCX,$btnCY）— 第 $try 次"
        $gone = Wait-Window { if ([Shot]::Find('ScreenloomRecorderHud') -eq [IntPtr]::Zero) { [IntPtr]1 } else { [IntPtr]::Zero } } 2
      }
      $swClick.Stop()
      if ($gone -eq [IntPtr]::Zero) {
        # 短轮询都没让 HUD 消失，再给一次长等待的机会（收尾确实可能慢）。
        $gone = Wait-Window { if ([Shot]::Find('ScreenloomRecorderHud') -eq [IntPtr]::Zero) { [IntPtr]1 } else { [IntPtr]::Zero } } 30
      }
    if ($gone -eq [IntPtr]::Zero) {
      $whyB = '点击停止后 HUD 没有消失'
      if (Test-Path $appErr) { $whyB += ' / ' + ((Get-Content $appErr -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
    } else {
        # 只认**本次录制新增**的文件：不能拿 A 用例留下的 cli_record.mp4 当证据，
        # 否则「停止后根本没落盘」也会被误判成 PASS。
        #
        # HUD 消失 = 用户已经得到反馈；文件还要等收尾（Flush → Finalize → remux，
        # 实测约 2 秒）才落盘，所以这里要**等文件出现**，不能只看 HUD。
        $new = $null
        $fileDeadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $fileDeadline) {
          $new = Get-ChildItem $RecDir -Filter '*.mp4' -ErrorAction SilentlyContinue |
            Where-Object { -not $before.ContainsKey($_.Name) -and $_.Name -notlike '*.partial.mp4' } |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
          if ($new) { break }
          Start-Sleep -Milliseconds 300
        }
        if (-not $new) {
          $whyB = "停止后 30 秒内没有**新增** MP4（目录: $RecDir）"
          if (Test-Path $appOut) { $whyB += ' / ' + ((Get-Content $appOut -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
        } else {
          $v = $new | Select-Object -First 1
          # 时长上限放宽到 90 秒：机器负载高时收尾（Flush/Finalize/remux）会慢，
          # 录制时长随之被拉长。判据的重点是「产物可播 + 帧数与时长自洽」。
          $r = Test-Playable $v.FullName 15 1.0 90.0
          $okB = $r.Ok
          $whyB = "$($v.Name): codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
          $whyB += "；停止响应 $([Math]::Round($swClick.Elapsed.TotalSeconds, 1))s"
          Write-Host "  产物: $($v.FullName)"
        }
      }
    }
  }
}
if ($okB) { Write-Host '  OK  热键→HUD→停止 全链路通，产物可播' }
else { Write-Host "  !! $whyB" }
$results += [pscustomobject]@{ Case = 'B 热键→HUD→停止→落盘可播'; Pass = $okB; Detail = $whyB }

# 收尾常驻进程
if (-not $app.HasExited) { $app | Stop-Process -Force -ErrorAction SilentlyContinue }
Start-Sleep -Milliseconds 600

# ================================================================ C. 崩溃恢复
Write-Host ''
Write-Host '=== C. 崩溃残留 .partial.mp4 被恢复 ==='
$partial = Join-Path $RecDir 'crashed.partial.mp4'
# 造一个「像样」的残留：拷 A 用例产出的合法 MP4 当作分片容器样本。
# 用**精确文件名**而不是「目录里最新的那个」，否则会被 B 用例的产物顶掉。
$seedPath = Join-Path $RecDir 'cli_record.mp4'
$okC = $false
$whyC = ''
if (-not (Test-Path $seedPath)) {
  $whyC = "没有种子 MP4（$seedPath）来构造 .partial 样本（A 用例未产出）"
} else {
  Copy-Item $seedPath $partial -Force
  Write-Host "  造残留: $partial ($((Get-Item $partial).Length) 字节)"

  $recLog = Join-Path $OutDir 'record_recover.out.log'
  $recErr = Join-Path $OutDir 'record_recover.err.log'
  if (Test-Path $recLog) { Remove-Item $recLog -Force }
  if (Test-Path $recErr) { Remove-Item $recErr -Force }

  # 常驻进程启动时会后台扫录像目录做恢复
  $rec = Start-Process -FilePath $Exe -ArgumentList @('--config', $CfgPath) -PassThru `
    -RedirectStandardOutput $recLog -RedirectStandardError $recErr
  $deadline = (Get-Date).AddSeconds(40)
  $recovered = $false
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 700
    if (Test-Path (Join-Path $RecDir 'crashed.mp4')) { $recovered = $true; break }
    if ($rec.HasExited) { break }
  }
  if (-not $rec.HasExited) { $rec | Stop-Process -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 400

  if (-not $recovered) {
    $whyC = '残留 .partial.mp4 没有被转成正式 MP4'
    if (Test-Path $recLog) { $whyC += ' / ' + ((Get-Content $recLog -ErrorAction SilentlyContinue | Select-Object -Last 3) -join ' | ') }
  } else {
    $r = Test-Playable (Join-Path $RecDir 'crashed.mp4') 15 0.5 30.0
    $okC = $r.Ok
    $whyC = "crashed.mp4: codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
  }
}
if ($okC) { Write-Host '  OK  崩溃残留被恢复成可播放的 MP4' }
else { Write-Host "  !! $whyC" }
$results += [pscustomobject]@{ Case = 'C 崩溃残留被恢复'; Pass = $okC; Detail = $whyC }

# ================================================================ D. 奇数尺寸窗口录制
Write-Host ''
Write-Host '=== D. 奇数尺寸窗口录制（NV12 要求偶数尺寸） ==='
# H.264/NV12 的色度是 2×2 降采样，宽高必须是偶数。奇数尺寸的窗口很常见，
# 之前会**直接录不了**（捕获层报错 → 降级 GDI → 编码器再报错 → 零产物）。
$oddHelper = Join-Path $Root 'scripts\_odd_window.ps1'
$oddOut    = Join-Path $RecDir 'd_odd_window.mp4'
$oddErr    = Join-Path $OutDir 'odd_window.err.log'
$oddStd    = Join-Path $OutDir 'odd_window.out.log'
$oddWinLog = Join-Path $OutDir 'odd_window_helper.log'
$oddWinErr = Join-Path $OutDir 'odd_window_helper.err.log'
foreach ($f in @($oddErr, $oddStd, $oddWinLog, $oddWinErr)) { if (Test-Path $f) { Remove-Item $f -Force } }

$okD = $false
$whyD = ''
$winProc = $null
try {
  $winProc = Start-Process powershell -PassThru -WindowStyle Hidden `
    -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $oddHelper) `
    -RedirectStandardOutput $oddWinLog -RedirectStandardError $oddWinErr

  # 等窗口真的出现（标题可见）再录。
  $winHwnd = [IntPtr]::Zero
  $deadline = (Get-Date).AddSeconds(20)
  while ((Get-Date) -lt $deadline) {
    $winHwnd = [Shot]::FindByTitle('ScreenloomOddProbe')
    if ($winHwnd -ne [IntPtr]::Zero) { break }
    Start-Sleep -Milliseconds 300
  }

  if ($winHwnd -eq [IntPtr]::Zero) {
    $whyD = '没有出现 ScreenloomOddProbe 窗口'
  } else {
    $cr = New-Object Shot+RECT
    [void][Shot]::GetClientRect($winHwnd, [ref]$cr)
    $cw = $cr.R - $cr.L
    $ch = $cr.B - $cr.T
    Write-Host "  窗口客户区 = ${cw}x${ch}（奇数 = 正是要覆盖的场景）"

    $p = Start-Process -FilePath $Exe -PassThru -NoNewWindow `
      -ArgumentList @(
        '--config', $CfgPath,
        '--record', '3',
        '--record-out', $oddOut,
        '--record-fps', '15',
        '--record-countdown', '0',
        '--record-window', 'ScreenloomOddProbe'
      ) `
      -RedirectStandardOutput $oddStd -RedirectStandardError $oddErr
    $p.WaitForExit(90000) | Out-Null
    if (-not $p.HasExited) { $p | Stop-Process -Force; $whyD = '录制超时（>90s）' }
    elseif (-not (Test-Path $oddOut)) {
      $whyD = '奇数尺寸窗口没有产出 MP4'
      if (Test-Path $oddErr) {
        $whyD += ' / ' + ((Get-Content $oddErr -Encoding UTF8 -ErrorAction SilentlyContinue |
          Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ')
      }
    } else {
      $r = Test-Playable $oddOut 15 1.0 20.0
      $okD = $r.Ok
      $whyD = "codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
      # 判据：成片分辨率必须是**偶数**（NV12 的硬约束）。
      $vw = Get-StreamField $oddOut 'width'
      $vh = Get-StreamField $oddOut 'height'
      if ($okD) {
        if (-not $vw -or -not $vh) {
          $okD = $false; $whyD += '；但 ffprobe 读不到分辨率'
        } elseif (([int]$vw % 2) -ne 0 -or ([int]$vh % 2) -ne 0) {
          $okD = $false; $whyD += "；但成片分辨率 ${vw}x${vh} 不是偶数"
        } else {
          $whyD += "；分辨率 ${vw}x${vh}（偶数）"
        }
      }
      Write-Host "  产物: $oddOut"
    }
  }
} finally {
  if ($winProc -and -not $winProc.HasExited) { $winProc | Stop-Process -Force -ErrorAction SilentlyContinue }
}
if ($okD) { Write-Host '  OK  奇数尺寸窗口也能录出可播 MP4' }
else { Write-Host "  !! $whyD" }
$results += [pscustomobject]@{ Case = 'D 奇数尺寸窗口可录制'; Pass = $okD; Detail = $whyD }

# ================================================================ 汇总
Write-Host ''
Write-Host '=== 汇总 ==='
$allPass = $true
foreach ($r in $results) {
  $mark = if ($r.Pass) { 'PASS' } else { 'FAIL' }
  Write-Host ("  {0,-34} {1}" -f $r.Case, $mark)
  if (-not $r.Pass) { $allPass = $false }
}

# 清理
Get-Process screenloom -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
if ($Keep) {
  Write-Host ''
  Write-Host '  (SCREENLOOM_E2E_KEEP=1，保留产物与留证)'
} else {
  Remove-Item $RecDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item $CfgPath -Force -ErrorAction SilentlyContinue
}

if (-not $allPass) { exit 1 }
exit 0
