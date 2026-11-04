# screenloom —— 微信 4.x 截图工具的 Rust 复刻

微信 PC 4.x（`Weixin.dll` 内的 `wx::` 命名空间）截图工具的独立复刻实现。
Windows 专用，Rust 2021。

> **命名说明**：项目名为 `screenloom`（crate 前缀 `screenloom-*`、Rust 模块 `screenloom_*`、
> 二进制 `screenloom.exe`、窗口类名 `Screenloom*Window`）。
> 文档中出现的 `wx::` 是**微信原版**内部命名空间的**文档别名**（本项目与微信的命名空间无关，
> 两者不要混淆）；取证脚本 `doc/scripts/scan_wechat_shot.py` 用十六进制常量按真实字节检索该命名空间。

---

## 1. 环境要求

| 项 | 要求 |
|---|---|
| 操作系统 | Windows 10 1903+ / Windows 11（需 WGC 与 `Windows.Media.Ocr`；本项目在 Win11 build 26200 上实测） |
| Rust | MSRV **1.75**（本机实测 rustc / cargo **1.96.0**） |
| 构建工具 | 仅 PE 图标注入需要 `rc.exe` + `cvtres.exe`（**找不到就跳过注入，不让构建失败**） |
| 网络 | 仅 AI 功能需要（走系统自带 WinHTTP，不引第三方 TLS 栈） |

依赖只有 4 个，刻意保持克制：

```toml
windows    = "0.61"   # Win32
png        = "0.17"   # PNG 编解码
serde_json = "1"      # AI 请求体
base64     = "0.22"   # AI 图片 data URL
```

**用新的 Win32 API 时**，先在 `screenloom-app/Cargo.toml` 打开对应 feature
（如 `Win32_Graphics_Dwm`、`Win32_UI_Controls`），不要为了单个 API 引新 crate。

---

## 2. 代码结构

```
D:\github\screenloom\
├─ screenloom-core/      纯逻辑，不碰 Win32：几何 / 图像 / 标注 / 撤销 / 长截图拼接 / 配置 / 图标
├─ screenloom-capture/   抓屏：WGC 主路径 + GDI 兜底 + 显示器枚举 + 虚拟屏整屏合成
├─ screenloom-ui/        渲染交互：GDI+ 管线 / 分层浮窗 / 选区 / 编辑器 / 工具栏 / 贴图窗
├─ screenloom-app/       应用层：main / hotkey / tray / settings / theme / clipboard / ai / ocr / save
├─ scripts/          E2E 与探针脚本（PowerShell）
└─ e2e_out/          视觉留证截图
```

依赖方向**单向**：`app → ui → capture → core`。

| crate | 职责 | 硬约束 |
|---|---|---|
| `screenloom-core` | 纯逻辑 | **不许引 Win32**（保持可移植、可单测） |
| `screenloom-capture` | 抓屏 | 必须提供**整块虚拟屏**图像（不是只抓主屏） |
| `screenloom-ui` | 渲染交互 | **不许依赖 `screenloom-app`**（否则循环依赖）→ 弹设置窗口只能发请求 |
| `screenloom-app` | 应用层 | 设置窗口的 Win32 逻辑全在这里 |

---

## 3. 编译

```powershell
cd D:\github\screenloom

# 改 app 层代码时必须先杀掉在跑的进程，否则 exe 被占用 → os error 5
Get-Process -Name screenloom -ErrorAction SilentlyContinue | Stop-Process -Force

cargo build --release -p screenloom-app --bins --examples
```

产物：`target\release\screenloom.exe`

| 命令 | 说明 |
|---|---|
| `cargo build --release -p screenloom-app --bins --examples` | 完整构建（**改 app 层必带 `--bins`**） |
| `cargo build --release -p screenloom-app` | 只出二进制 |
| `cargo test --workspace` | 全量单测 |
| `cargo build --workspace` | 全 workspace |

当前状态：`cargo test --workspace` **90 个测试全绿**（app 49 / core 24 / ui 17），release **零警告**。

---

## 4. 运行

```powershell
.\target\release\screenloom.exe            # 常驻：注册全局热键（默认 Alt+A）+ 托盘图标
.\target\release\screenloom.exe --settings # 直接开设置窗口
```

