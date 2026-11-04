//! 全屏分层浮窗：选区 → 编辑态 → 工具栏。
//!
//! 坐标系（重要）：
//! * **客户区像素** = 屏幕物理像素 − 浮窗原点 = 背景帧像素坐标（浮窗正好铺在虚拟屏幕上）。
//! * **逻辑坐标** = 客户区像素 ÷ `scale`，UI 模型（选区、标注、工具栏）都存在这一层。
//! * 逻辑 ↔ 物理 的换算只走 `Rect::to_physical` / `Rect::to_logical`。

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, SetTextColor, HDC};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

use screenloom_core::{
    inpaint::inpaint_region, sample_color, Annotation, Canvas, Point, Rect, Result, Rgba,
    RgbaImage, ShapeKind, Tool, BORDER, MASK_ALPHA, TEXT,
};

use crate::aipanel;
use crate::colorpanel;
use crate::gdiplus::{self, Graphics};
use crate::render::{paint_annotations, pargb_to_rgba};
use crate::toolbar::{ItemKind, Mapper, Toolbar};
use crate::win::{self, LayeredTarget};

// ---------------------------------------------------------------- 子模块
//
// 拆分自原本的单文件 `overlay.rs`：状态生命周期 / 坐标几何 / 自绘 / 交互 /
// 工具栏动作 / AI 通道 / 文字输入各管一摊，公开 API 仍在 `mod.rs`。

mod actions;
mod ai;
mod geom;
mod interact;
mod paint;
mod state;
mod text;

// 只含 `impl OverlayState` 的模块不需要再导出；`geom` / `text` 里有自由函数与类型，
// 兄弟模块通过 `use super::*` 取。
pub(crate) use geom::*;
pub(crate) use text::*;

const CLASS_NAME: &str = "ScreenloomOverlayWindow";

/// AI 回复回投用的自定义窗口消息。
///
/// app 层在**后台线程**里调完模型后，用 [`post_ai_reply`] 把结果送到浮窗；
/// 浮窗的消息循环收到它再刷新界面 —— 这样 AI 调用**不会卡住 UI 线程**。
pub const WM_AI_REPLY: u32 = WM_APP + 1;

/// 窗口选择结果回投用的自定义窗口消息。
///
/// app 层弹出「窗口选择器」后，用 [`post_window_picked`] 把用户选中的窗口矩形
/// 送回浮窗；浮窗收到后把选区切到那个窗口。
pub const WM_WINDOW_PICKED: u32 = WM_APP + 2;

/// 浮窗发给 app 层的 AI 请求。
#[derive(Clone, Debug)]
pub enum AiRequest {
    /// 问 AI：图片 + 用户输入的问题。
    Ask {
        /// 用户输入的问题。
        question: String,
        /// 选区内的图像（裁剪后，物理像素）。
        image: RgbaImage,
    },
    /// 翻译选区图片里的文字。
    Translate {
        /// 选区内的图像（裁剪后，物理像素）。
        image: RgbaImage,
    },
    /// AI 一键打码：让模型标出敏感区域。
    Mosaic {
        /// 选区内的图像（裁剪后，物理像素）。
        image: RgbaImage,
    },
}

/// 一个待打码的区域（**相对选区左上角**的像素）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MosaicRect {
    /// 左上角 x。
    pub x: i32,
    /// 左上角 y。
    pub y: i32,
    /// 宽。
    pub w: i32,
    /// 高。
    pub h: i32,
}

/// app 层回给浮窗的 AI 结果。
///
/// 这个类型不只装 AI 的回复 —— **任何异步回投**都走它
/// （OCR 文本、二维码内容…），因为它们共用同一套
/// 「后台线程 → [`post_ai_reply`] → 浮窗刷新」的管道。
#[derive(Clone, Debug, PartialEq)]
pub enum AiReply {
    /// 文本结果（回答 / 译文）。
    Text(String),
    /// 出错信息。
    Error(String),
    /// 打码区域（相对选区左上角的像素）。
    MosaicRegions(Vec<MosaicRect>),
    /// 识别到的二维码内容（每个码一条文本）。
    QrCodes(Vec<String>),
}

/// 把 AI 结果送回浮窗（**可从后台线程调用**）。
///
/// 文本通过堆指针随 `LPARAM` 传递，浮窗收到消息后接管并释放。
pub fn post_ai_reply(hwnd: HWND, reply: AiReply) {
    let boxed = Box::into_raw(Box::new(reply));
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_AI_REPLY, WPARAM(0), LPARAM(boxed as isize));
    }
}

