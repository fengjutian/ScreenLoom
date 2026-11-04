# 微信 4.x 截图工具（`wx`）逆向规格书

| 项 | 值 |
|---|---|
| 分析对象 | WeChat for Windows **4.1.15.13**（构建于 2026-09-23） |
| 宿主路径 | `D:\TencentWeixin\Weixin\4.1.15.13\` |
| 主程序 | `Weixin.exe`（2.9MB 启动器）+ `Weixin.dll`（192MB 主库） |
| 分析日期 | 2026-11-04 |
| 分析方式 | **纯静态**：PE 结构解析 + 字符串/反射成员表提取；**未做动态调试** |
| 复现脚本 | `scripts/scan_wechat_shot.py`（只读，见 §13） |
| 命名空间别名 | 本文档用 `wx::` / `WxShot*` 代指微信 DLL 内的**真实**内部命名空间（字面量已避免出现在项目中）；取证脚本以十六进制常量按真实字节检索 |

---

## 1. 结论摘要

1. **微信 4.x 的截图工具不是独立程序**，而是编译在主库 `Weixin.dll` 内的 `wx` 命名空间，入口类 `wx::WxShot`。
2. **无法通过"反编译"得到可独立编译运行的程序**（证据见 §3）。但由于其后端全部是公开 API（Windows Graphics Capture / D3D11 / GDI / Qt6 / OpenCV），且微信自身的反射成员表**完整暴露了类结构与功能清单**（§6），因此**按规格重写一个功能对等的独立工具是可行的**。
3. 本文档即为该重写所需的规格书：类结构、线程模型、抓屏后端、UI 功能表、配置开关、协议依赖、已知坑位与验收标准。

---

## 2. 宿主进程与进程模型

### 2.1 进程构成（实测运行态）

| PID | 命令行要点 | 角色 | 与截图的关系 |
|---|---|---|---|
| 31568 | `Weixin.exe --scene=taskbarpins` | **主进程**（124 线程 / 119MB） | **截图代码在此**，加载 `Weixin.dll` |
| 33140 | `--type=wxocr --app-path=.../WeChatOcr/8096/extracted` | OCR 插件进程（Mojo） | 截图取字/翻译的后端 |
| 32976 | `--type=wxutility --app-path=.../WeChatUtility/6030/extracted` | 通用工具插件 | 无关（已排除） |
| 32520 | `--type=wxpublic --app-path=4.1.15.13` | 公共库进程 | 无关（已排除） |
| 27748 | `--type=xplayer --app-path=.../XPlayer/15065/extracted` | 播放器插件 | 无关（已排除） |

### 2.2 插件目录（`%APPDATA%\Tencent\xwechat\xplugin\plugins`）

```
CourgettePatch/20   RadiumWMPF/26506   UpdateNotify/7291,7302   WaveAudioModel/10042
WeChatOcr/8096      WeChatPlayer/5067  WeChatUtility/6030      WeixinUpdate/9013
WinWeixinShare/2004,2007   WMPFDrm/1004   XEditor/22615   XFilesOfficeReader/7734,8043
XPlayer/15065
```

> **架构结论**：微信 4.x 的插件**全部是 Mojo 子进程**（`--type=xxx`），主进程才带 `Weixin.dll`。截图是主进程内行为，**不存在独立的截图 exe/dll**。

### 2.3 已排除的"疑似截图"组件

> **全目录覆盖验证**：`scripts/scan_binaries.py` 对 `4.1.15.13` 下 **23 个 ≥0.5MB 的文件**（共 870MB）做了完整关键词扫描，0 错误。
> 结果：**`wx` 字样在整个安装目录中只出现在 `Weixin.dll`**，其余文件均无 `wx::` 命名空间。

| 组件 | 实测内容 | 判定 |
|---|---|---|
| `WeChatOcr.bin`（49.5MB） | `--type=wxocr` 独立进程；`ocr_manager.cc`、`UploadImageCdn`；**256 处 opencv 命中**（文本检测/矫正用的 OpenCV） | 截图后的**文字识别引擎**，非截图本体 |
| `WeChatUtility.bin`（26.6MB） | 无任何 shot/capture 代码，仅媒体 codec 表 | 无关 |
| `XEditor.bin`（23.4MB） | `captureAndSaveTileView` / `captureGLFramebuffer` | 编辑器**离屏渲染**抓帧，非屏幕截图 |
| `XPlayer.bin`（63.4MB） | 8 处 `screenshot` 命中，实为 `TechSmith Screen Capture Codec` 等**媒体编解码器名**；DXGI 命中属视频渲染 | 无关 |
| `RadiumWMPF.bin`（412MB） | `CdpCommandCaptureScreenshot`（DevTools 内部截图） | 小程序 Chromium 内部截图，与客户端截图无关 |
| `wcprobe.dll` / `wxpublic.dll` / `ilink2.dll` / `roam_*.dll` 等 | 仅零星 `capture`/`bitmap` 通用 API 命中 | 无关 |

---

## 3. 为什么不能"反编译"成独立程序

| 检查项 | 实测结果 | 推论 |
|---|---|---|
| PE 导出表 | 117 个导出，**截图相关命中 = 0**（唯一疑似命中是 Chromium 的 `SetEnablePublicKeyPinningBypassForLocalTrustAnchors@EngineParams@mmcronet`，纯误报） | 外部程序**没有 ABI 入口**可调，无法"链接调用" |
| RTTI 类型描述符 | `.?AVWxShot@` / `.?AVScreenShoter@` / `.?AVShotSelection@` / `wx@@` 命中数**全为 0** | RTTI 被裁剪，vtable 无类型名；IDA 中定位类只能靠字符串交叉引用 |
| 调试信息 | 残留 PDB 路径 `E:\x\x_rel_t7\out\build\RelWithDebInfo\Weixin.dll.pdb`，**符号文件未随包发布** | 无符号可恢复 |
| 链接形态 | 12 个 section，`.text` 120MB / `.rdata` 60MB；`.qtmetad`+`.qtmimed` 段说明 **Qt6 静态编译**；`sk_sp<SkImage>` 说明 **Skia 静态链接**；`opencv_t`/`wx::OpenCVWorker` 说明 **OpenCV 静态链接**；`third_party/webrtc/modules/desktop_capture/win/d3d_device.cc` 说明 **WebRTC 静态链接** | 截图代码与 Qt6/Skia/OpenCV/WebRTC/mmui 框架/Mojo IPC **静态揉成单体，无模块边界可拆** |

**补充判断**：即便硬做逆向，x64 机器码 → 可编译 C++ 的还原量在**数万行**量级，且需重建 Qt 事件循环、mmui 服务注册、Mojo 通道与微信上下文（登录态、配置下发、素材资源 `:/gui/svg/...`）。周期以月计，且触碰腾讯服务条款与著作权。**结论：走重写路线。**

---

## 4. 架构总览

```
                       ┌──────────────────────────────────────┐
  快捷键/菜单触发 ───► │  wx::WxShot                      │  入口
                       │  exec / trigger / path / session_id │
                       └───────────────┬──────────────────────┘
                                       │
                       ┌───────────────▼──────────────────────┐
                       │  wx::ScreenShoter（主编排）        │
                       │  snip/copy/save/share/pin/qrcode/...  │
                       └──┬────────┬─────────┬────────┬───────┘
                          │        │         │        │
        ┌─────────────────▼─┐ ┌────▼─────┐ ┌─▼──────┐ ┌▼──────────────────┐
        │ wx::GrabWorker│ │ShotSelec-│ │LongScr-│ │ mmui::ScreenShot-  │
        │ WGC/GDI 抓屏线程   │ │tion 选区 │ │eenShot-│ │ PinWindow 贴图窗   │
        │ SetTarget/Exclude │ │ 浮层     │ │er      │ │ mmui::WindowPinUi- │
        └───────────────────┘ └──────────┘ └──┬─────┘ │ Service            │
                                               │       └───────────────────┘
                        ┌──────────────────────▼──────────────────────┐
                        │ wx::OpenCVWorker  +  SpliceWorker      │
                        │ 滚动抓取 → OpenCV 模板匹配 → 拼接          │
                        └─────────────────────────────────────────────┘
