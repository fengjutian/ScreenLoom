/**
 * 站点内容数据。
 *
 * 所有事实性描述都以仓库当前状态为准（README.md / doc/ / 源码），
 * 不写「计划中」的功能为已实现。
 */

/** 顶部导航。 */
export const NAV_ITEMS = [
  { id: 'features', label: '功能' },
  { id: 'gallery', label: '界面' },
  { id: 'cli', label: '命令行' },
  { id: 'architecture', label: '架构' },
  { id: 'build', label: '构建' },
] as const

/** 功能卡片。 */
export interface Feature {
  title: string
  desc: string
  /** 标签：本地能力 / 需要 API key / 实验中 */
  tag?: string
  tagTone?: 'local' | 'needs-key' | 'wip'
}

export const FEATURES: Feature[] = [
  {
    title: '区域截图与跨屏选区',
    desc: '基于 WGC 的持续帧源抓取整块虚拟屏，再按物理像素裁剪选区，跨显示器拖拽也能一次框住。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '标注与编辑',
    desc: '矩形、圆、箭头、直线、画笔、马赛克、文字，配色板与撤销重做，编辑后的图片可直接复制或存盘。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '滚动长截图',
    desc: '用户手动滚动页面，程序按像素匹配自动拼接多屏长图，Enter 继续拼接、Esc 结束。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '本地 OCR 提取文字',
    desc: '调用系统自带的 Windows.Media.Ocr 识别图中文字，结果面板展示并写入剪贴板，全程离线。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '二维码识别',
    desc: '纯 Rust 解码器 rqrr，截图后自动扫描图像中的二维码并列出内容。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '问 AI / 翻译 / AI 打码',
    desc: '把选区图片连同问题发给模型，支持 MiniMax 与 DeepSeek；翻译会先做本地 OCR；打码由模型标出敏感区域再转成马赛克。',
    tag: '需要 API key',
    tagTone: 'needs-key',
  },
  {
    title: '录屏导出 MP4',
    desc: 'WGC 持续抓帧，经 BGRA→NV12 转换后交给 Media Foundation SinkWriter 编码成 H.264，支持热键、托盘与 HUD 计时控制。',
    tag: '本地能力',
    tagTone: 'local',
  },
  {
    title: '录制系统声音',
    desc: 'WASAPI 环回采集系统输出，编码为 AAC 音轨与画面一起封装进 MP4；麦克风采集尚未实现。',
    tag: '开发中',
    tagTone: 'wip',
  },
]

/** 命令行参数表。 */
export interface CliRow {
  flag: string
  desc: string
}

export const CLI_ROWS: CliRow[] = [
  { flag: '--once', desc: '截一次图（复制到剪贴板 + 存盘）后退出' },
  { flag: '--long', desc: '长截图（用户滚动 + 自动匹配拼接）模式' },
  { flag: '--sel x,y,w,h', desc: '脚本化截图：直接截指定区域，坐标是虚拟屏物理像素' },
  { flag: '--ocr', desc: '识别剪贴板里图片的文字（本地 Windows.Media.Ocr）' },
  { flag: '--record N', desc: '录屏 N 秒，输出 H.264 / MP4' },
  { flag: '--record-out PATH', desc: '指定录制输出文件（必须是 .mp4）' },
  { flag: '--record-monitor N', desc: '录制指定显示器下标（默认主显示器）' },
  { flag: '--record-fps N', desc: '录制帧率（1–120）' },
  { flag: '--record-bitrate B', desc: '录制码率（bps）' },
  { flag: '--record-countdown N', desc: '开始前倒计时秒数（0–10）' },
  { flag: '--record-settings', desc: '打开录屏设置窗口（帧率 / 码率 / 目录 / 显示器 / 指针 / HUD）' },
  { flag: '--settings', desc: '打开设置窗口' },
  { flag: '--hotkey KEY', desc: '临时覆盖热键，如 Ctrl+Shift+A / Alt+F9' },
  { flag: '--config PATH', desc: '指定配置文件' },
  { flag: '--theme PATH', desc: '主题覆盖 JSON，文件改动自动热重载' },
  { flag: '--ask "问题"', desc: '问 AI（图片默认取剪贴板）' },
  { flag: '--translate [语言]', desc: '翻译图里的文字（默认英文）' },
  { flag: '--ai-mosaic', desc: 'AI 一键打码（打码后存盘 + 复制）' },
  { flag: '--provider NAME', desc: '指定服务商：minimax（默认）/ deepseek' },
  { flag: '--model NAME', desc: '指定模型，如 MiniMax-M3' },
  { flag: '--ai-probe', desc: '连通性探针，不需要 key（预期 401 = 链路通）' },
]