### 全部命令行参数

| 参数 | 说明 |
|---|---|
| （无） | 常驻模式，注册全局热键（默认 `Alt+A`），同时驻留托盘图标 |
| `--once` | 截一次图（复制到剪贴板 + 存盘）后退出 |
| `--long` | 长截图（用户滚动 + 自动匹配拼接）模式 |
| `--sel x,y,w,h` | **脚本化截图**：直接截指定区域，不需要鼠标。坐标是**虚拟屏物理像素** |
| `--ocr` | 识别剪贴板里图片的文字（本地 `Windows.Media.Ocr`） |
| `--hotkey KEY` | 临时覆盖热键，如 `Ctrl+Shift+A` / `Alt+F9` |
| `--config PATH` | 指定配置文件 |
| `--theme PATH` | 主题覆盖 JSON（配色/圆角）；**文件改了自动热重载**，不用重启 |
| `--version` / `-V` | 显示版本 |
| `--help` / `-h` | 帮助 |

### AI 能力

| 参数 | 说明 |
|---|---|
| `--ask "问题"` | 问 AI（图片默认取剪贴板） |
| `--translate [语言]` | 翻译图里的文字（默认英文） |
| `--ai-mosaic` | AI 一键打码（打码后存盘 + 复制） |
| `--image PATH` | 指定输入图片（默认剪贴板） |
| `--provider NAME` | `minimax`（默认）/ `deepseek` |
| `--model NAME` | 指定模型（如 `MiniMax-M3`） |
| `--endpoint URL` | 指定端点 |
| `--ai-out DIR` | `--ai-mosaic` 的输出目录 |
| `--ai-block N` | 马赛克块边长（像素，默认 12） |
| `--ai-probe` | **连通性探针，不需要 key**（预期 401 = 链路通） |

> **API key 来源**：环境变量 `MINIMAX_API_KEY` / `DEEPSEEK_API_KEY`，
> 或配置文件里的 `ai_key_minimax` / `ai_key_deepseek`。**不硬编码 key**。
>
> ⚠️ 本机目前**没有 key**，所以 `--ask` / `--translate` / `--ai-mosaic` 的**真实端到端调用尚未验证**。
> 已验证的是：请求体构造、响应解析、bbox 解析、以及两家端点都拿到 **HTTP 401**（DNS + TLS + HTTP + 请求头格式全对，只差 key）。

### 三处入口（都能打开同一个设置窗口）

| 入口 | 触发方式 |
|---|---|
| 命令行 | `screenloom.exe --settings` |
| 托盘菜单 | 右键托盘图标 →「设置…」 |
| 浮窗工具栏 | 截图浮窗上点「设置」 |

> 托盘图标在 Win11 默认被收进「隐藏图标」溢出区（系统行为，不是 bug）——点任务栏 chevron `^` 展开。

### 浮窗工具栏（选区完成后出现）

| 按钮 | 行为 | 需要 API key |
|---|---|---|
| 工具 / 调色板 / 线宽 / 字号 / 撤销 / 重做 | 编辑标注 | 否 |
| **问 AI** | 弹出输入框，把选区图片 + 问题送去模型 | **是** |
| **翻译** | 先本地 OCR，再翻成英文 | **是**（OCR 段不需要） |
| **AI 打码** | 模型标出敏感区域 → 直接变成马赛克标注 | **是** |
| **提取文字** | **本地** OCR（`Windows.Media.Ocr`），结果进结果面板并写剪贴板 | **否** |
| **滚动截图** | 对当前选区跑滚动拼接（`Enter` 继续拼 / `Esc` 结束），结果自动存盘 + 复制 | 否 |
| 设置 / 完成 / 复制 / 保存图片 / 置顶在桌面 / 退出 | — | 否 |

> **提取文字**与**滚动截图**都是纯本地能力，**不需要 API key**，无条件可用。
> 两者共用同一套「后台线程 → `post_ai_reply` 回投 → 结果面板」的机制，所以 UI 线程不会被卡住。

---

## 5. 路径与配置

