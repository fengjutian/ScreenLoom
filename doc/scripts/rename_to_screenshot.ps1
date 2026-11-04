# 项目改名脚本：screenloom -> screenshot
#
# 精确规则（关键：绝不能误伤微信内部的 `wx::` 命名空间，那是关于微信的事实）：
#   R1 标识符/包名 : screenloom_core / screenloom_capture / screenloom_ui / screenloom-app / screenloom_ocr_
#                   -> screenshot_core / screenshot_capture / screenshot_ui / screenshot-app / screenshot_ocr_
#   R2 crate 目录名 : screenloom-core / screenloom-capture / screenloom-ui / screenloom-app -> screenshot-*
#   R3 字符串字面量 : "screenloom  (后面不接 ::)  -> "screenshot
#   R4 配置目录     : join("screenloom") -> join("screenshot")
#   R5 路径/文档引用 : screenloom -> screenshot, doc -> screenshot-spec
#   **保留**        : `screenloom`、`wx::`、`wx::WxShot` 等散文引用不动
param([switch]$DryRun)

$ErrorActionPreference = 'Stop'
$ROOT = 'D:\TencentWeixin\Weixin'
$UTF8 = New-Object System.Text.UTF8Encoding($false)

function Rewrite([string]$path, [hashtable]$map, [hashtable[]]$regexMap) {
    if (-not (Test-Path $path)) { return }
    $text = [System.IO.File]::ReadAllText($path)
    $orig = $text
    $notes = @()
    foreach ($k in $map.Keys) {
        $n = ([regex]::Matches($text, [regex]::Escape($k))).Count
        if ($n -gt 0) {
            $text = $text.Replace($k, $map[$k])
            $notes += "$k x$n"
        }
    }
    foreach ($rm in $regexMap) {
        $n = ([regex]::Matches($text, $rm['pattern'])).Count
        if ($n -gt 0) {
            $text = [regex]::Replace($text, $rm['pattern'], $rm['replace'])
            $notes += "$($rm['label']) x$n"
        }
    }
    if ($text -ne $orig) {
        if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $text, $UTF8) }
        Write-Host ("  {0}  <- {1}" -f (Split-Path $path -Leaf), ($notes -join ', '))
    }
}

# ---- 映射表 ----
$identMap = @{
    'screenloom_core'    = 'screenshot_core'
    'screenloom_capture' = 'screenshot_capture'
    'screenloom_ui'      = 'screenshot_ui'
    'screenloom-app'     = 'screenshot-app'
    'screenloom_ocr_'    = 'screenshot_ocr_'
    'screenloom-core'    = 'screenshot-core'
    'screenloom-capture' = 'screenshot-capture'
    'screenloom-ui'      = 'screenshot-ui'
}
# 字符串字面量里的 "screenloom（后面不能紧跟 ::）
$strRegex = @(@{ pattern = '"screenloom(?!::)'; replace = '"screenshot'; label = '"screenloom' })
# 路径 / 目录引用
$pathMap = @{
    'screenloom'   = 'screenshot'
    'doc' = 'screenshot-spec'
}
$pathRegex = @(@{ pattern = 'join\("screenloom"\)'; replace = 'join("screenshot")'; label = 'join("screenloom")' })

Write-Host "== 1. 移动目录 =="
$dirMoves = @(
    @{ from = "$ROOT\screenloom";   to = "$ROOT\screenshot" },
    @{ from = "$ROOT\doc"; to = "$ROOT\screenshot-spec" }
)
foreach ($m in $dirMoves) {
    if (Test-Path $m.from) {
        if (Test-Path $m.to) { throw "目标已存在: $($m.to)" }
        if (-not $DryRun) { Move-Item -LiteralPath $m.from -Destination $m.to }
        Write-Host "  $($m.from)  ->  $($m.to)"
    }
}

$WS = if ($DryRun) { "$ROOT\screenloom" } else { "$ROOT\screenshot" }
foreach ($c in @('screenloom-core', 'screenloom-capture', 'screenloom-ui', 'screenloom-app')) {
    $from = Join-Path $WS $c
    $to = Join-Path $WS ($c -replace '^screenloom-', 'screenshot-')
    if (Test-Path $from) {
        if (Test-Path $to) { throw "目标已存在: $to" }
        if (-not $DryRun) { Move-Item -LiteralPath $from -Destination $to }
        Write-Host "  crate 目录: $c -> $($c -replace '^screenloom-','screenshot-')"
    }
}

Write-Host "== 2. Cargo.toml =="
Rewrite (Join-Path $WS 'Cargo.toml') $pathMap @()

$appToml = Join-Path $WS 'screenshot-app\Cargo.toml'
if (Test-Path $appToml) {
    $t = [System.IO.File]::ReadAllText($appToml)
    if ($t -notmatch '\[\[bin\]\]') {
        $t = $t -replace '(?m)^description = .*$', "`$0`n`n[[bin]]`nname = `"screenshot`"`npath = `"src/main.rs`""
        if (-not $DryRun) { [System.IO.File]::WriteAllText($appToml, $t, $UTF8) }
        Write-Host "  app: 追加 [[bin]] name = screenshot"
    }
}

Write-Host "== 3. Rust 源码 =="
$rsFiles = Get-ChildItem $WS -Recurse -File -Include '*.rs', '*.toml' | Where-Object { $_.FullName -notmatch '\\target\\' }
foreach ($f in $rsFiles) { Rewrite $f.FullName $identMap $strRegex }

Write-Host "== 4. 文档 =="
$SPEC = if ($DryRun) { "$ROOT\doc" } else { "$ROOT\screenshot-spec" }
if (Test-Path $SPEC) {
    Get-ChildItem $SPEC -Recurse -File -Include '*.md', '*.py' | ForEach-Object {
        Rewrite $_.FullName $pathMap $pathRegex
    }
}

Write-Host "== 5. 结果 =="
Get-ChildItem $ROOT -Directory | ForEach-Object { "  " + $_.Name }
if (Test-Path $WS) { Get-ChildItem $WS -Directory | ForEach-Object { "    " + $_.Name } }
