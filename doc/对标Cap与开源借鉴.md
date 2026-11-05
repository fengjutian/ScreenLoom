# 对标 Cap：功能取舍与开源借鉴方案

> 状态：**方案待评审**（功能取舍与优先级已定；实施细节以各里程碑的 PR 为准）
> 定位：以 **Cap 的功能面**为对标基准，以 **OBS / ShareX / Cap 源码**为借鉴来源，产出 Screenloom 的功能取舍、架构改造与里程碑。
> 关联文档：[录屏方案.md](录屏方案.md)（MF 链路与踩坑）、[录屏功能需求.md](录屏功能需求.md)（需求与验收）、[实现契约.md](实现契约.md)（分层约束）、[../README.md](../README.md)（现状与命令）

---

## 0. 摘要

1. **Cap 是 SaaS 形态**（录制 → 立即上传 → 分享链接 → 评论/分析/团队），Screenloom 是**本地工具**。直接对标的是「捕获 → 录制 → 落盘」；「编辑 → 导出」本地化降级；「云与协作」不做。
2. Cap 值得抄的不是功能列表，而是**它怎么在出问题时活下来**：muxer 进程隔离、分片写入、崩溃恢复、输出校验。这恰好命中 [录屏方案.md](录屏方案.md) 风险 11 的「约 9% 录制会卡死」。
3. 三个参照物分工明确：**OBS 教媒体管线与时钟，ShareX 教交互与工作流，Cap 教可靠性工程**。
4. Cap 主仓是 AGPL-3.0，但 `scap-*` / `cap-camera*` 是 MIT——这是三个项目里**唯一可以合法复用代码**的部分。
5. 首版对标不追求「功能数量接近 Cap」，而是把 **P0 的可靠性**做到位，再按 P1 补齐录制体验。功能广度是陷阱。

---

## 1. 为什么对标 Cap，以及不对标什么

### 1.1 三个产品的形态差异

| | Cap | Screenloom |
|---|---|---|
| 形态 | 桌面端 + Web 端 + 媒体服务 + 数据库（SaaS 为主，可自托管） | 单机 Windows 应用 |
| 核心闭环 | 录制 → 上传 → 链接 → 观看/评论/分析 | 截图/录屏 → 本地文件 → 剪贴板/贴图 |
| 商业模式 | SaaS（Cloud / 自托管 / S3 自带） | 开源本地工具（MIT） |
| 数据归属 | 可选云端 | 天然全本地 |
| 桌面技术栈 | Rust + GPUI（自绘 UI） | Rust + Win32 自绘 |

**结论**：Cap 与 Screenloom 在「捕获与录制内核」上高度同构（都是 Rust + 平台原生捕获 + 自绘 UI），在「产品闭环」上完全不同。因此**对标内核、不对标闭环**。

### 1.2 三层判决

| 层 | 判决 | 说明 |
|---|---|---|
| 捕获 → 录制 → 落盘 | **直接对标** | 同一技术域，Cap 的工程答案可以直接检验我们的设计 |
| 编辑 → 导出 | **本地化降级** | 只做「裁剪 + 背景 + 圆角/阴影 + 缩放」四项，不做多轨时间轴、人工字幕轨、去口癖 |
| 上传 → 链接 → 协作 | **不做** | 与「本地、离线、零依赖」定位冲突 |

### 1.3 取舍判据

任何一项功能进入路线图，必须同时满足：

1. **服务本地场景**：不引入账号、不依赖网络（AI 类为可选增强）；
2. **不破坏依赖克制**：不引入 FFmpeg / 重型运行时（见 [录屏方案.md](录屏方案.md) §1）；
3. **可外部验证**：有 ffprobe / 单测 / 手工矩阵能证明「做到了」，不接受「调用没报错」。

---

## 2. 许可与借鉴边界

| 项目 | 许可 | 能否复用代码 | 说明 |
|---|---|---|---|
| **Cap** | AGPL-3.0；例外：`scap-*`、`cap-camera*` 为 **MIT** | **仅 MIT 部分可** | `scap-direct3d` / `scap-targets` / `scap-cpal` 等可直接依赖或改写 |
| **OBS Studio** | GPL-2.0-or-later | 否 | 只能看设计与算法思路，不能抄代码 |
| **ShareX** | GPL-3.0 | 否 | 同上 |
| FFmpeg / GStreamer | LGPL / GPL | 不引入 | 与「零新增依赖」冲突，仅作行为参照 |