| 项 | 位置 |
|---|---|
| 配置文件 | `%APPDATA%\screenloom\config.txt`（`key=value`，**UTF-8 带 BOM**） |
| 开发主题 | `screenloom/theme.dev.json`（配色覆盖，配合 `--theme` 热重载） |
| 截图落盘 | `%USERPROFILE%\Pictures\Screenloom\yyyy-MM-dd_hhmmss_zzz.png` |
| 应用图标 | `screenloom-app/assets/screenloom.ico`（由 `build.rs` 注入 exe 的 PE 资源节） |

设置窗口管理的 **12 个字段**：`hotkey` / `hide_self` / `toolbar_collapsed` / `copy_format` / `default_dir` /
`long_screenshot_max` / `long_screenshot_search` / `ai_provider` / `ai_model` / `ai_endpoint` /
`ai_api_key` / `ai_timeout`。

配置文件里另有 `ai_key_minimax` / `ai_key_deepseek` 两个**按 provider 分开**的 key 位（设置窗口不暴露，
供命令行 / 环境变量之外的第三种注入方式使用），所以配置文件实际有 14 个键。

---

## 6. 开发循环

### 6.1 一键开发模式（推荐）

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\dev.ps1
```

它做三件事，**零新依赖**（不需要 cargo-watch / sccache / node）：

1. **自动重编译 + 自动重启**：监视 `*/src/**/*.rs`、`Cargo.toml`、`build.rs`，
   一有改动就 `cargo build` → 杀掉旧进程 → 起新的。release 增量约 **5 秒**一轮。
2. **主题热重载**：改 `theme.dev.json`（配色 / 圆角）**不重编译**，应用自己在 400ms 内重绘。
3. **事件日志**：写 `%TEMP%\screenloom_dev.log`，事后可核验它干了什么。

```powershell
.\scripts\dev.ps1 -Debug          # debug profile（增量 2s，更快）
.\scripts\dev.ps1 -Mode resident  # 起常驻模式（热键+托盘）而不是设置窗口
.\scripts\dev.ps1 -NoWatch        # 只编译一次并启动，不监视
```

### 6.2 主题热重载（改配色不用重编译）

设置窗口的配色与圆角**不是编译期常量**，而是运行时从 JSON 覆盖文件读的：

```json
{
  "accent": "#07C160",
  "bg": "#131518",
  "radius_panel": 9
}
```

保存文件 → 应用 400ms 内重绘（**不重启、不重编译**）。
可覆盖 24 个颜色键 + 5 个圆角键，键名见 `theme.dev.json`（未列出的键会被忽略）。
颜色接受 `#RRGGBB` / `#AARRGGBB` / `0xAARRGGBB`。

> **注意**：JSON **不能带 BOM**（`serde_json` 不认），写文件时用
> `[System.IO.File]::WriteAllText($p, $json, (New-Object System.Text.UTF8Encoding($false)))`。

### 6.3 手动循环（不用 watcher）

```powershell
cd D:\github\screenloom

# 0) 一条命令跑完全部验证（格式 / clippy / 编译 / 单测 / 5 个 E2E）
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_all.ps1

# 1) 编译
Get-Process -Name screenloom -ErrorAction SilentlyContinue | Stop-Process -Force
cargo build --release -p screenloom-app --bins --examples

# 2) 测试
cargo test --workspace

# 3) 端到端
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\e2e_overlay.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\e2e_settings.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\capture_settings_theme.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\capture_theme_hotreload.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\e2e_toolbar_buttons.ps1

# 4) 手动跑
.\target\release\screenloom.exe --settings --theme .\theme.dev.json
```

### 6.4 代码质量门禁

| 命令 | 当前状态 |
|---|---|
| `cargo fmt --all -- --check` | **0 diff**（全仓已 rustfmt 格式化） |
| `cargo clippy --workspace --all-targets` | **0 warning / 0 error** |
| `cargo test --workspace` | **90 个测试全绿**（app 49 / core 24 / ui 17） |

### 调试探针（examples）

拿数据说话，不要靠猜：

