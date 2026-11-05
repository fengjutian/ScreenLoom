# Screenloom

Screenloom 是一款使用 Rust 开发的 Windows 截图与录屏工具，支持区域截图、滚动长截图、图片标注、本地 OCR、屏幕录制和可选的 AI 图像处理能力。

## 主要功能

- 区域截图与多显示器支持
- 箭头、矩形、文字、马赛克等图片标注
- 滚动长截图与自动拼接
- 图片复制、保存和窗口置顶
- 基于 `Windows.Media.Ocr` 的本地文字识别
- H.264/MP4 屏幕录制
- 显示器、窗口、区域和虚拟屏录制
- 录制倒计时、帧率、码率和输出目录设置
- 可选的系统声音录制
- 托盘菜单、全局快捷键和深色界面
- 可选的 AI 问答、翻译和敏感区域打码

## 系统要求

| 项目 | 要求 |
| --- | --- |
| 操作系统 | Windows 10 1903 或更高版本、Windows 11 |
| Rust | 1.75 或更高版本 |
| 构建工具 | Cargo；安装 Visual Studio Build Tools 可获得完整的 Windows 构建环境 |

Screenloom 目前仅支持 Windows。OCR、屏幕捕获和媒体编码依赖 Windows 系统 API。

## 构建

```powershell
git clone https://github.com/fengjutian/ScreenLoom.git
cd ScreenLoom
cargo build --release -p screenloom-app
```

构建产物位于：

```text
target\release\screenloom.exe
```

如果程序正在运行并导致可执行文件被占用，可以先结束进程：

```powershell
Get-Process -Name screenloom -ErrorAction SilentlyContinue | Stop-Process -Force
```

## 使用方法

直接启动程序会进入常驻模式，并注册全局快捷键和托盘图标：

```powershell
.\target\release\screenloom.exe
```

常用命令：

```powershell
# 截图一次，保存并复制到剪贴板
.\target\release\screenloom.exe --once

# 滚动长截图
.\target\release\screenloom.exe --long

# 打开设置窗口
.\target\release\screenloom.exe --settings

# 识别剪贴板图片中的文字
.\target\release\screenloom.exe --ocr

# 录制 10 秒视频
.\target\release\screenloom.exe --record 10

# 打开录屏设置
.\target\release\screenloom.exe --record-settings
```

### 截图参数

| 参数 | 说明 |
| --- | --- |
| `--once` | 完成一次截图后退出 |
| `--long` | 启用滚动长截图模式 |
| `--sel x,y,w,h` | 按虚拟屏物理像素截取指定区域 |
| `--ocr` | 识别剪贴板图片中的文字 |
| `--hotkey KEY` | 临时指定快捷键，例如 `Ctrl+Shift+A` |
| `--settings` | 打开常规设置窗口 |
| `--theme PATH` | 加载自定义主题 JSON |

### 录屏参数

| 参数 | 说明 |
| --- | --- |
| `--record N` | 录制 N 秒并输出 H.264/MP4 文件 |
| `--record-out PATH` | 指定 `.mp4` 输出路径 |
| `--record-monitor N` | 录制指定显示器 |
| `--record-window TITLE` | 按窗口标题录制窗口 |
| `--record-region x,y,w,h` | 录制指定屏幕区域 |
| `--record-virtual` | 录制整个虚拟屏 |
| `--record-fps N` | 设置帧率，范围为 1–120 |
| `--record-bitrate BPS` | 设置视频码率 |
| `--record-countdown N` | 设置 0–10 秒的开始倒计时 |
| `--record-settings` | 打开录屏设置窗口 |

默认截图目录为 `%USERPROFILE%\Pictures\Screenloom`，默认录屏目录为 `%USERPROFILE%\Videos\Screenloom`。

录屏设置里可开启「录制系统声音」和「录制麦克风」：录制阶段先写视频 + 同名 WAV，结束后自动混流为带 AAC 音轨的 MP4；两路音频在写入 WAV 前混合，混流失败时保留纯视频和 WAV，不会丢失素材。录制中可从托盘菜单单独静音麦克风或系统声，采集继续但静音的一路不写入混音。

## AI 功能

AI 功能是可选能力，需要配置相应服务的 API Key：

```powershell
# 针对剪贴板中的图片提问
.\target\release\screenloom.exe --ask "这张图片里有什么？"

# OCR 后翻译为指定语言
.\target\release\screenloom.exe --translate 英文

# 自动识别敏感区域并打码
.\target\release\screenloom.exe --ai-mosaic
```

可通过环境变量 `MINIMAX_API_KEY` 或 `DEEPSEEK_API_KEY` 提供密钥。请勿将 API Key 提交到仓库。

## 项目结构

```text
screenloom-core/       通用数据结构、配置、图像与几何逻辑
screenloom-capture/    Windows 屏幕捕获实现
screenloom-ui/         截图浮层、工具栏和编辑交互
screenloom-app/        应用入口、托盘、设置、OCR、AI 和录屏
website/               React + TypeScript 项目网站
scripts/               开发、验证和端到端测试脚本
probes/                图形、捕获与编码验证工具
```

主要依赖方向为：

```text
screenloom-app -> screenloom-ui -> screenloom-capture -> screenloom-core
```

## 开发与测试

运行完整工作区测试：

```powershell
cargo test --workspace
```

检查格式和静态问题：

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
```

运行项目验证脚本：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_all.ps1
```

启动自动构建与重启的开发模式：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\dev.ps1
```

## 配置

用户配置默认保存在：

```text
%APPDATA%\screenloom\config.txt
```

开发时可以通过 `theme.dev.json` 调整界面主题，并使用 `--theme` 参数加载。主题文件应保存为不带 BOM 的 UTF-8 JSON。

## 网站

项目网站位于 `website/`：

```powershell
cd website
npm install
npm run dev
```

生产构建：

```powershell
npm run build
```

## 许可证

本项目采用 MIT License。