> 红线：本项目 MIT。**引入任何 GPL/AGPL 代码都会导致整仓许可变更**。借鉴 = 读设计、重写实现；`scap-*` 若直接依赖，需在 `NOTICE`/README 保留 MIT 声明。

---

## 3. Cap 功能全景

按「用户能感知的能力」拆，不按代码模块拆。来源：[README](https://github.com/CapSoftware/Cap/blob/main/README.md)、[crates/](https://github.com/CapSoftware/Cap/tree/main/crates)（部分细节据仓库结构整理，以官方文档为准）。

| 功能域 | Cap 的做法 |
|---|---|
| 录制模式 | **Instant Mode**：边录边传，停止即出链接；**Studio Mode**：本地录制 + 编辑器 + 导出 |
| 捕获目标 | 屏幕 / 窗口选择器，多显示器 |
| 视频 | 分辨率、帧率、码率可配（具体档位以官方设置为准） |
| 音频 | 系统声 + 麦克风，双源 |
| 摄像头 | 画中画（camera 系列 crate） |
| 光标 | 独立采集（`cursor-capture` / `cursor-info`），项目模型里也有光标轨（`project/src/cursor.rs`） |
| 录制控制 | 倒计时、暂停/继续、快捷键、托盘/菜单栏入口 |
| 可靠性 | 分片写入（`recording/src/fragmentation/`）、崩溃恢复（`recovery.rs` + `recovery/`）、坏轨修复（`track_heal.rs`）、产物校验（`output_validation.rs`）、音画校准（`sync_calibration.rs`） |
| Muxer | 独立进程 `cap-muxer`，经 `cap-muxer-protocol` 与主进程通信 |
| 编辑 | 裁剪、背景、缩放、字幕、章节、导出控制（Studio） |
| 导出 | MP4 等本地导出；独立 `export` crate |
| 分享 | 链接、密码、评论、反应、转录、观看分析、自定义域名 |
| 团队 | 工作区、权限、自托管、S3 自带 |
| AI | 标题、摘要、可点击章节、字幕、转录 |
| 平台 | macOS / Windows / Linux |

---

## 4. 取舍矩阵

### 4.1 直接对标（做，并做到同档）

| 能力 | Cap 形态 | Screenloom 目标 | 落点 | 优先级/状态 |
|---|---|---|---|---|
| 监视器/窗口目标 | 选择器 | 已有 CLI 与托盘窗口录制，补 UI 选择器 | [windowpicker.rs](../screenloom-app/src/windowpicker.rs) | P1 |
| 区域录制 | 支持 | 复用截图选区浮层，固定区域录制 | [overlay/](../screenloom-ui/src/overlay/) | P1 |
| 帧率/码率可配 | 支持 | 已有配置项，补设置页与预设 | [config.rs](../screenloom-core/src/config.rs) | P1 |
| 系统声 + 麦克风 | 双源 | 采集与混音已实现（默认只开系统声），补电平/静音/设备选择 | [audio.rs](../screenloom-app/src/record/audio.rs) | P1 |
| 光标 | 独立采集 | 光标位置/点击记成数据轨，后期合成 | [source.rs](../screenloom-app/src/record/source.rs) + 新模块 | P1 |
| 暂停/继续 | 支持 | 已实现（暂停时间从成片剔除） | [pacer.rs](../screenloom-app/src/record/pacer.rs) | ✅ |
| 倒计时 | 支持 | 已实现 | [session.rs](../screenloom-app/src/record/session.rs) | ✅ |
| 录制可靠性 | 分片 + 恢复 + 校验 | **本方案 P0**，见 §6 | [record/](../screenloom-app/src/record/) | P0 |
| 本地导出 | MP4 | 已实现 | [encoder.rs](../screenloom-app/src/record/encoder.rs) | ✅ |
| 截图 | 有独立实现 | 已实现（区域/滚动/标注/OCR/贴图） | [flow.rs](../screenloom-app/src/flow.rs) | ✅ |

### 4.2 本地化降级（做，但换形态）

| Cap 形态 | Screenloom 形态 | 理由 | 优先级 |
|---|---|---|---|
| Instant Mode：停止即得链接 | **停止即得可播文件**：分片落盘 + 快速 remux，不等待转码、不阻塞 UI | 内核等价物是「快速完成」，不是「上传」 | P1 |
| Studio Mode：时间轴编辑器 | 轻量编辑器：**裁剪 / 背景 / 圆角阴影 / 缩放**，四项封顶 | 多轨时间轴是另一个产品的工作量 | P2 |
| 分享链接 + 评论 + 分析 | 本地交付：打开文件夹、复制路径、复制到剪贴板、贴图 | 用户要的是「发出去」，不是「平台」 | P1 |
| AI 标题/摘要/章节/字幕 | 可选增强：复用现有 AI 配置，做「摘要 + 章节」；字幕后置 | AI 是可选能力，不做内置依赖 | P3 |
| 摄像头画中画 | 暂不做 | 与「轻量截图/录屏」定位不符 | 不做 |
| 多平台（mac/Linux） | 只做 Windows | OCR/捕获/编码均依赖系统 API | 不做 |

### 4.3 不做（及理由）

| 不做项 | 理由 |
|---|---|
| 账号体系、团队工作区、权限 | 本地工具无此需求 |
| 云上传、S3 自带、自托管 | 与「数据不出本机」的定位相反 |
| 评论、反应、观看分析 | 需要服务端，且非核心场景 |
| 浏览器端播放/嵌入 SDK | 同上 |
| Loom 导入 | 无存量迁移诉求 |
| 完整时间轴编辑器、多轨、字幕轨 | 范围失控风险最高的一项 |

---

## 5. 开源借鉴分工

| 主题 | 主参照 | 可借鉴 | 不可借鉴 |
|---|---|---|---|
| 帧节奏、时间戳、编码器抽象、输出停机顺序 | **OBS** | 固定 tick + 落后计数；`stop → flush → finalize` 顺序；muxer 子进程隔离 | 插件系统、场景合成器、Qt/`gs_*` 抽象 |
| 选区交互、热键预设、动作链、文件生命周期 | **ShareX** | 放大镜/微调/重复上次；一热键一套 TaskSettings；命名模板 + 自动清理 | 80+ 上传目标、工具大杂烩、WinForms |
| 可靠性工程、进程隔离协议、光标轨、合成源测试 | **Cap** | 分片/恢复/校验三件套；`cap-muxer-protocol` 设计；`test_sources` | 云/协作层、编辑器、FFmpeg 编码底座 |
| DXGI 捕获、GPU 色彩转换 | **Cap（MIT 部分）** | `scap-direct3d` 的 Duplication 重建逻辑；`gpu-converters` 仅看设计 | `gpu-converters` 属 AGPL，不能抄代码 |
| 零拷贝收益量化 | wl-screenrec | CPU 500% → 2.5% 的对照，用于论证 M4 | — |
| 光标合成、去重复帧 | ScreenToGif | 三种光标绘制、重复帧剔除 | C#/WPF 实现 |

> 一句话：**OBS 管帧，ShareX 管流程，Cap 管活命。**

---

## 6. 架构级借鉴（与具体功能解耦）

这六项不新增用户可见功能，但决定「功能能不能可靠交付」。

### 6.1 Muxer / 编码进程隔离

**问题**：MF `SinkWriter` 偶发永久阻塞，现有 8 秒看门狗只能把整个会话判死（风险 11）。

**参照**：Cap 的 `cap-muxer` 独立二进制 + `cap-muxer-protocol`；OBS 的 `obs-ffmpeg-mux`。

**方案**：把 MF 的 **SinkWriter（编码 + 封装）整体移入子进程**——风险 11 的阻塞点在 `WriteSample` 内部，只搬 muxer 解决不了。主进程传帧走**共享内存 + 句柄**（NV12 帧零拷贝映射），子进程回传统计与错误，协议最小集：

```text
Init(width, height, fps, bitrate) → Frame(shared_handle, pts) → Flush → Finalize → Done
Abort / 心跳 / 错误码
```

若将来自研编码器（M4 GPU 路径），可退化为「主进程编码、子进程只做封装」，此时跨进程传的是**已编码样本**，带宽问题消失——这正是 Cap `cap-muxer` 的形态。

子进程卡死时：kill → 保留已写分片 → 主进程上报「编码器无响应」，UI 不挂。

**落点**：新增 `screenloom-encoder-host`（bin）+ `screenloom-encoder-protocol`（lib）。
**判据**：人为在子进程注入 10s 阻塞，主进程仍可停止、可保存、可再次录制。

### 6.2 分片 + manifest + 恢复 + 校验

**问题**：MP4 的 `moov` 在文件尾，中途被杀即不可播（风险 4 / REC-65）。

**参照**：Cap `fragmentation/`、`recovery.rs`、`track_heal.rs`、`output_validation.rs`；OBS Hybrid MP4。

**方案**：

1. 每 10s（关键帧对齐）切一个分片，同时追加 manifest：`{序号, 起始 pts, 帧数, 字节数, 是否完整}`；
2. 正常停止：按 manifest 顺序 remux 成单文件；
3. 启动时扫描残留：缺尾片则愈合到最后可解码处，标记 `.partial`；
4. 收尾后校验产物（编码/时长/帧数/音轨），失败不改成正式名。

**落点**：[paths.rs](../screenloom-app/src/record/paths.rs)、[session.rs](../screenloom-app/src/record/session.rs)、[mux.rs](../screenloom-app/src/record/mux.rs)。
**前置探针（已验证）**：MF 支持 `MFTranscodeContainerType_FMPEG4`，且分片文件在**不 Finalize 的情况下仍可解码、可 remux**。实测数据见 [录屏方案.md](录屏方案.md) §4.8（探针 8）。因此分片方案不需要自写 muxer，直接在 MF 上落地。

### 6.3 光标数据轨

**问题**：光标现在只是「录/不录」的布尔；DXGI 路径会丢硬件光标（风险 9）；后期无法调整。

**参照**：Cap `cursor-capture` / `cursor-info` / `project/src/cursor.rs`；ScreenToGif 的三类光标绘制。

**方案**：120Hz 采样 `GetCursorPos` + 低级鼠标钩子记录点击，输出一条独立时间线；合成阶段按选择绘制（开/关、样式、点击高亮、平滑插值）。

**落点**：[source.rs](../screenloom-app/src/record/source.rs) + 新 `cursor.rs`；[encoder.rs](../screenloom-app/src/record/encoder.rs) 之前叠加。
**判据**：同一段录制，光标开关不影响画面其余部分；多屏/混合 DPI 下位置误差 ≤ 1px。

### 6.4 合成源回归测试

**问题**：[probes/](../probes/) 是人工探针，改编码参数只能人肉复验；风险 11 这类偶发问题没有回归网。

**参照**：Cap `test_sources/` + `cap-test` + `verify*`。

**方案**：内置「已知图案帧源」（色块/滚动条/时间码），跑完整管线，断言帧数、时长、颜色、音画偏差；接入 `cargo test`。

**判据**：任何编码层改动都能在无屏幕环境下跑通端到端断言。

### 6.5 GPU 色彩转换

**问题**：CPU BGRA→NV12 在 1080p@30 约 93MB/s 输入，占 1~2 核（风险 2）。

**参照**：`scap-direct3d`（MIT，可直接参考/依赖）、Cap `gpu-converters`（仅看设计）、wl-screenrec 的收益量化。

**方案**：D3D11 视频处理器做 NV12 转换，MF 侧走 DXGI 设备管理器；CPU 路径保留兜底。

**判据**：1080p30 CPU 占用 ≤ 15%（REC-105）。

### 6.6 诊断与脱敏

**问题**：日志会带窗口标题、应用名、路径；用户发诊断包即泄露隐私（REC-107 只有日志，无脱敏）。

**参照**：Cap `diagnostics.rs`、`log_redaction.rs`；OBS 的日志分析器与 Safe Mode。

**方案**：结构化日志（阶段/耗时/错误码）+ 一键导出诊断包 + 标题/路径脱敏 + 异常退出后「安全模式」（禁用硬件编码/音频启动）。

---

## 7. 功能级借鉴清单

### P0 · 可靠性基线（先做这个，不做就是给用户埋雷）

| # | 事项 | 参照 | 落点 | 验收判据 |
|---|---|---|---|---|
| P0-1 | 产物校验 | Cap `output_validation` | 录后流程 | 每次录制输出 codec/时长/帧数/音轨校验结果；失败保留 `.partial` 并提示 |
| P0-2 | 编码进程隔离 | Cap muxer + OBS mux | 新增 `encoder-host` + 协议 lib | 注入阻塞后主进程不挂、可停止、文件可播 |
| P0-3 | 失败原因区分 | 需求 REC-60 | [session.rs](../screenloom-app/src/record/session.rs) | HUD/日志能区分：用户停止/超时/磁盘/目标消失/编码错误 |
| P0-4 | 实时统计 | OBS Stats | [stats.rs](../screenloom-app/src/record/stats.rs)、[hud.rs](../screenloom-app/src/record/hud.rs) | 显示时长/帧数/丢帧/码率/大小，1Hz 刷新 |
| P0-5 | 退出/注销收尾 | 需求 REC-09 | [main.rs](../screenloom-app/src/main.rs) | `WM_QUERYENDSESSION` 与 Ctrl+C 都能 Finalize |

### P1 · 录制体验对标

| # | 事项 | 参照 | 落点 | 验收判据 |
|---|---|---|---|---|
| P1-1 | 分片 + manifest + 启动恢复 | Cap `fragmentation`/`recovery` | record 模块 | 强杀后重启能恢复到最后一片；成片时长误差 <0.5s |
| P1-2 | 光标数据轨 | Cap cursor | 新 `cursor.rs` | 光标开关/样式/点击高亮可用；DPI 正确 |
| P1-3 | 双源音频完善 | Cap audio | [audio.rs](../screenloom-app/src/record/audio.rs) | 分轨音量/静音/设备选择；音画偏差 ≤100ms |
| P1-4 | 区域/窗口录制 UI | ShareX 选区 | [windowpicker.rs](../screenloom-app/src/windowpicker.rs) | 复用截图浮层，窗口被遮挡仍可录，窗口关闭自动收尾 |
| P1-5 | 文件命名模板 + 自动清理 | ShareX | [save.rs](../screenloom-app/src/save.rs) | 模板变量生效、重名不覆盖、按天数/数量清理 |
| P1-6 | 热键预设 + 动作链 | ShareX Quick Task | [hotkey.rs](../screenloom-app/src/hotkey.rs)、[flow.rs](../screenloom-app/src/flow.rs) | 一热键 = 捕获方式 + 后续动作；冲突明确报错 |
| P1-7 | 录制后动作 | ShareX | [controller.rs](../screenloom-app/src/record/controller.rs) | 打开文件夹/复制路径/可选转码，可配置 |

### P2 · 编辑与导出（Studio 的本地版）

| # | 事项 | 参照 | 说明 |
|---|---|---|---|
| P2-1 | 轻量编辑器 | Cap Studio | 裁剪、背景、圆角阴影、缩放，四项封顶 |
| P2-2 | GIF / 多分辨率导出 | Cap enc-gif | 多分辨率 MP4 复用现有编码层；GIF 需独立编码器，仅看设计后自写，不引入依赖 |
| P2-3 | 章节标记 | Cap markers | 录制中热键打点，导出章节文件 |
| P2-4 | GPU 色彩转换 | scap-direct3d | 见 §6.5 |

---

## 8. 里程碑

| 里程碑 | 交付物 | 验收判据 | 依赖 |
|---|---|---|---|
| **M1 可靠性基线** | P0-1~P0-5 | 连续 50 次录制零坏文件；注入阻塞可恢复；诊断日志可读 | 无 |
| **M2 录制对标** | P1-1~P1-4 | 强杀恢复成功；光标可开关；双源音频同步；区域/窗口 UI 可用 | M1 的进程隔离 |
| **M3 体验打磨** | P1-5~P1-7 | 命名/清理/动作链/预设可用，且配置可迁移 | M2 |
| **M4 编辑导出** | P2-1~P2-4 | 四项编辑能力可用；1080p30 CPU ≤15% | M3 |

> 排期原则：**M1 不产生新功能，但必须先做**。理由见 [录屏方案.md](录屏方案.md) 风险 11——可靠性问题不是 bug，是概率事件，晚做一天就多一批「录制中程序卡死」的用户投诉。

---

## 9. 风险与待决策

| # | 风险 | 影响 | 对策 |
|---|---|---|---|
| 1 | 进程隔离引入 IPC 与帧传输成本 | 延迟、复杂度上升 | 共享内存传帧；先整体隔离 SinkWriter，性能不达标再评估「自研编码器 + 轻封装」两段式 |
| 2 | MF 分片容器不可用（**已排除**） | 无 | 探针 8 实测 FMPEG4 可用、崩溃后可读可 remux；按方案 6.2 落地，验收时用真实录制复核丢帧 |
| 3 | 光标轨跨 DPI/多屏坐标 | 光标位置错位 | 统一虚拟屏物理像素坐标；单测覆盖 100/125/150/200% |
| 4 | 编辑器范围失控 | 拖垮主线 | 四项封顶，写进验收；超出走 issue 讨论 |
| 5 | 许可污染 | 整仓许可被迫变更 | 代码审查检查来源；`scap-*` 依赖需保留声明 |
| 6 | 4K/60 性能未验证 | 高分辨率掉帧 | M2 前补性能矩阵（风险 1/2 实测） |

**待决策**：

| 议题 | 选项 | 建议 |
|---|---|---|
| 分片时长 | 5s / 10s / 30s | **10s**（恢复粒度与文件数平衡） |
| 子进程范围 | 仅 muxer / 编码 + muxer | **编码 + muxer**（卡死根因在编码侧） |
| 光标叠加时机 | 录制时 / 后期合成 | **后期合成**（可开关、可换样式） |
| 编辑器载体 | 内嵌浮层 / 独立窗口 | **独立窗口**（空间足够，且不干扰截图浮层） |

---

## 10. 附录

### A. Cap 仓库导览（读哪里）

| Crate / 路径 | 看什么 |
|---|---|
| [crates/recording/src](https://github.com/CapSoftware/Cap/tree/main/crates/recording/src) | 录制编排总入口；`lib.rs` 先看模块关系 |
| [fragmentation/](https://github.com/CapSoftware/Cap/tree/main/crates/recording/src/fragmentation) | 分片写入与边界处理 |
| [recovery.rs](https://github.com/CapSoftware/Cap/blob/main/crates/recording/src/recovery.rs) / `recovery/` | 崩溃恢复的状态机 |
| [track_heal.rs](https://github.com/CapSoftware/Cap/blob/main/crates/recording/src/track_heal.rs) | 截断轨的修复策略 |
| [output_validation.rs](https://github.com/CapSoftware/Cap/blob/main/crates/recording/src/output_validation.rs) | 产物校验项清单 |
| [cap-muxer-protocol](https://github.com/CapSoftware/Cap/tree/main/crates/cap-muxer-protocol) | 进程间协议设计 |
| [crates/project/src/meta.rs](https://github.com/CapSoftware/Cap/blob/main/crates/project/src/meta.rs) | 录制项目元数据模型 |
| [sync_calibration.rs](https://github.com/CapSoftware/Cap/blob/main/crates/recording/src/sync_calibration.rs) | 音画校准 |
| [test_sources/](https://github.com/CapSoftware/Cap/tree/main/crates/recording/src/test_sources) | 合成源测试 |
| [scap-direct3d](https://github.com/CapSoftware/Cap/tree/main/crates/scap-direct3d)（MIT） | DXGI Duplication 的 Rust 实现 |

### B. 现状盘点

| 能力 | 现状 | 说明 |
|---|---|---|
| 截图（区域/滚动/标注/OCR/贴图） | ✅ | 见 [README.md](../README.md) |
| H.264 / MP4 录制 | ✅ | WGC + MF，BT.709 |
| 暂停/继续、倒计时 | ✅ | 暂停时间从成片剔除 |
| 系统声 + 麦克风采集与混音 | ⚠️ | 已实现，默认只开系统声；缺电平/静音/设备选择 |
| 托盘/热键/HUD | ✅ | HUD 显示计时与暂停/停止 |
| 分片/崩溃恢复/产物校验 | ❌ | 本方案 P0/P1 |
| 光标数据轨 | ❌ | 目前仅布尔开关，且 GDI 路径无光标 |
| 编辑器 | ❌ | 仅截图标注，无视频编辑 |
| 热键预设/动作链/命名模板 | ❌ | [flow.rs](../screenloom-app/src/flow.rs) 行为硬编码 |

### C. 参考

- Cap：[仓库](https://github.com/CapSoftware/Cap)、[crates](https://github.com/CapSoftware/Cap/tree/main/crates)、[LICENSE](https://github.com/CapSoftware/Cap/blob/main/LICENSE)
- OBS：[libobs](https://github.com/obsproject/obs-studio/tree/master/libobs)、[Hybrid MP4 实现记录](https://obsproject.com/blog/obs-studio-hybrid-mp4)
- ShareX：[仓库](https://github.com/ShareX/ShareX)、[TaskSettings.cs](https://github.com/ShareX/ShareX/blob/develop/ShareX/TaskSettings.cs)
- 本文与 [录屏功能需求.md](录屏功能需求.md) 的关系：**本文定取舍与优先级，需求文档定验收细则**；P0/P1 项在实施前应回填为 REC-xx 条目。
