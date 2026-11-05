# 编码子进程隔离（REC-102）的端到端验证（Windows PowerShell 5.1）
#
# 背景：Media Foundation 的 `SinkWriter` 在写 `moov`（Finalize）时约 9% 概率永久阻塞。
# 进程内没有任何办法能救 —— COM 调用不可取消。把 SinkWriter 关进子进程后，
# 卡死就变成"kill 一个子进程"，已录内容（分片容器）还能重封装成可播放的 MP4。
#
# 四条链路，每条都有**外部可证伪**的判据：
#   A. 子进程模式正常录制
#      判据：产物能被 ffprobe 解析（h264 / yuv420p、帧数>0、时长自洽）；
#            且日志明确走的是「子进程」路径
#   B. 注入 Finalize 卡死 → **用户当场拿到可播放的正式 MP4**
#      判据：目录里出现正式 `.mp4`（不是 `.partial.mp4`）且能被 ffprobe 解析；
#            `.partial.mp4` 已消失；全程在 45 秒内结束（不需要重启程序）
#   C. 编码子进程拉不起来 → 自动回退进程内，录制仍然成功
#      判据：产物可播；日志出现「回退到进程内编码」
#   E. 写入阶段中途卡死（录到一半才死）→ 已录内容仍能救回
#      判据：目录里出现正式 `.mp4` 且能被 ffprobe 解析、帧数 > 0
#   D. 卡死处置后不残留编码子进程（不能留孤儿占着文件）
#      判据：命令行含 `--encoder-host` 的进程数为 0
#
# 用法：
#   powershell -NoProfile -ExecutionPolicy Bypass -File D:\github\screenloom\scripts\e2e_encoder_host.ps1
#
# 可用环境变量：
#   SCREENLOOM_E2E_KEEP=1   保留产物与留证（默认清理）

$ErrorActionPreference = 'Stop'

$Root    = 'D:\github\screenloom'
$Exe     = Join-Path $Root 'target\release\screenloom.exe'
$OutDir  = Join-Path $Root 'probes\e2e_out'
$CfgPath = Join-Path $Root 'e2e_encoder_config.txt'
$Keep    = ($env:SCREENLOOM_E2E_KEEP -eq '1')

# 产物目录放在工作区内（写工作区外的路径会被本地安全策略拦）。
$RecDir  = Join-Path $Root 'probes\e2e_out\encoder_out'

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
if (Test-Path $RecDir) { Remove-Item $RecDir -Recurse -Force }
New-Item -ItemType Directory -Path $RecDir | Out-Null

# ---------------------------------------------------------------- 工具函数

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