```

---

## 5. 抓屏后端规格

### 5.1 平台抽象层

证据：同一模块内嵌了 **GNOME Shell Screencast** 的 gjs 脚本（`org.gnome.Shell.Screencast` / `ScreencastArea` / `StopScreencast`，`@152737808–152744300`），说明 `wx` 有一套**跨平台抓屏抽象**，Windows 与 Linux 各有后端。

### 5.2 Windows 后端实现（按优先级）

| 优先级 | 机制 | 证据（文件内偏移） |
|---|---|---|
| 主 | **WGC**（Windows Graphics Capture）——`Windows.Graphics.Capture`、`Windows.Graphics.Capture.Direct3D11.CaptureFramePool`、`Windows.Graphics.Capture.GraphicsCaptureItem`、`Windows.Graphics.Capture.GraphicsCaptureSession` | UTF-16 字符串 @`160094880`、@`162247040` |
| 主 | **D3D11/DXGI** 设备与帧池：`D3D11CreateDevice`、`ID3D11Device`、`CreateDXGIFactory`、`IDXGIAdapter3` | @`163687576`、`164006180` |
| 辅 | **WebRTC 桌面捕获**：`third_party/webrtc/modules/desktop_capture/win/d3d_device.cc` | @`163687576` |
| 兜底 | **GDI**：`GetDC` / `BitBlt` / `CreateCompatibleDC`；错误串 `Both PrintWindow() and BitBlt() failed.`、`Capturing owned window failed` | @`163682351`、`@149990956` |
| 兜底 | **PrintWindow**（窗口渲染内容） | @`163682333` |
| Qt 路径 | `QScreen::grabWindow()`（带弃用告警：`this function is deprecated, use QScreen::grabWindow() instead. Defaulting to primary screen.`） | @`149010602` |

### 5.3 关键辅助机制

| 机制 | 说明 | 证据 |
|---|---|---|
| **隐藏自身窗口** | 截图时把微信主窗口从画面中排除：`SetWindowDisplayAffinity`；配置键 `gScreenShotHideWindow` | @`163985016`、`@149637920` |
| **按窗口排除** | `GrabWorker::SetExcludeHwnds(QVector<HWND>)`——可排除任意 HWND 列表 | @`185900472` |
| **按目标抓取** | `GrabWorker::SetTarget.target` + `SetScreen(QScreen*)`——可指定目标窗口/屏幕 | @`185900472` |
| **窗口坐标获取** | `screenshot_use_getwindowrect`（开关：截图定位用 `GetWindowRect` 而非枚举） | @`152723072` |
| **分层浮层窗口** | `WS_EX_LAYERED`、`SetLayeredWindowAttributes`（选区/贴图窗的透明实现） | @`153904673`、`@163985016` |
| **DPI 处理** | `GetDpiForWindow`、`UpdateDPI`、`UpdateGeometry`、`VirtualGeometry` | @`185905071` |
| **隐私模式** | `clicfg_screen_privacy`、`gScreenPrivacyEnabled`、`clicfg_screen_privacy_reddot` | @`149676264` |

---

## 6. 类与成员全表（反射表转录）

> 微信在 `Weixin.dll` 内建了一套**反射/成员名注册表**（`@185899120–185907400` 段），完整暴露了类名、信号槽名与成员名。以下为原样转录。

### 6.1 核心

| 类 | 成员 / 信号槽 | 推断职责 |
|---|---|---|
| `wx::WxShot` | `signalPixmap`, `path`, `exec`, `trigger`, `QWidget* window`, `from`, `session_id`, `clear` | 截图流程入口与调度 |
| `wx::ScreenShoter` | `pinData`, `std::shared_ptr<QMimeData>`, `Captured`, `etype`, `closed`, `start`, `exit`, `share`, `pin`, `ask_ai`, `ask_ai_with_prompt`, `prompt`, `save`, `copy`, `qrcode`, `handle_qr_code_result`, `convert`, `long_screenshot`, `translate_enter`, `translate_leave`, `aimosaic_enter`, `aimosaic_leave`, `snip`, `std::pair<QPixmap,QPoint>`, `moveMenu`, `updateCursor`, `ShotplaceLocation`, `refresh`, `createItem`, `pos`, `finishItem`, `ShimmerStart`, `ShimmerReset`, `ShimmerFadeOut`, `UpdateShimmerExclusions`, `UpdateDPI`, `UpdateGeometry`, `VirtualGeometry`, `smile_show`, `QPixmap& pixmap`, `use_pixel`, `sk_sp<SkImage>`, `kernel::FileBufferPtr`, `file_buffer`, `handle_qrcode`, `kernel::manager::QRItemResult item`, `handle_capture`, `translate_image`, `OpenOcrPreview`, `file_path`, `std::shared_ptr<kernel::manager::OCRResult> preloaded_ocr`, `ShowSelectionCenterFailToast`, `textReportAiMenuResultToast`, `is_translate`, `get_pixmap`, `gen_pixmap_file`, `gen_pixmap_file_translate`, `handle_menu_copy`, `handle_menu_close`, **`handle_keyboard_escape`**, **`handle_keyboard_enter`**, **`handle_keyboard_return`**, **`handle_mouse_dbclick`** | 主编排：动作分发、图像载荷、剪贴板、贴图、AI 入口、键盘/鼠标交互 |
| `mmui::ScreenshotView` | `ScreenshotWillStart`, `ScreenshotDidFinish`, `etype`, `signalUpdate`, `Captured` | 应用级钩子：截图开始/结束通知（其它 UI 据此隐藏，见 `HiddenWhenScreenShot`） |

### 6.2 选区与浮层

| 类 | 成员 / 信号槽 | 推断职责 |
|---|---|---|
| `wx::ShotSelection` | `captured`, `moved`, `resized`, `locked`, `statusChanged`, `ShotSelectionStatus`, `drawSelected`, `Shotplace&`, `start` | 选区框：拖拽/缩放/锁定/状态；尺寸提示文案 `-- x -- `、` %1 x %2 `（@`152714392`） |
| `wx::graphics::window_filter_t` | `status`, `showRegion` | **窗口选择器**：截图时列出可捕获的窗口 |
| `wx::CoverAnimationWidget` | — | 覆盖层入场动画 |
| `wx::CombinedToolButton` | — | 组合工具按钮（工具栏折叠态） |

### 6.3 抓屏与长截图线程

| 类 | 成员 / 信号槽 | 推断职责 |
|---|---|---|
| `wx::GrabWorker` | `worker_stop`, `doWork`, `SetTarget.target`, `SetScreen.QScreen* screen`, `SetExcludeHwnds.QVector<HWND> exclude_hwnds`, `Stop` | **抓屏线程**：目标/屏幕/排除窗口三参数 |
| `wx::OpenCVWorker` | `show_broken_toast`, `is_show`, `splice_init_pixmap`, `pixmap`, `splice_pixmap`, `offset`, `splice_id`, `update_pixmap`, `worker_stop`, `doWork`, `handle_works`, `Stop` | **长截图拼接线程**（OpenCV 模板匹配定位滚动偏移） |
| `wx::SpliceWorker` | `update_preview`, `pixmap`, `handle_init_pixmap`, `handle_splice_pimxap`, `offset`, `splice_id`, `SetScreen.QScreen* screen` | 拼接输出与滚动预览 |
| `wx::LongScreenShoter` | — | 长截图总控 |
| `wx::WxShotLongScreenshotmenu` | `signal_edit`, `signal_save`, `signal_close`, `signal_copy` | 长截图浮动菜单 |

线程名（可直接用作日志/调试标识）：
`longscreenshoter_grab_thread`、`longscreenshoter_opencvworker_thread`、`longscreenshoter_splice_thread`

### 6.4 编辑器与工具栏

| 类 | 成员 / 信号槽 | 推断职责 |
|---|---|---|
| `wx::WxShotEditormenu` | `smile_show`, `emoticonSelected`, `std::shared_ptr<mmui::EmoticonWrap>`, `wrap`, `share`, `save`, `copy`, `exit`, `undo`, `pin`, `ask_ai`, `ask_ai_with_prompt`, `prompt`, `qrcode`, `qrcode_leave`, `convert`, `long_screenshot`, `translate_enter`, `translate_leave`, `aimosaic_enter`, `aimosaic_leave`, `crop_enter`, `crop_leave`, `widthChanged`, `idClicked`, `id`, **`shape_fold_clicked`**, **`line_fold_clicked`**, **`graphChanged`**, **`canvas::graphics_t`**, **`penChanged`**, **`fontChanged`**, **`fillChanged`**, **`canUndo`**, **`val`**, **`paintGraph`**, **`graph`** | 编辑器主工具栏（动作全集）。**`shape_fold_clicked` / `line_fold_clicked` 表明工具栏为「图形 / 线条」两行折叠式布局**；`canvas::graphics_t` 是独立的图形数据模型（含 `paintGraph` / `graph` 图形对象与 `canUndo` 撤销栈） |
| `wx::WxShotEditorSubmenu` | `penChanged`, `fontChanged`, `fillChanged`, `moved`, `aimosaic_enter`, `aimosaic_leave` | 二级子菜单（笔/字体/填充） |
| `wx::AiMosaicButton` | `signalSelect`, `SizeType.type` | **AI 一键打码** |
| `wx::ButtonGroup` | `buttonClicked`, `mmui::XButton* button`, `buttonToggled`, `checked`, `idClicked`, `id`, `idToggled`, `emptied`, `clear` | 工具按钮分组与互斥选中 |
| `wx::ColorPanel` | `signalColorSelect`, `color` | 取色器 |
| `wx::FontSelect` | `signalSelect`, `SizeType.type` | 字体选择 |
| `wx::SizeSelect` | `signalSelect`, `SizeType.type` | 字号/粗细选择 |

### 6.5 贴图窗（置顶）

| 类 | 成员 | 推断职责 |
|---|---|---|
| `mmui::ScreenShotPinWindow` | — | 截图贴图浮窗 |
| `mmui::WindowPinUiService` | `WindowPinnedStateChanged`, `Pinned`, `HiddenWhenScreenShot`, `MinWidth`, `MinHeight`, `MaxWidth`, `MaxHeight`, `FixedWidth`, `FixedHeight` | 窗口置顶服务：尺寸约束 + 截图时隐藏 |
| `mmui::PinnedButton` | — | 置顶按钮 |

---

## 7. 用户可见功能清单（中文 UI 文案 → 成员映射）

> 来源：`@185060400–185060960`（编辑器）、`@185054540–185054700`（设置项）、`@185000545`（消息类型）。

### 7.1 编辑器工具栏

| UI 文案 | 对应成员 / 类 | 说明 |
|---|---|---|
| 坐标 / 矩形 / 椭圆 | `WxShotEditorSubmenu.penChanged` / `ButtonGroup.idClicked` | 图形工具 |
| 箭头、画笔、文字 | `penChanged`, `fontChanged`, `fillChanged` | 标注工具 |
| 马赛克 / 打马赛克 / 一键打码 | `aimosaic_enter/leave`；`wx::AiMosaicButton` | AI 自动识别敏感区域打码 |
| 还原 / 撤销 | `undo` | 撤销栈 |
| 收藏 / 表情 | `smile_show`, `emoticonSelected`, `wrap` | 插入微信表情 |
| 转发 / 分享 | `share` | 发送给他人 |
| 问 AI｜由元宝提供 | `ask_ai`, `ask_ai_with_prompt`, `prompt` | 元宝 AI 入口（带用户协议） |
| 二维码 | `qrcode`, `qrcode_leave`, `handle_qr_code_result` | 识别图中二维码 |
| 色值 | `ColorPanel`；文案「已复制色值」「按 Ctrl+C 复制色值」 | 取色器 + 复制 HEX |
| 裁剪 | `crop_enter/crop_leave`, `widthChanged` | 二次裁剪 |
| 置顶在桌面 | `pin`；`mmui::ScreenShotPinWindow` | 贴图浮窗 |
| 保存图片 | `save` | 另存（过滤器见 §9） |
| 滚动截图 | `long_screenshot` | 长截图/滚动截取 |
| 更多… / 用 AI 处理 | `convert`；子项「提取文字」「消除杂物」「翻译」 | AI 处理组 |
| 图片编辑 / 编辑 / 完成 / 退出 | `exit` | 编辑态切换 |

**交互约定（从成员名反推）**

| 操作 | 成员 | 说明 |
|---|---|---|
| `Esc` | `handle_keyboard_escape` | 取消截图 / 关闭编辑器 |
| `Enter` / `Return` | `handle_keyboard_enter` / `handle_keyboard_return` | 确认选区 / 完成 |
| 双击 | `handle_mouse_dbclick` | 选区中心确认（失败时弹 `ShowSelectionCenterFailToast`） |
| 工具栏折叠 | `shape_fold_clicked` / `line_fold_clicked` | **图形 / 线条两行折叠布局**（对应 `clicfg_pc_screenshot_toolbar_collapse`） |
| 图形模型变更 | `graphChanged` / `canvas::graphics_t` | 画布图形对象树，`canUndo` 维护撤销栈 |

### 7.2 设置项

| UI 文案 | 对应键 |
|---|---|
| 隐藏窗口截图 / 设置 / 点击截图按钮时保留当前窗口 / 默认 / 保留窗口截图 | `gScreenShotHideWindow` |

### 7.3 错误与提示文案（重写时需覆盖）

```
设备环境不允许当前应用截图，可联系网管或IT人员确认。   ← 受保护/DRM 窗口
图片过大，暂不支持编辑
截图中断，返回中断位置继续滚动
已达最大长度，无法继续截图
滚动页面截取更多内容
没有提取到文字 / 没有识别到二维码 / 二维码识别中 / 没有翻译结果
已添加到剪贴板 / 已复制色值
```

### 7.4 其它可观测行为

- 截图是一种**可发送的消息内容类型**：`[截图]`（`@185000545`，与 `[图片]`/`[视频]`/`[文字]` 并列）。
- 演示/投屏场景：「演示屏幕时不显示微信主窗口」「其他应用正在录制屏幕，已对其他应用隐藏微信主窗口」（`@185045022`、`@185045662`）——与 `SetWindowDisplayAffinity` 同一机制。

---

## 8. 配置项与开关全表

| 键 | 作用 |
|---|---|
| `gScreenshotShortcutKey` | 截图快捷键 |
| `gScreenShotHideWindow` | 截图时隐藏微信主窗口 |
| `clicfg_screen_privacy` / `gScreenPrivacyEnabled` / `clicfg_screen_privacy_reddot` | 隐私模式（防截屏） |
| `clicfg_pc_screenshot_toolbar_collapse` | 工具栏折叠 |
| `clicfg_screenshot_ask_ai_type` | 问 AI 入口形态 |
| `clicfg_yuanbao_screenshot_agreement_version` / `_content` / `g_kv_ask_ai_screen_shot_user_agreement` | 元宝 AI 截图用户协议 |
| `longscreenshot_use_grabwindow` | 长截图是否走 `GrabWindow` 路径 |
| `screenshot_use_getwindowrect` | 窗口定位方式 |
| `screenshot_qrcode` | 二维码识别开关 |
| `clicfg_xwechat_force_netocr` / `clicfg_xwechat_ocr_option` / `rec_threshold` / `det_threshold` | OCR 走网络识别及阈值 |

---

## 9. 线程模型、图像管线与资源

### 9.1 线程

| 线程 | 职责 | 可观测标识 |
|---|---|---|
| 主线程 | UI / 编排 / 渲染 | `wx::ScreenShoter` |
| 抓屏线程 | WGC/GDI 抓帧 | `longscreenshoter_grab_thread` |
| 拼接线程 | OpenCV 模板匹配 + 拼接 | `longscreenshoter_opencvworker_thread` |
| 拼接输出线程 | 生成预览与最终图 | `longscreenshoter_splice_thread` |

### 9.2 图像管线

- 载体：`QPixmap`（`QPixmap& pixmap`）、`sk_sp<SkImage>`（Skia）、`kernel::FileBufferPtr`（Chromium 风格的文件缓冲）
- 像素处理标记：`use_pixel`（走 CPU 像素路径 vs GPU 路径）
- 遮罩动画：`ShimmerStart` / `ShimmerReset` / `ShimmerFadeOut` / `UpdateShimmerExclusions`
- 剪贴板：`std::shared_ptr<QMimeData>`（`pinData` / `Captured`）
- 裁剪结果类型：`std::pair<QPixmap, QPoint>`（`snip`）

### 9.3 资源与格式

- Qt 资源路径：`:/gui/svg/preview/open_window.svg`、`:/icons/rotate`
- 保存过滤器：`PNG(*.png);;JPEG(*.jpg *.jpeg);;BMP(*.bmp)`
- 长截图命名：`yyyy-MM-dd_hhmmss_zzz(.png)`，日志前缀 `wechat_longscreenshot` / `wechat_longscreenshot_`，进度 `%1/%2`
- 其它日志前缀：`ScreenShot_`

---

## 10. OCR / AI 集成

| 层 | 符号 | 说明 |
|---|---|---|
| 服务 | `mmui::OCRClientService` | 客户端 OCR 服务 |
| 管理 | `kernel::manager::OCRManager::UploadImageCdn`（`ocr_manager.cc`） | **网络 OCR**：图片上传 CDN 后识别 |
| 协议 | `micromsg.NewOCRTranslationRequest` / `NewOCRTranslationResponse` | OCR+翻译 请求/响应 |
| 协议 | `mmremoting.ocr.protocol.OCRSupportMessage` / `OCRRect` | OCR 能力协商与文本框坐标 |
| 阈值 | `rec_threshold` / `det_threshold` | 识别/检测阈值 |
| AI | `ask_ai` / `ask_ai_with_prompt` / `prompt` | 元宝多模态入口，带协议同意流 |

> **重写提示**：OCR 与 AI 走腾讯私有协议，独立程序无法复用。若只做本地工具，应改用本地引擎（PaddleOCR / Windows.Media.Ocr）并自行实现「提取文字 / 翻译 / AI 打码」的等价能力。
>
> **2026-11-04 决策更新（用户指令：「AI 可以使用 deepseek、minimax 接入」）**：
> 不再「放弃复刻」，改为**接公开大模型服务**——问 AI / 翻译 / AI 一键打码三件套都落地，
> DeepSeek 与 MiniMax 都是 OpenAI 兼容接口，一套客户端按 provider 切 endpoint + model。
> 详细契约见 `实现契约.md` §7（AI 接入契约）。本表保留**逆向观察到的原始事实**，不改写。

---

## 11. 重写实施建议

### 11.1 技术选型

| 方案 | 优点 | 缺点 | 建议 |
|---|---|---|---|
| **Qt 6 Widgets**（C++） | 与微信同栈，`QScreen`/`QPixmap`/信号槽直接对应原设计，长截图拼接可用 OpenCV | 需 Qt 6 + MSVC 工具链，单文件约 40–80MB | **推荐** |
| 纯 Win32 + Direct2D | 依赖极少、启动快 | 浮层/DPI/多屏/PNG 编码全要手写，逼近 2k 行 | 次选 |
| C# / WinUI 3 | 开发效率高 | WGC 需 WinRT 互操作，复杂 | 不推荐 |

### 11.2 建议模块划分（对齐原类名，便于对照）

```
capture/  grab_worker.(h|cpp)      // WGC 抓帧 + GDI 兜底 + 排除 HWND
capture/  screen_filter.(h|cpp)    // 窗口选择器（window_filter_t）
ui/       shot_selection.(h|cpp)   // 选区浮层（ShotSelection）
ui/       editor_toolbar.(h|cpp)   // 工具栏 + 子菜单（WxShotEditormenu）
ui/       color_panel / font_select / size_select
ui/       pin_window.(h|cpp)       // 贴图窗（ScreenShotPinWindow）
long/     long_shooter.(h|cpp)     // 长截图总控（LongScreenShoter）
long/     opencv_worker / splice_worker
core/     screen_shoter.(h|cpp)    // 主编排（ScreenShoter）
core/     shot.(h|cpp)             // 入口（WxShot）
```

### 11.3 里程碑

| 阶段 | 内容 | 验收标准 |
|---|---|---|
| M0 | WGC 抓屏最小可用 | 能抓到全屏/指定窗口帧，PNG 落盘；`GetDC`/`BitBlt` 兜底可用 |
| M1 | 选区浮层 | 拖拽/缩放/锁定、Esc 取消、Enter 确认、尺寸提示、多屏 + 混合 DPI 正确 |
| M2 | 编辑器 | 矩形/椭圆/箭头/画笔/文字/马赛克/取色 + 撤销栈 + 复制到剪贴板 + 保存 |
| M3 | 长截图 | 滚动抓取 + 模板匹配拼接 + 中断续滚 + 最大长度保护 |
| M4 | 贴图窗 + AI 打码 | 常驻置顶窗（可拖动/缩放/圆角/透明度）+ AI/本地自动打码 |

### 11.4 已知坑位（重写必踩）

1. **WGC 不覆盖全屏独占/受保护窗口**：需 GDI `PrintWindow`/`BitBlt` 兜底，并对 DRM 窗口给出「设备环境不允许当前应用截图」提示。
2. **多屏 + 混合缩放**：`GetDpiForWindow` / `VirtualGeometry` 必须做，否则选区坐标在 125%/150% 屏上错位。
3. **截图时自隐藏**：`SetWindowDisplayAffinity` 必须在抓帧前设置、帧后恢复，否则会把自己截进去。
4. **WGC 需 GPU 拷贝**：帧格式转换（BGRA→RGBA）与 D3D11 设备丢失处理。
5. **剪贴板 PNG**：必须提供 `CF_DIB`/`PNG` 两种格式，否则 Chrome 等应用粘贴出黑图。
6. **长截图拼接**：滚动距离估计用 OpenCV 模板匹配，需要处理动画界面导致的匹配失败（对应 `show_broken_toast` 降级提示）。
7. **贴图窗点击穿透**：需要 `WS_EX_LAYERED | WS_EX_TRANSPARENT` 与 `SetLayeredWindowAttributes` 配合，仅用 `WS_EX_TRANSPARENT` 会导致无法拖动。

---

## 12. 未决问题（需动态调试确认）

| # | 问题 | 建议手段 |
|---|---|---|
| 1 | WGC 与 GDI 的实际选择条件（`longscreenshot_use_grabwindow` 的判定逻辑） | IDA 交叉引用 `longscreenshot_use_grabwindow` 字符串 |
| 2 | `ShotSelectionStatus` 的完整枚举取值 | 运行时 hook 或补齐 RTTI |
| 3 | 窗口选择器 `window_filter_t.showRegion` 的窗口枚举与过滤规则 | 动态观察 |
| 4 | AI 一键打码的模型与服务端协议 | 抓包 —— ~~该部分应直接放弃复刻~~ **（2026-11-04 更新：不再抓包，改为接 DeepSeek / MiniMax 的公开接口，见 `实现契约.md` §7）** |
| 5 | `ScreenShot_` 日志前缀下的完整日志点 | 跑一次全流程截图收集日志 |

---

## 13. 复现方法

```powershell
# 只读取证脚本（不改任何文件）
python D:\github\screenloom\doc\scripts\scan_wechat_shot.py