```powershell
# 判定 GDI 屏幕 DC 的坐标原点（用非黑像素数反推，不靠经验）
cargo run -p screenloom-app --example probe_gdi

# 抓屏幕物理区域（虚拟屏物理坐标，WGC 抓整屏再裁剪）
cargo run -p screenloom-app --example grab_region -- 100 200 800 600 out.png

# 按窗口标题抓窗口（PrintWindow + PW_RENDERFULLCONTENT）
cargo run -p screenloom-app --example shot_window -- "screenloom 设置" out.png

# 抓一帧 / 抓整块虚拟屏
cargo run -p screenloom-capture --example capture_once
cargo run -p screenloom-capture --example capture_virtual

# 重新生成应用图标
cargo run -p screenloom-app --example gen_icon

# UI 层自检
cargo run -p screenloom-ui --example selftest

# OCR 打开临时文件这一步的路径诊断（canonicalize 的 \\?\ 前缀 vs 纯绝对路径）
cargo run -p screenloom-app --example ocr_probe
```

### scripts/

| 脚本 | 作用 |
|---|---|
| `verify_all.ps1` | **一条命令跑完全部验证**：fmt / clippy / 构建 / 单测 / 5 个 E2E，出汇总表 |
| `dev.ps1` | **开发模式**：自动重编译 + 自动重启 + 主题热重载提示 |
| `e2e_overlay.ps1` | 抓屏 / 跨屏选区 / 浮窗渲染（断言存盘尺寸 == 请求的物理像素） |
| `e2e_settings.ps1` | 设置窗口：改字段 → 点「保存」→ 校验配置写盘 |
| `e2e_toolbar_buttons.ps1` | **工具栏「提取文字」「滚动截图」**：按钮存在 → OCR 真读出屏幕文字（剪贴板判据）→ 长截图提示窗出现 + 落盘 |
| `capture_settings_theme.ps1` | 深色设置窗口**真实屏幕**留证 + 下拉菜单展开（带二值判据） |
| `capture_theme_hotreload.ps1` | **主题热重载验证**：同一 PID 内改 JSON → 配色必须跟着变 |
| `_shotlib.ps1` | 上面几个脚本共用的抓屏助手（点源使用） |
| `enum_settings_kids.ps1` | 枚举设置窗口子控件 |
| `probe_tray_overflow.ps1` / `probe_tray_zoom.ps1` | 托盘溢出区 / 图标放大留证 |

---

## 7. 八个必踩的坑