/** 架构分层：依赖方向单向 app → ui → capture → core。 */
export interface Layer {
  name: string
  desc: string
  rule: string
}

export const LAYERS: Layer[] = [
  {
    name: 'screenloom-app',
    desc: '应用层：main / 热键 / 托盘 / 设置 / 主题 / 剪贴板 / AI / OCR / 存盘 / 录屏',
    rule: '设置窗口的 Win32 逻辑全部集中在这里',
  },
  {
    name: 'screenloom-ui',
    desc: '渲染交互：GDI+ 管线 / 分层浮窗 / 选区 / 编辑器 / 工具栏 / 贴图窗',
    rule: '不许依赖 screenloom-app，避免循环依赖',
  },
  {
    name: 'screenloom-capture',
    desc: '抓屏：WGC 主路径 + GDI 兜底 + 显示器枚举 + 虚拟屏整屏合成 + 录屏持续帧源',
    rule: '必须提供整块虚拟屏图像，而不是只抓主屏',
  },
  {
    name: 'screenloom-core',
    desc: '纯逻辑：几何 / 图像 / 标注 / 撤销 / 长截图拼接 / NV12 转换 / 配置 / 二维码',
    rule: '不许引 Win32，保持可移植、可单测',
  },
]

/** 已知限制：诚实地写清楚边界。 */
export interface Limit {
  title: string
  desc: string
}

export const LIMITS: Limit[] = [
  {
    title: '只做 Windows',
    desc: 'core 层保持可移植，但目前不实现 macOS / Linux，录屏依赖 WGC 与 Media Foundation。',
  },
  {
    title: 'AI 端到端调用尚未验证',
    desc: '本机没有 API key。已验证的是请求体构造、响应解析与 bbox 解析；两家端点都能拿到 HTTP 401，说明链路通。',
  },
  {
    title: '录屏还缺区域与窗口',
    desc: '当前支持单显示器录制。区域 / 窗口 / 整块虚拟屏录制，以及麦克风采集、崩溃恢复都还没做。',
  },
  {
    title: '长截图靠你手动滚',
    desc: '是「用户滚动 + 自动匹配拼接」，不是微信那种自动滚动；真实滚动场景仍建议人工复验。',
  },
  {
    title: '不做微信内表情与转发',
    desc: '依赖腾讯私有协议与服务端能力，明确不实现。',
  },
]

/** 架构依赖流向。 */
export const ARCH_FLOW = 'app → ui → capture → core'

/** 构建与运行要点。 */
export const BUILD_STEPS = [
  {
    title: '准备环境',
    desc: 'Windows 10 1903+ 或 Windows 11（需 WGC 与 Windows.Media.Ocr），Rust MSRV 1.75。',
    code: 'rustc --version',
  },
  {
    title: '编译产物',
    desc: 'release 构建产出 target/release/screenloom.exe，只依赖 5 个 crate。',
    code: 'cargo build --release -p screenloom-app --bins',
  },
  {
    title: '常驻运行',
    desc: '注册全局热键（默认 Alt+A）并驻留托盘图标，也可用参数直接进入某个功能。',
    code: '.\\target\\release\\screenloom.exe',
  },
]