# 指定其它版本的 DLL
python D:\github\screenloom\doc\scripts\scan_wechat_shot.py "<path>\Weixin.dll"
```

脚本输出四部分，与本文档章节对应：

| 脚本输出 | 对应章节 |
|---|---|
| `[1] PE 结构` | §3 |
| `[2] wx 反射成员表` | §6 |
| `[3] 截图相关配置键 / 日志前缀` | §8、§9 |
| `[4] 中文 UI 文案` | §7 |

> **注意**：脚本用 `mmap` 分块读取。对 100MB+ 的二进制直接 `read()` 整个文件会因内存压力导致进程被杀（Windows 退出码 `4294967295`），不要改回整文件读取。

---

## 附录 A：关键证据偏移表（`Weixin.dll`，基准 = 文件头）

| 偏移 | 内容 |
|---|---|
| `149010602` | Qt `QScreen::grabWindow()` 弃用告警 |
| `149637744` / `149637920` | `gScreenshotShortcutKey` / `gScreenShotHideWindow` |
| `149676264` | `clicfg_screen_privacy` |
| `149990956` | 自有抓屏路径：`GetDC failed` / `BitBlt failed` / `win%1_%2.png` |
| `152265440–152265952` | 元宝 AI 截图协议相关键 |
| `152714184` | `ShotSelection` + 尺寸提示 `-- x -- ` / ` %1 x %2 ` |
| `152716160–152716352` | `longscreenshot_use_grabwindow`、三个长截图线程名、`wechat_longscreenshot` 前缀 |
| `152717376` | 保存过滤器 `PNG(*.png);;JPEG(*.jpg *.jpeg);;BMP(*.bmp)` |
| `152723072 / 152723168 / 152723208` | `screenshot_use_getwindowrect` / `screenshot_qrcode` / `ScreenShot_` |
| `152737808–152744300` | Linux GNOME Shell Screencast gjs 脚本（跨平台抽象证据） |
| `152742635` | `class ScreenshotUtils {`（Linux 侧） |
| `153904673` | `WS_EX_LAYERED` |
| `154471060` | `wxutility` / `wxocr` 插件名常量 |
| `155752465` | `ocr_manager.cc` / `clicfg_xwechat_force_netocr` / `rec_threshold` / `det_threshold` |
| `157275752` | `longscreenshot_toolbar_button_size` / `screenshot_toolbar_button_size` |
| `157287184` | `ScreenShotPinWindow` |
| `159031297` | 聊天右键菜单项 `kScreenShot` |
| `160094880` | WGC 接口名（UTF-16） |
| `163682351` | `Both PrintWindow() and BitBlt() failed.` |
| `163687576` | `third_party/webrtc/modules/desktop_capture/win/d3d_device.cc` |
| `185060400–185060960` | 截图编辑器中文文案全集 |
| `185899120–185907400` | **`wx` 反射成员表全段** |
| `185902056` 起 | `WxShotEditormenu` 成员表（尾部含 `line_fold_clicked` / `canvas::graphics_t` / `canUndo` / `paintGraph`） |
| `185905992` 起 | `ScreenShoter` 成员表尾部（`handle_keyboard_*` / `handle_mouse_dbclick` / `OpenOcrPreview` / `translate_image`） |
| `185932864` | `mmui::ScreenShotClientService` |
| `185939216` | `WindowPinnedStateChanged` / `HiddenWhenScreenShot` |