# 返回 @{ Ok=bool; Reason=string; Codec=..; PixFmt=..; Frames=..; Duration=.. }
function Test-Playable([string]$file, [double]$minSec, [double]$maxSec) {
  $r = @{ Ok = $false; Reason = ''; Codec = ''; PixFmt = ''; Frames = 0; Duration = 0.0 }
  if (-not (Test-Path $file)) { $r.Reason = "文件不存在: $file"; return $r }
  $len = (Get-Item $file).Length
  if ($len -lt 1024) { $r.Reason = "文件过小（$len 字节），不像有效视频"; return $r }

  $r.Codec = Get-StreamField $file 'codec_name'
  $r.PixFmt = Get-StreamField $file 'pix_fmt'
  $durRaw = Get-FormatField $file 'duration'
  if ($durRaw) { $r.Duration = [double]$durRaw }
  $framesRaw = Get-StreamField $file 'nb_frames'
  if ($framesRaw) { $r.Frames = [int]$framesRaw }

  if ($r.Codec -ne 'h264') { $r.Reason = "codec 不是 h264（实际: '$($r.Codec)'）"; return $r }
  if ($r.PixFmt -ne 'yuv420p') { $r.Reason = "pix_fmt 不是 yuv420p（实际: '$($r.PixFmt)'）"; return $r }
  if ($r.Duration -le 0) { $r.Reason = 'duration 解析失败或为 0'; return $r }
  if ($r.Duration -lt $minSec -or $r.Duration -gt $maxSec) {
    $r.Reason = "时长 $($r.Duration)s 不在期望区间 [$minSec, $maxSec]s"
    return $r
  }
  if ($r.Frames -le 0) {
    $decoded = & ffprobe -v error -count_frames -select_streams 'v:0' -show_entries 'stream=nb_read_frames' `
      -of default=noprint_wrappers=1:nokey=1 -- "$file" 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $decoded) { $r.Reason = 'nb_frames 缺失且无法解码统计帧数'; return $r }
    $r.Frames = [int]($decoded | Select-Object -First 1)
  }
  if ($r.Frames -le 0) { $r.Reason = '解码后帧数为 0'; return $r }
  $r.Ok = $true
  $r.Reason = 'ok'
  return $r
}

function Write-RecordConfig([string]$path, [string]$recordDir) {
  $lines = @(
    '# e2e_encoder_host 专用配置'
    'hotkey = Alt+A'
    'record_dir = ' + $recordDir
    'record_audio_system = false'
    'record_audio_mic = false'
    'record_countdown = 0'
    'record_show_hud = false'
    'record_open_folder = false'
    'record_fps = 15'
    'record_bitrate = 2000000'
  )
  [System.IO.File]::WriteAllLines($path, $lines, (New-Object System.Text.UTF8Encoding($false)))
}

# 跑一次 CLI 录制，返回 @{ Exit=..; Sec=耗时秒; Out=正式MP4; Partial=残留分片; Err=stderr全文 }
function Invoke-Record([string]$tag, [string]$outFile, [hashtable]$envVars, [int]$timeoutSec = 90) {
  $outLog = Join-Path $OutDir "enc_$tag.out.log"
  $errLog = Join-Path $OutDir "enc_$tag.err.log"
  foreach ($f in @($outLog, $errLog)) { if (Test-Path $f) { Remove-Item $f -Force } }

  # 环境变量必须由当前进程设置后继承（PowerShell 5.1 的 Start-Process 没有 -Environment）。
  $saved = @{}
  foreach ($k in $envVars.Keys) {
    $saved[$k] = [Environment]::GetEnvironmentVariable($k)
    [Environment]::SetEnvironmentVariable($k, $envVars[$k])
  }
  try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $Exe -PassThru -NoNewWindow `
      -ArgumentList @(
        '--config', $CfgPath,
        '--record', '3',
        '--record-out', $outFile,
        '--record-fps', '15',
        '--record-bitrate', '2000000',
        '--record-countdown', '0',
        '--record-region', '0,0,640,360'
      ) `
      -RedirectStandardOutput $outLog -RedirectStandardError $errLog
    $p.WaitForExit($timeoutSec * 1000) | Out-Null
    $sw.Stop()
    $timedOut = -not $p.HasExited
    if ($timedOut) { $p | Stop-Process -Force -ErrorAction SilentlyContinue }
    $exit = $null
    try { $exit = $p.ExitCode } catch { $exit = $null }
  } finally {
    foreach ($k in $saved.Keys) { [Environment]::SetEnvironmentVariable($k, $saved[$k]) }
  }

  $partial = Get-ChildItem $RecDir -Filter '*.partial.mp4' -ErrorAction SilentlyContinue |
    Select-Object -First 1
  $errText = ''
  # Rust 的 stderr 是 UTF-8 字节流；PowerShell 5.1 的 Get-Content 默认按 ANSI(cp936) 解，
  # 中文会变乱码，导致后面按中文关键字匹配日志时全部落空。
  if (Test-Path $errLog) { $errText = (Get-Content $errLog -Raw -Encoding UTF8 -ErrorAction SilentlyContinue) }

  return @{
    Exit    = $exit
    TimedOut = $timedOut
    Sec     = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
    Out     = $outFile
    Partial = if ($partial) { $partial.FullName } else { '' }
    Err     = $errText
    OutLog  = $outLog
    ErrLog  = $errLog
  }
}

# 还在跑的编码子进程数量（命令行含 --encoder-host）。
function Get-HostChildCount() {
  $n = 0
  try {
    $procs = Get-CimInstance Win32_Process -Filter "Name='screenloom.exe'" -ErrorAction SilentlyContinue
    foreach ($p in $procs) {
      if ($p.CommandLine -and $p.CommandLine -like '*--encoder-host*') { $n++ }
    }
  } catch { return -1 }
  return $n
}

if (-not (Test-Path $Exe)) {
  Write-Host "!! 找不到 $Exe"
  Write-Host '   先执行: cargo build --release -p screenloom-app --bins'
  exit 1
}

Write-RecordConfig $CfgPath $RecDir

# 清场：残留的 screenloom 进程（含编码子进程）全部收掉。
Get-Process screenloom -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

$results = @()

# ================================================================ A. 子进程模式正常录制
Write-Host '=== A. 编码子进程模式：正常录制 ==='
$aOut = Join-Path $RecDir 'a_child.mp4'
$a = Invoke-Record 'a' $aOut @{}
$okA = $false
$whyA = ''
if ($a.TimedOut) {
  $whyA = "录制超时（>90s）"
} elseif (-not (Test-Path $aOut)) {
  $whyA = '没有产出正式 MP4'
  if ($a.Err) { $whyA += ' / ' + (($a.Err -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ') }
} else {
  $r = Test-Playable $aOut 1.0 20.0
  $okA = $r.Ok
  $whyA = "codec=$($r.Codec) pix=$($r.PixFmt) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
  # 判据：必须**真的走了子进程**，否则这条用例测的不是被测对象。
  if ($okA -and $a.Err -notlike '*录屏编码路径: 子进程*') {
    $okA = $false
    $whyA += '；但日志里没有「录屏编码路径: 子进程」'
  }
}
if ($okA) { Write-Host "  OK  子进程模式正常录制，产物可播（$whyA）" }
else { Write-Host "  !! $whyA" }
$results += [pscustomobject]@{ Case = 'A 子进程模式正常录制'; Pass = $okA; Detail = $whyA }

# ================================================================ B. 注入 Finalize 卡死
Write-Host ''
Write-Host '=== B. 注入 Finalize 卡死 → 用户当场拿到可播 MP4 ==='
$bOut = Join-Path $RecDir 'b_hang.mp4'
# SCREENLOOM_HOST_HANG 会被编码子进程继承（子进程是父进程自己拉起的）。
$b = Invoke-Record 'b' $bOut @{ SCREENLOOM_HOST_HANG = 'finalize' } 90
$okB = $false
$whyB = ''
if ($b.TimedOut) {
  $whyB = '录制超时（>90s）——卡死没有被处置'
} elseif (-not (Test-Path $bOut)) {
  $whyB = '卡死后没有产出正式 MP4（只剩分片文件 = 用户拿不到文件）'
  if ($b.Partial) { $whyB += "；残留分片: $b.Partial" }
  if ($b.Err) { $whyB += ' / ' + (($b.Err -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ') }
} else {
  $r = Test-Playable $bOut 0.5 30.0
  $okB = $r.Ok
  $whyB = "codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
  # 判据：临时分片必须已经消失（说明走的是「重封装成正式文件」这条路）。
  if ($okB -and $b.Partial) {
    $okB = $false
    $whyB += "；但还残留分片文件 $($b.Partial)"
  }
  # 判据：必须是「当场拿到」，不是「重启程序后恢复」。
  # 卡死处置（kill 子进程 4s 超时）+ 重封装应该在 45 秒内走完。
  if ($okB -and $b.Sec -gt 45) {
    $okB = $false
    $whyB += "；但整个流程花了 $($b.Sec) 秒，超出 45 秒"
  }
}
if ($okB) { Write-Host "  OK  卡死后当场产出可播 MP4（耗时 $($b.Sec)s / $whyB）" }
else { Write-Host "  !! $whyB" }
$results += [pscustomobject]@{ Case = 'B 卡死后当场拿到可播 MP4'; Pass = $okB; Detail = $whyB }

# ================================================================ C. 子进程拉不起来 → 回退
Write-Host ''
Write-Host '=== C. 编码子进程不可用 → 自动回退进程内 ==='
$cOut = Join-Path $RecDir 'c_fallback.mp4'
$missing = Join-Path $RecDir 'no_such_host.exe'
# 指向不存在的 exe：子进程必然拉不起来 → 必须回退进程内，录制仍然成功。
#
# 注意：进程内路径本身就有约 9% 的 Finalize 卡死概率（**正是 REC-102 要根治的那个缺陷**），
# 所以这里允许重试 —— 测的是「回退机制可用」，不是「进程内编码不会卡」。
$okC = $false
$whyC = ''
for ($attempt = 1; $attempt -le 3 -and -not $okC; $attempt++) {
  if (Test-Path $cOut) { Remove-Item $cOut -Force }
  Get-ChildItem $RecDir -Filter 'c_fallback*.partial.mp4' -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
  $c = Invoke-Record 'c' $cOut @{ SCREENLOOM_ENCODER_HOST = $missing } 90
  $whyC = ''
  if ($c.TimedOut) {
    $whyC = '录制超时（>90s）'
  } elseif (-not (Test-Path $cOut)) {
    $whyC = '回退后没有产出 MP4'
    if ($c.Err) { $whyC += ' / ' + (($c.Err -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ') }
  } else {
    $r = Test-Playable $cOut 1.0 20.0
    $okC = $r.Ok
    $whyC = "codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
    if ($okC -and $c.Err -notlike '*回退到进程内编码*') {
      $okC = $false
      $whyC += '；但日志里没有「回退到进程内编码」'
    }
    if ($okC) { $whyC += "（第 $attempt 次尝试）" }
  }
  if (-not $okC) { Write-Host "  -- 第 $attempt 次尝试未通过：$whyC" }
}
if ($okC) { Write-Host "  OK  子进程不可用时回退进程内，录制仍成功（$whyC）" }
else { Write-Host "  !! $whyC" }
$results += [pscustomobject]@{ Case = 'C 子进程不可用时回退进程内'; Pass = $okC; Detail = $whyC }

# ================================================================ E. 写入阶段中途卡死
Write-Host ''
Write-Host '=== E. 写入阶段中途卡死 → 已录内容仍能救回 ==='
$eOut = Join-Path $RecDir 'e_midhang.mp4'
# 先写 30 帧再卡死：真实故障是"录到一半才卡"，这时文件里已经有内容，
# 才是真正要验的救回场景（只在第 0 帧卡死的话文件是空的，测不到东西）。
$e = Invoke-Record 'e' $eOut @{ SCREENLOOM_HOST_HANG = 'write'; SCREENLOOM_HOST_HANG_AFTER = '30' } 90
$okE = $false
$whyE = ''
if ($e.TimedOut) {
  $whyE = '录制超时（>90s）'
} elseif (-not (Test-Path $eOut)) {
  $whyE = '中途卡死后没有产出正式 MP4'
  if ($e.Partial) { $whyE += "；残留分片: $e.Partial" }
  if ($e.Err) { $whyE += ' / ' + (($e.Err -split "`n" | Where-Object { $_.Trim() } | Select-Object -Last 3) -join ' | ') }
} else {
  $r = Test-Playable $eOut 0.3 30.0
  $okE = $r.Ok
  $whyE = "codec=$($r.Codec) frames=$($r.Frames) dur=$($r.Duration)s → $($r.Reason)"
  if ($okE -and $e.Partial) {
    $okE = $false
    $whyE += "；但还残留分片文件 $($e.Partial)"
  }
}
if ($okE) { Write-Host "  OK  中途卡死后已录内容救回成可播 MP4（$whyE）" }
else { Write-Host "  !! $whyE" }
$results += [pscustomobject]@{ Case = 'E 中途卡死仍能救回已录内容'; Pass = $okE; Detail = $whyE }

# ================================================================ D. 不残留编码子进程
Write-Host ''
Write-Host '=== D. 卡死处置后不残留编码子进程 ==='
$left = Get-HostChildCount
$okD = ($left -eq 0)
$whyD = "残留编码子进程数 = $left"
if ($okD) { Write-Host '  OK  没有残留的编码子进程' }
else { Write-Host "  !! $whyD" }
$results += [pscustomobject]@{ Case = 'D 无残留编码子进程'; Pass = $okD; Detail = $whyD }

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