/// 把「用户选中的窗口矩形」送回浮窗（**可从后台线程调用**）。
///
/// 矩形是**背景帧内的物理像素坐标**（与 [`OverlayOutcome::Selected::region`] 同坐标系）。
pub fn post_window_picked(hwnd: HWND, region: Rect) {
    let boxed = Box::into_raw(Box::new(region));
    unsafe {
        let _ = PostMessageW(
            Some(hwnd),
            WM_WINDOW_PICKED,
            WPARAM(0),
            LPARAM(boxed as isize),
        );
    }
}

/// 浮窗配置。
#[derive(Clone, Copy, Debug)]
pub struct OverlayConfig {
    /// 虚拟屏幕左上角的 **设备（物理）坐标**。
    ///
    /// 背景帧就是从虚拟屏幕左上角开始截的，所以 `图像内坐标 = 屏幕物理坐标 - virtual_origin`。
    pub virtual_origin: Point,
    /// 缩放比（物理像素 ÷ 逻辑像素）。
    pub scale: f32,
    /// 浮窗是否对截屏程序隐藏（`WDA_EXCLUDEFROMCAPTURE`）。
    pub hide_from_capture: bool,
}

/// 用户在工具栏上点击的最终动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    /// 「完成」/ 编辑结束 —— app 自行复制 + 存盘
    Confirm,
    /// 「复制」—— app 只需写剪贴板
    Copy,
    /// 「保存图片」—— app 只需落盘
    Save,
    /// 「置顶在桌面」—— app 需要弹出贴图窗
    Pin,
    /// 「滚动截图」—— app 需要对当前选区跑滚动拼接
    LongShot,
}

/// 浮窗的最终结果。
#[derive(Debug)]
pub enum OverlayOutcome {
    Cancelled,
    Selected {
        /// 选区，**背景帧内的物理像素坐标**（可直接 `frame.image.crop(region)`）。
        region: Rect,
        /// 标注，坐标相对 **裁剪后图像左上角**（0,0）。
        canvas: Canvas,
        action: OverlayAction,
        /// 被「消除杂物」改过的背景（选区裁剪后，物理像素）。
        ///
        /// `None` = 背景没被改过，调用方可以直接用原始帧 `crop(region)`；
        /// `Some(img)` = 背景被内容填充改过，**必须**用这张图当底图。
        edited: Option<RgbaImage>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// 选区阶段
    Select,
    /// 编辑阶段
    Edit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Handle {
    Tl,
    T,
    Tr,
    L,
    R,
    Bl,
    B,
    Br,
}

enum Drag {
    /// 新建选区
    New { start: Point },
    /// 拖动整个选区
    Move { grab: Point, orig: Rect },
    /// 拖动控制点缩放
    Resize { handle: Handle, orig: Rect },
    /// 绘制标注
    Draw {
        tool: Tool,
        start: Point,
        points: Vec<Point>,
    },
    /// 「消除杂物」：框住要消除的物体
    Erase { start: Point },
}

/// 色值面板的状态（吸管取色后出现）。
#[derive(Clone, Copy, Debug)]
struct ColorPanelState {
    /// 取到的颜色。
    color: Rgba,
    /// 面板锚点（**背景帧内的物理像素**），面板画在它右下角。
    anchor: Point,
    /// 是否已经复制过色值（复制后文案变成「已复制色值」）。
    copied: bool,
}

/// 全屏分层浮窗。
pub struct Overlay {
    hwnd: HWND,
    state: Box<OverlayState>,
    target: LayeredTarget,
}

impl Overlay {
    /// 创建浮窗（此时还没有背景帧，`show()` 里再贴图）。
    pub fn new(cfg: OverlayConfig) -> Result<Overlay> {
        gdiplus::startup()?;
        win::ensure_dpi_aware();

        let mut state = Box::new(OverlayState::new(cfg));
        let state_ptr = &mut *state as *mut OverlayState;

        win::register_class(CLASS_NAME, Some(overlay_wnd_proc))?;
        let class = win::wide(CLASS_NAME);
        let title = win::wide("screenloom overlay");

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                cfg.virtual_origin.x,
                cfg.virtual_origin.y,
                16,
                16,
                None,
                None,
                Some(win::module_handle()),
                Some(state_ptr as *const core::ffi::c_void),
            )
        }
        .map_err(|e| screenloom_core::Error::Window(format!("CreateWindowExW 失败: {e}")))?;

        state.hwnd = hwnd;
        state.surface = (16, 16);
        let target = LayeredTarget::new(16, 16)?;

        Ok(Overlay {
            hwnd,
            state,
            target,
        })
    }

