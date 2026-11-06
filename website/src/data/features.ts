export const NAV_ITEMS = [{ id: 'features', label: '能力' }, { id: 'use-cases', label: '场景' }, { id: 'gallery', label: '界面' }, { id: 'cli', label: '命令行' }] as const

export interface Feature { title: string; desc: string; tag?: string; tagTone?: 'local' | 'needs-key' | 'wip' }
export const FEATURES: Feature[] = [
  { title: '区域截图与标注', desc: '框选任意屏幕区域，直接添加矩形、箭头、文字与马赛克；支持撤销重做、复制、保存和置顶。', tag: '本地', tagTone: 'local' },
  { title: '滚动长截图', desc: '手动滚动页面，程序按像素匹配并自动拼接，适合保存网页、聊天记录和长文档。', tag: '本地', tagTone: 'local' },
  { title: '文字与二维码识别', desc: '调用 Windows.Media.Ocr 提取图片文字，同时识别二维码；结果可直接复制，离线也能使用。', tag: '本地', tagTone: 'local' },
  { title: 'H.264 屏幕录制', desc: '录制指定显示器并导出 MP4，支持帧率、码率、倒计时、指针和系统声音设置。', tag: '本地', tagTone: 'local' },
  { title: '问图、翻译与智能打码', desc: '对选区内容提问、翻译文字，或让模型定位敏感区域后自动打码；凭证保存在本机配置中。', tag: '需 API Key', tagTone: 'needs-key' },
  { title: '脚本化调用', desc: '截图、OCR、录屏和 AI 能力均提供命令行入口，可接入快捷方式、批处理与自动化流程。', tag: 'CLI', tagTone: 'local' },
]

export const USE_CASES = [
  { label: '沟通', title: '把问题讲清楚', desc: '截图后立即加箭头、框选和文字说明，减少来回解释。', flow: '选区 → 标注 → 复制' },
  { label: '整理', title: '把信息带走', desc: '从界面、图片或长页面中提取文字与完整内容。', flow: '截图 → OCR / 拼接 → 保存' },
  { label: '演示', title: '把过程录下来', desc: '带上系统声音录制操作过程，直接获得可分享的 MP4。', flow: '选择屏幕 → 录制 → MP4' },
] as const

export interface CliRow { flag: string; desc: string }
export const CLI_ROWS: CliRow[] = [
  { flag: '--once', desc: '完成一次截图，复制并保存后退出' }, { flag: '--long', desc: '进入手动滚动、自动拼接的长截图模式' },
  { flag: '--ocr', desc: '识别剪贴板图片中的文字' }, { flag: '--record N', desc: '录屏 N 秒并导出 H.264 / MP4' },
  { flag: '--settings', desc: '打开统一设置窗口' }, { flag: '--ask "问题"', desc: '针对剪贴板图片向已配置的模型提问' },
]

export interface Limit { title: string; desc: string }
export const LIMITS: Limit[] = [
  { title: '目前仅支持 Windows', desc: '截图、OCR 和录屏使用 Windows 原生接口，暂不提供 macOS 或 Linux 版本。' },
  { title: '录屏以显示器为单位', desc: '当前支持单显示器录制；区域、窗口与整块虚拟屏录制尚未完成。' },
  { title: '麦克风采集尚未实现', desc: '可以录制系统输出声音，但当前版本还不能同时采集麦克风。' },
  { title: 'AI 能力需要自行配置', desc: '问图、翻译和智能打码需要用户提供支持的服务商 API Key。' },
]

export const BUILD_STEPS = [
  { title: '检查工具链', desc: '确认本机已安装 Rust 1.75 或更高版本。', code: 'rustc --version' },
  { title: '编译 release 版本', desc: '在仓库根目录构建 screenloom 可执行文件。', code: 'cargo build --release -p screenloom-app --bins' },
  { title: '启动常驻模式', desc: '注册全局热键并显示托盘图标。', code: '.\\target\\release\\screenloom.exe' },
] as const