1. **编译前先杀 `screenloom-app`** —— exe 被占用会报 `failed to remove file ... 拒绝访问 (os error 5)`。
2. **只编 `--examples` 不会重编 bin** —— 改完 app 层跑起来还是旧行为，会误判"改了没用"。必须 `--bins --examples`。
3. **改源文件只能用 `edit` / `write` 工具** —— PowerShell 的 `Get-Content`/`Set-Content` 往返会把 UTF-8 中文变成**双重编码**，且逆变换有损（只能整文件重写）。
4. **`.ps1` 必须带 UTF-8 BOM**（PowerShell 5.1 读无 BOM 的脚本会按 ANSI 解，中文乱码）；临时日志写 `$env:TEMP`，别写项目目录。
5. **坐标一律物理像素** —— 逻辑↔物理只走 `screenloom_core::Rect::to_physical` / `to_logical`，别自己乘 scale。
6. **留证截图必须有二值判据** —— 空图 / 纯黑图和成功图看起来一模一样。断言：① 抓图尺寸 == `GetWindowRect` 尺寸；② 目标窗口真的存在；③ 位图里有**只有该界面才有的特征色像素数**。任一条不过就 `exit 1`。
7. **`Path::canonicalize()` 的 `\\?\` 前缀 WinRT 不认** —— `StorageFile::GetFileFromPathAsync` 直接报 `0x800700A1`「路径过长」，而路径只有 60 个字符。给 WinRT 的路径要**剥掉 `\\?\`**（UNC 还要把 `\\?\UNC\` 还原成 `\\`）。
8. **浮窗退出后残留的 `WM_QUIT` 会杀掉下一个消息循环** —— `finish()` 投 `WM_QUIT`，但事件循环是**先看到 outcome 才 break** 的，那条 `WM_QUIT` 留在队列里 → 紧接着的长截图「按 Enter 拼接」一 `GetMessageW` 就退出，**长截图只截到第一帧**。退出前必须把 `WM_QUIT` 抽干。

---

## 8. 加功能时往哪放

| 想做的事 | 落点 |
|---|---|
| 新配置项 | `screenloom-core/src/config.rs` 加字段 → `screenloom-app/src/settings.rs` 加 `Field` / `SECTIONS`（**`EDIT` 创建顺序必须 == 字段顺序**，E2E 按下标取控件） |
| 改界面配色 / 圆角 | `screenloom-app/src/theme.rs`（纯配色 + 绘制基元，不含 Win32 逻辑） |
| 新截图工具 / 编辑器 | `screenloom-ui/src/toolbar.rs` + `screenloom-ui/src/overlay.rs` |
| 新工具栏按钮 | ① `toolbar.rs` 加 `ItemKind` 变体 + `measure()` 宽度 + `build()` 布局 + `paint()` 标签；② `overlay.rs` 加分支（必要时加 `OverlayAction` 变体）；③ `screenloom-app/src/flow.rs` 装 handler / 处理 action。**宽度一定要同步改 `measure()`**，否则工具栏排版错位 |
| 新 AI 能力 | `screenloom-app/src/ai.rs`（OpenAI 兼容接口，DeepSeek / MiniMax 共用） |
| 新抓屏后端 | `screenloom-capture/`（WGC 为主，GDI 兜底） |
| **录屏** | 方案已论证（见 `doc/录屏方案.md`）：`core/src/nv12.rs`（色彩转换）+ `capture/src/stream.rs`（持续帧源）+ `app/src/record/`（MF 编码器）+ `ui/src/recorder_hud.rs`（REC 浮窗） |
| 图标 | `screenloom-core/src/icon.rs` → `cargo run -p screenloom-app --example gen_icon` |

新增 Win32 API 后，**在 `doc/实现契约.md` 附录 B.3 记一条坑**（那里 42 条全是真机踩出来的）。

---

## 9. 已知限制

| 限制 | 说明 |
|---|---|
| AI 真实调用未验证 | 本机没有 API key，端到端实调待补 |
| 微信内表情插入 / 转发 | 依赖腾讯私有协议与服务端，**明确不做** |
| 自动滚动长截图 | 只做「用户滚动 + 自动匹配拼接」，不做微信那种自动滚 |
| 长截图拼接未做真机滚动实测 | 入口链路（按钮 → 提示窗 → 落盘 + 复制）**已用 E2E 验证**；但 `match_offset` / `splice` 在**真实滚动页面**上的拼接效果还没实测 |
| 多屏 DPI | `screenloom-ui` 单一 `scale` 假设全虚拟屏同 DPI |
| **录屏未实现** | 方案已论证（`doc/录屏方案.md`），**代码尚未落地**，当前无任何录屏入口 |
| 标注填充 | 形状填充固定 30% alpha |
| 工具栏位置 | 固定在虚拟屏底部 |
| 设置窗口无滚动条 | 内容高度由布局常量决定（客户区 829 物理像素 @1.25，适配 12 个字段）；加字段需重算 `compute_layout` |
| 自绘控件无障碍 | 设置窗口的按钮 / 复选框 / 下拉框是自绘的，不是标准 BUTTON 类 → UI Automation 识别不到，失去系统级屏幕阅读器语义 |
| 只做 Windows | `screenloom-core` 保持可移植，但不实现 macOS/Linux |

---

## 10. 相关文档

| 文档 | 内容 |
|---|---|
| `doc/WeChat4-截图工具逆向规格书.md` | 微信 4.x 截图工具的逆向分析（函数、结构、行为） |
| `doc/实现契约.md` | **实现准绳**：分层约定、类型签名、事件流、AI 契约、设置窗口契约、Win32 坑位、验收清单 |
| `doc/录屏方案.md` | **录屏**的可行性论证与落地设计（MF 编码链路实测证据 + 8 条踩坑 + 风险 + 分阶段），**实施尚未开始** |

`实现契约.md` §3 的类型签名**即契约，不得擅自更改**；改实现要同步改文档。