    /// 贴上抓到的屏幕图像并显示浮窗。
    pub fn show(&mut self, bg: &RgbaImage) -> Result<()> {
        self.state.bg = Some(bg.clone());
        self.state.surface = (bg.w as i32, bg.h as i32);
        self.state.relayout_toolbar();

        let o = self.state.cfg.virtual_origin;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                o.x,
                o.y,
                bg.w as i32,
                bg.h as i32,
                SWP_NOACTIVATE,
            );
        }
        self.target.resize(bg.w as i32, bg.h as i32)?;

        if self.state.cfg.hide_from_capture {
            unsafe {
                let _ = SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE);
            }
        }

        self.state.dirty = true;
        self.render()?;

        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }
        Ok(())
    }

    /// 事件循环：选区确认后返回 `Selected`，Esc 取消返回 `Cancelled`。
    pub fn run(&mut self) -> Result<OverlayOutcome> {
        self.state.dirty = true;
        self.render()?;
        unsafe {
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
        }

        let mut msg = MSG::default();
        loop {
            let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if r.0 <= 0 {
                break;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // 文字输入框的结果
            if let Some(res) = take_text_result() {
                self.state.apply_text_result(res);
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }

            if self.state.dirty {
                self.state.dirty = false;
                self.render()?;
            }
            if self.state.outcome.is_some() {
                break;
            }
        }

        // 抽干队列里残留的 WM_QUIT。
        //
        // `finish()` 用 `PostQuitMessage` 退出，但本循环是**先看到 outcome 才 break** 的，
        // 那条 WM_QUIT 还留在队列里。下一个消息循环（比如长截图的「按 Enter 拼接」等待）
        // 一 GetMessageW 就会立刻拿到它 → 直接返回，长截图功能形同虚设。
        OverlayState::drain_quit_message();
        Ok(self
            .state
            .outcome
            .take()
            .unwrap_or(OverlayOutcome::Cancelled))
    }

    /// 销毁浮窗（不跑事件循环时用）。
    pub fn destroy(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    /// 程序化设置选区（逻辑坐标）。
    pub fn set_selection(&mut self, r: Rect) {
        self.state.selection = Some(r);
        self.state.dirty = true;
    }

    /// 程序化设置选区（**图像内物理像素**，与 [`OverlayOutcome::Selected::region`] 同坐标系）。
    ///
    /// 供脚本化 / 自动化场景使用：不需要真实鼠标输入就能确定选区。
    pub fn set_selection_physical(&mut self, r: Rect) {
        let logical = self.state.i2l(r);
        self.state.selection = Some(logical);
        self.state.dirty = true;
    }

    /// 程序化确认选区（等价于点击工具栏「完成」）。
    pub fn confirm(&mut self) {
        self.state.finish(OverlayAction::Confirm);
    }

    /// 浮窗句柄（供外部驱动 / 设置窗口属性用）。
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// 注册 AI 处理器。
    ///
    /// 处理器在**UI 线程**被调用（收到「问 AI / 翻译 / AI 打码」时），
    /// 它应当**立刻返回**——真正的网络调用要放到后台线程，
    /// 完成后用 [`post_ai_reply`] 把结果送回浮窗。
    pub fn set_ai_handler(&mut self, f: impl Fn(AiRequest) + Send + Sync + 'static) {
        self.state.ai_handler = Some(Box::new(f));
    }

    /// 当前是否有 AI 处理器。
    pub fn has_ai_handler(&self) -> bool {
        self.state.ai_handler.is_some()
    }

    /// 注册「提取文字」（本地 OCR）处理器。
    ///
    /// 与 AI 处理器分开：**OCR 是本地的，不需要 API key**，
    /// 所以 app 层应当**无条件**装上它。
    ///
    /// 处理器在 **UI 线程**被调用，应当**立刻返回**——
    /// 真正的识别（写临时 PNG + WinRT 异步）放后台线程，
    /// 完成后用 [`post_ai_reply`] 把文本送回浮窗。
    pub fn set_ocr_handler(&mut self, f: impl Fn(RgbaImage) + Send + Sync + 'static) {
        self.state.ocr_handler = Some(Box::new(f));
    }

    /// 注册「打开设置」处理器。
    ///
    /// 工具栏上点「设置」时在 **UI 线程**调用它；它应当自己开设置窗口
    /// （设置窗口属于 app 层，不在 UI crate 里）。
    pub fn set_settings_handler(&mut self, f: impl Fn() + Send + Sync + 'static) {
        self.state.settings_handler = Some(Box::new(f));
    }

    /// 注册「识别二维码」处理器。
    ///
    /// 处理器在 **UI 线程**被调用（点工具栏「识别二维码」时），
    /// 它应当**立刻返回**——解码放到后台线程，
    /// 完成后用 [`post_ai_reply`] 把结果送回浮窗。
    ///
    /// 与 AI 不同：二维码解码是**本地**的，不需要 API key，所以 app 层应当**无条件**装上。
    pub fn set_qr_handler(&mut self, f: impl Fn(RgbaImage) + Send + Sync + 'static) {
        self.state.qr_handler = Some(Box::new(f));
    }

    /// 注册「复制文本到剪贴板」处理器。
    ///
    /// 浮窗自己不碰剪贴板（剪贴板属于 app 层），色值复制 / 二维码内容复制都走它。
    pub fn set_clipboard_handler(&mut self, f: impl Fn(String) + Send + Sync + 'static) {
        self.state.clipboard_handler = Some(Box::new(f));
    }

    /// 注册「窗口选择器」处理器。
    ///
    /// 工具栏上点「窗口选择」时在 **UI 线程**调用它；它应当自己弹窗口选择器
    /// （选择器属于 app 层，不在 UI crate 里），选完后用
    /// [`post_window_picked`] 把矩形投回浮窗。
    pub fn set_window_picker_handler(&mut self, f: impl Fn() + Send + Sync + 'static) {
        self.state.window_picker_handler = Some(Box::new(f));
    }

    /// 渲染当前一帧（覆盖整块虚拟屏幕，物理像素），用于预览 / 自检。
    pub fn render_frame(&self) -> Result<RgbaImage> {
        let (w, h) = self.state.surface;
        let bytes = self.state.render_pargb()?;
        Ok(pargb_to_rgba(&bytes, w as u32, h as u32))
    }

    /// 把当前帧提交给分层窗口。
    fn render(&mut self) -> Result<()> {
        let (w, h) = self.state.surface;
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let bytes = self.state.render_pargb()?;
        self.target.upload(&bytes);
        let o = self.state.cfg.virtual_origin;
        self.target.commit(self.hwnd, POINT { x: o.x, y: o.y })?;
        Ok(())
    }
}

struct OverlayState {
    cfg: OverlayConfig,
    hwnd: HWND,
    mode: Mode,
    bg: Option<RgbaImage>,
    /// 选区（逻辑坐标）
    selection: Option<Rect>,
    /// 标注（逻辑坐标，绝对位置）
    canvas: Canvas,
    toolbar: Toolbar,
    drag: Option<Drag>,
    outcome: Option<OverlayOutcome>,
    dirty: bool,
    /// 客户区尺寸（物理像素）
    surface: (i32, i32),
    text_anchor: Point,
    /// AI 处理器（app 层注册）
    ai_handler: Option<Box<dyn Fn(AiRequest) + Send + Sync>>,
    /// 「提取文字」（本地 OCR）处理器（app 层注册）—— **不需要 API key**
    ocr_handler: Option<Box<dyn Fn(RgbaImage) + Send + Sync>>,
    /// 「打开设置」处理器（app 层注册）
    settings_handler: Option<Box<dyn Fn() + Send + Sync>>,
    /// 「识别二维码」处理器（app 层注册）—— **本地解码，不需要 API key**
    qr_handler: Option<Box<dyn Fn(RgbaImage) + Send + Sync>>,
    /// 复制文本到剪贴板（app 层注册）—— 色值 / 二维码内容都走它
    clipboard_handler: Option<Box<dyn Fn(String) + Send + Sync>>,
    /// 「窗口选择器」处理器（app 层注册）
    window_picker_handler: Option<Box<dyn Fn() + Send + Sync>>,
    /// 最近一次 AI 结果
    ai_reply: Option<AiReply>,
    /// AI 请求是否在飞行中
    ai_pending: bool,
    /// 结果面板的标题（问 AI / 翻译 / 提取文字…）
    ai_title: String,
    /// 是否处于「消除杂物」模式（下一步拖拽 = 框选要消除的物体）
    erase_mode: bool,
    /// 背景是否被「消除杂物」改过
    bg_modified: bool,
    /// 色值面板
    color_panel: Option<ColorPanelState>,
}

// ---------------------------------------------------------------- 窗口过程

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut OverlayState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;

    match msg {
        WM_LBUTTONDOWN => {
            st.on_mouse_down(lp);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            st.on_mouse_up(lp);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            st.on_mouse_move(lp);
            return LRESULT(0);
        }
        WM_LBUTTONDBLCLK => {
            st.on_dbl_click(lp);
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            st.on_key_down(wp);
            return LRESULT(0);
        }
        WM_CHAR => {
            return LRESULT(0);
        }
        WM_AI_REPLY => {
            if lp.0 != 0 {
                // 接管堆上的回复（谁投递谁负责释放，这里 Box::from_raw 接管）
                let boxed = Box::from_raw(lp.0 as *mut AiReply);
                st.apply_ai_reply(*boxed);
            }
            return LRESULT(0);
        }
        WM_WINDOW_PICKED => {
            if lp.0 != 0 {
                let boxed = Box::from_raw(lp.0 as *mut Rect);
                st.apply_window_picked(*boxed);
            }
            return LRESULT(0);
        }
        WM_SETCURSOR => {
            if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(Some(c));
            }
            return LRESULT(1);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[cfg(test)]
mod tests;
