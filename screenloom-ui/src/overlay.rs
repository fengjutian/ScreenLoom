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
    sample_color, Annotation, Canvas, Point, Rect, Result, Rgba, RgbaImage, ShapeKind, Tool,
    BORDER, MASK_ALPHA, TEXT,
};

use crate::aipanel;
use crate::gdiplus::{self, Graphics};
use crate::render::{paint_annotations, pargb_to_rgba};
use crate::toolbar::{ItemKind, Mapper, Toolbar};
use crate::win::{self, LayeredTarget};

const CLASS_NAME: &str = "ScreenloomOverlayWindow";

/// AI 回复回投用的自定义窗口消息。
///
/// app 层在**后台线程**里调完模型后，用 [`post_ai_reply`] 把结果送到浮窗；
/// 浮窗的消息循环收到它再刷新界面 —— 这样 AI 调用**不会卡住 UI 线程**。
pub const WM_AI_REPLY: u32 = WM_APP + 1;

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
#[derive(Clone, Debug, PartialEq)]
pub enum AiReply {
    /// 文本结果（回答 / 译文）。
    Text(String),
    /// 出错信息。
    Error(String),
    /// 打码区域（相对选区左上角的像素）。
    MosaicRegions(Vec<MosaicRect>),
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
enum Handle {
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

// ---------------------------------------------------------------- 浮窗状态

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
    /// 最近一次 AI 结果
    ai_reply: Option<AiReply>,
    /// AI 请求是否在飞行中
    ai_pending: bool,
    /// 结果面板的标题（问 AI / 翻译 / 提取文字…）
    ai_title: String,
}

impl OverlayState {
    fn new(cfg: OverlayConfig) -> Self {
        OverlayState {
            cfg,
            hwnd: HWND::default(),
            mode: Mode::Select,
            bg: None,
            selection: None,
            canvas: Canvas::new(),
            toolbar: Toolbar::default(),
            drag: None,
            outcome: None,
            dirty: true,
            surface: (0, 0),
            text_anchor: Point::new(0, 0),
            ai_handler: None,
            ocr_handler: None,
            settings_handler: None,
            ai_reply: None,
            ai_pending: false,
            ai_title: "AI".to_string(),
        }
    }

    // ---------------- 坐标换算 ----------------

    /// 逻辑矩形 → 图像（物理像素）矩形。
    fn l2i(&self, r: Rect) -> Rect {
        let d = r.to_physical(self.cfg.virtual_origin, self.cfg.scale);
        Rect::new(
            d.x - self.cfg.virtual_origin.x,
            d.y - self.cfg.virtual_origin.y,
            d.w,
            d.h,
        )
    }

    /// 图像（物理像素）矩形 → 逻辑矩形。
    fn i2l(&self, r: Rect) -> Rect {
        let d = Rect::new(
            r.x + self.cfg.virtual_origin.x,
            r.y + self.cfg.virtual_origin.y,
            r.w,
            r.h,
        );
        d.to_logical(self.cfg.virtual_origin, self.cfg.scale)
    }

    fn l2i_point(&self, p: Point) -> Point {
        let r = self.l2i(Rect::new(p.x, p.y, 0, 0));
        Point::new(r.x, r.y)
    }

    fn i2l_point(&self, p: Point) -> Point {
        let r = self.i2l(Rect::new(p.x, p.y, 0, 0));
        Point::new(r.x, r.y)
    }

    /// 逻辑坐标下的虚拟屏幕矩形。
    fn logical_screen(&self) -> Rect {
        let (w, h) = self.surface;
        self.i2l(Rect::new(0, 0, w, h))
    }

    fn handle_px(&self) -> i32 {
        (8.0 * self.cfg.scale).round() as i32
    }

    fn mapper(&self) -> Mapper {
        Mapper {
            origin: self.cfg.virtual_origin,
            scale: self.cfg.scale,
            dx: 0,
            dy: 0,
        }
    }

    fn relayout_toolbar(&mut self) {
        let screen = self.logical_screen();
        self.toolbar.layout(screen);
        self.dump_toolbar_geometry();
    }

    /// 调试用：把工具栏每个按钮的**物理像素**矩形打到 stderr。
    ///
    /// 设 `SCREENLOOM_DEBUG_TOOLBAR=1` 才输出，正常运行零开销。
    /// 自动化测试靠它拿到按钮的真实坐标来点击，避免在脚本里复刻布局算法。
    fn dump_toolbar_geometry(&self) {
        if std::env::var_os("SCREENLOOM_DEBUG_TOOLBAR").is_none() {
            return;
        }
        let m = self.mapper();
        if let Some(sel) = self.selection {
            let r = m.rect(sel);
            eprintln!(
                "[selection] logical=({},{} {}x{}) phys=({},{} {}x{})",
                sel.x, sel.y, sel.w, sel.h, r.x, r.y, r.w, r.h
            );
        }
        for it in &self.toolbar.items {
            let r = m.rect(it.rect);
            eprintln!(
                "[toolbar] {:?} phys=({},{} {}x{})",
                it.kind, r.x, r.y, r.w, r.h
            );
        }
    }

    // ---------------- 绘制 ----------------

    /// 渲染一帧，返回 PARGB 字节（预乘 BGRA）。
    fn render_pargb(&self) -> Result<Vec<u8>> {
        let (w, h) = self.surface;
        if w <= 0 || h <= 0 {
            return Ok(Vec::new());
        }
        let base = self.base_pixels();
        let bmp = gdiplus::Bitmap::new(w, h)?;
        {
            let mut d = bmp.lock()?;
            let px = base.to_bgra_premultiplied();
            let n = px.len().min((d.Stride as usize) * h as usize);
            unsafe {
                ptr::copy_nonoverlapping(px.as_ptr(), d.Scan0 as *mut u8, n);
            }
            bmp.unlock(&mut d);
        }
        {
            let mut g = bmp.graphics()?;
            g.setup_quality();
            self.paint(&mut g);
        }
        let mut d = bmp.lock()?;
        let stride = d.Stride as usize;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let s = y * stride;
            let dst = y * w as usize * 4;
            let n = w as usize * 4;
            unsafe {
                ptr::copy_nonoverlapping(
                    (d.Scan0 as *const u8).add(s),
                    out.as_mut_ptr().add(dst),
                    n,
                );
            }
        }
        bmp.unlock(&mut d);
        Ok(out)
    }

    /// 本帧的背景像素。
    fn base_pixels(&self) -> RgbaImage {
        let (w, h) = self.surface;
        let mut out = RgbaImage::new(w.max(0) as u32, h.max(0) as u32);
        let Some(bg) = &self.bg else {
            return out;
        };
        match self.mode {
            Mode::Select => {
                let n = out.data.len().min(bg.data.len());
                out.data[..n].copy_from_slice(&bg.data[..n]);
            }
            Mode::Edit => {
                if let Some(sel) = self.selection {
                    let sp = self.l2i(sel);
                    copy_region(&mut out, bg, sp);
                    // 马赛克在像素层处理
                    for (r, block) in self.canvas.mosaic_regions() {
                        out.pixelate(self.l2i(r), block);
                    }
                }
            }
        }
        out
    }

    fn paint(&self, g: &mut Graphics) {
        match self.mode {
            Mode::Select => {
                let Some(sel) = self.selection else {
                    return;
                };
                let sp = self.l2i(sel);
                self.paint_dim(g, sp);
                g.draw_rect(
                    sp.x as f32,
                    sp.y as f32,
                    sp.w as f32,
                    sp.h as f32,
                    gdiplus::argb(BORDER),
                    1.0,
                );
                for (_, r) in handle_rects(sp, self.handle_px()) {
                    g.fill_rect(
                        r.x as f32,
                        r.y as f32,
                        r.w as f32,
                        r.h as f32,
                        gdiplus::argb(Rgba::WHITE),
                    );
                    g.draw_rect(
                        r.x as f32,
                        r.y as f32,
                        r.w as f32,
                        r.h as f32,
                        gdiplus::argb(BORDER),
                        1.0,
                    );
                }
                self.paint_size_label(g, sp);
            }
            Mode::Edit => {
                let Some(sel) = self.selection else {
                    return;
                };
                let sp = self.l2i(sel);
                let canvas = self.display_canvas();
                g.clip_rect(sp.x as f32, sp.y as f32, sp.w as f32, sp.h as f32);
                paint_annotations(g, &canvas, Point::new(sp.x, sp.y));
                g.clear_clip();
                g.draw_rect(
                    sp.x as f32,
                    sp.y as f32,
                    sp.w as f32,
                    sp.h as f32,
                    gdiplus::argb(BORDER),
                    1.0,
                );
                self.toolbar.paint(g, &self.canvas, &self.mapper());
                self.paint_ai_panel(g);
            }
        }
    }

    /// 选区外压暗。
    fn paint_dim(&self, g: &mut Graphics, sp: Rect) {
        let (w, h) = self.surface;
        let full = Rect::new(0, 0, w, h);
        let c = gdiplus::argb(Rgba::rgb(0, 0, 0).with_alpha(MASK_ALPHA));
        let top = sp.y - full.y;
        let bottom = full.bottom() - sp.bottom();
        let left = sp.x - full.x;
        let right = full.right() - sp.right();
        if top > 0 {
            g.fill_rect(full.x as f32, full.y as f32, full.w as f32, top as f32, c);
        }
        if bottom > 0 {
            g.fill_rect(
                full.x as f32,
                sp.bottom() as f32,
                full.w as f32,
                bottom as f32,
                c,
            );
        }
        if left > 0 {
            g.fill_rect(full.x as f32, sp.y as f32, left as f32, sp.h as f32, c);
        }
        if right > 0 {
            g.fill_rect(sp.right() as f32, sp.y as f32, right as f32, sp.h as f32, c);
        }
    }

    /// AI 结果面板（回答 / 译文 / 错误）。
    fn paint_ai_panel(&self, g: &mut Graphics) {
        if !self.ai_pending && self.ai_reply.is_none() {
            return;
        }
        let Some(sel) = self.selection else {
            return;
        };
        let scale = self.cfg.scale;
        let m = self.mapper();

        // 面板宽（逻辑）：不超过选区宽度，也不超过上限
        let w_l = (sel.w - 16).clamp(220, aipanel::MAX_WIDTH_LOGICAL);
        let x_l = sel.x + (sel.w - w_l) / 2;
        // 选区太矮就放到选区下面
        let y_l = if sel.h < 180 {
            sel.bottom() + 8
        } else {
            sel.y + 8
        };
        let pos = m.rect(Rect::new(x_l, y_l, w_l, 0));
        let pw = (w_l as f32 * scale) as i32;

        let (title, body, is_error) = match &self.ai_reply {
            Some(AiReply::Text(t)) => (self.ai_title.clone(), t.clone(), false),
            Some(AiReply::Error(e)) => (format!("{} 出错", self.ai_title), e.clone(), true),
            Some(AiReply::MosaicRegions(v)) => {
                let mut s = format!("AI 打码：识别到 {} 处敏感区域", v.len());
                for r in v.iter().take(8) {
                    s.push_str(&format!("\n· {}x{} @ ({},{})", r.w, r.h, r.x, r.y));
                }
                ("AI 打码".to_string(), s, false)
            }
            None => (self.ai_title.clone(), "正在处理…".to_string(), false),
        };
        aipanel::paint_panel(
            g,
            Rect::new(pos.x, pos.y, pw, 0),
            scale,
            &title,
            &body,
            is_error,
        );
    }

    /// 选区尺寸提示 `W x H`。
    fn paint_size_label(&self, g: &mut Graphics, sp: Rect) {
        let text = format!("{} x {}", sp.w, sp.h);
        let size = 13.0 * self.cfg.scale;
        let pad = 6.0 * self.cfg.scale;
        let (tw, th) = g.measure_text(&text, size, false);
        let w = tw + pad * 2.0;
        let h = th + pad * 2.0;
        let mut x = sp.right() as f32 - w;
        if x < 0.0 {
            x = 0.0;
        }
        let mut y = sp.y as f32 - h - 6.0 * self.cfg.scale;
        if y < 0.0 {
            y = sp.y as f32 + 6.0 * self.cfg.scale;
        }
        g.fill_round_rect(x, y, w, h, 4.0, 0xD9_00_00_00);
        g.draw_text(x + pad, y + pad, &text, size, gdiplus::argb(TEXT), false);
    }

    // ---------------- 结果 ----------------

    /// 选区内的标注（坐标相对裁剪后图像左上角，单位=**物理像素**）。
    ///
    /// 画布上的标注存在**逻辑坐标**里（鼠标事件给的），而输出坐标系是
    /// 「裁剪后图像的物理像素」。所以这里要走两步：
    /// 1. 逻辑 → 图像内物理像素（`to_physical`，即 × scale）；
    /// 2. 减去选区在图像内的物理原点（`l2i(sel)`）。
    ///
    /// ⚠️ 第 1 步不能省：本机主屏 scale = 1.25，省掉会让标注位置整体偏移 25%。
    fn display_canvas(&self) -> Canvas {
        let mut c = self.canvas.clone();
        let Some(sel) = self.selection else {
            return c;
        };
        let origin = self.l2i(sel);
        let d = Point::new(-origin.x, -origin.y);
        let s = self.cfg.scale;
        // 逻辑 → 物理（相对图像原点）
        let lp = |p: Point| {
            let r = Rect::new(p.x, p.y, 0, 0).to_physical(Point::new(0, 0), s);
            Point::new(r.x, r.y)
        };
        let lr = |r: Rect| {
            let p = r.to_physical(Point::new(0, 0), s);
            Rect::new(p.x, p.y, p.w, p.h)
        };
        for a in &mut c.items {
            match a {
                Annotation::Shape { rect, .. } => *rect = shift_rect(lr(*rect), d),
                Annotation::Arrow { from, to, .. } => {
                    *from = shift_point(lp(*from), d);
                    *to = shift_point(lp(*to), d);
                }
                Annotation::Pen { points, .. } => {
                    for p in points.iter_mut() {
                        *p = shift_point(lp(*p), d);
                    }
                }
                Annotation::Text { at, .. } => *at = shift_point(lp(*at), d),
                Annotation::Mosaic { rect, .. } => *rect = shift_rect(lr(*rect), d),
            }
        }
        c
    }

    fn build_outcome(&self, action: OverlayAction) -> Option<OverlayOutcome> {
        let sel = self.selection?;
        let region = self.l2i(sel);
        if region.is_empty() {
            return None;
        }
        Some(OverlayOutcome::Selected {
            region,
            canvas: self.display_canvas(),
            action,
        })
    }

    fn finish(&mut self, action: OverlayAction) {
        if let Some(out) = self.build_outcome(action) {
            self.outcome = Some(out);
            unsafe {
                PostQuitMessage(0);
            }
        }
    }

    fn cancel(&mut self) {
        self.outcome = Some(OverlayOutcome::Cancelled);
        unsafe {
            PostQuitMessage(0);
        }
    }

    /// 抽掉线程消息队列里残留的 `WM_QUIT`（只抽 WM_QUIT，别把别的消息一起吞掉）。
    fn drain_quit_message() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {}
        }
    }

    fn enter_edit(&mut self) {
        if self.selection.is_none() {
            return;
        }
        self.mode = Mode::Edit;
        self.relayout_toolbar();
        self.dirty = true;
    }

    // ---------------- 交互 ----------------

    fn on_mouse_down(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        let p = self.i2l_point(c);
        match self.mode {
            Mode::Select => {
                if let Some(sel) = self.selection {
                    let sp = self.l2i(sel);
                    for (h, r) in handle_rects(sp, self.handle_px()) {
                        if r.contains(c) {
                            self.drag = Some(Drag::Resize {
                                handle: h,
                                orig: sel,
                            });
                            self.dirty = true;
                            return;
                        }
                    }
                    if sel.contains(p) {
                        self.drag = Some(Drag::Move { grab: p, orig: sel });
                        self.dirty = true;
                        return;
                    }
                }
                self.selection = None;
                self.drag = Some(Drag::New { start: p });
            }
            Mode::Edit => {
                if let Some(kind) = self.toolbar.hit(p) {
                    self.on_toolbar(kind);
                    return;
                }
                let tool = self.canvas.tool;
                match tool {
                    Tool::Text => {
                        self.open_text_input(p);
                    }
                    Tool::ColorPicker => {
                        self.pick_color(c);
                    }
                    _ => {
                        self.drag = Some(Drag::Draw {
                            tool,
                            start: p,
                            points: vec![p],
                        });
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn on_mouse_move(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        let p = self.i2l_point(c);
        match self.drag.as_mut() {
            Some(Drag::New { start }) => {
                let r = Rect::new(start.x, start.y, p.x - start.x, p.y - start.y).normalized();
                self.selection = Some(r);
            }
            Some(Drag::Move { grab, orig }) => {
                let dx = p.x - grab.x;
                let dy = p.y - grab.y;
                self.selection = Some(Rect::new(orig.x + dx, orig.y + dy, orig.w, orig.h));
            }
            Some(Drag::Resize { handle, orig }) => {
                self.selection = Some(resize_rect(*orig, *handle, p));
            }
            Some(Drag::Draw { tool, points, .. }) if *tool == Tool::Pen => {
                if let Some(last) = points.last() {
                    if *last != p {
                        points.push(p);
                    }
                }
            }
            // 其它工具（矩形/箭头/马赛克/文字/取色）：按下时不用额外处理
            Some(Drag::Draw { .. }) => {}
            None => {}
        }
        self.dirty = true;
    }

    fn on_mouse_up(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        let p = self.i2l_point(c);
        if let Some(d) = self.drag.take() {
            match d {
                Drag::New { .. } => {
                    if let Some(s) = self.selection {
                        if s.w < 3 || s.h < 3 {
                            self.selection = None;
                        }
                    }
                }
                Drag::Draw {
                    tool,
                    start,
                    mut points,
                } => {
                    let color = self.canvas.color;
                    let width = self.canvas.width;
                    match tool {
                        Tool::Rect | Tool::Ellipse => {
                            let r = Rect::new(start.x, start.y, p.x - start.x, p.y - start.y)
                                .normalized();
                            if r.w > 2 && r.h > 2 {
                                let kind = if tool == Tool::Rect {
                                    ShapeKind::Rect
                                } else {
                                    ShapeKind::Ellipse
                                };
                                self.canvas.push(Annotation::Shape {
                                    kind,
                                    rect: r,
                                    color,
                                    width,
                                });
                            }
                        }
                        Tool::Arrow => {
                            if (p.x - start.x).abs() + (p.y - start.y).abs() > 3 {
                                self.canvas.push(Annotation::Arrow {
                                    from: start,
                                    to: p,
                                    color,
                                    width,
                                });
                            }
                        }
                        Tool::Pen => {
                            if let Some(last) = points.last() {
                                if *last != p {
                                    points.push(p);
                                }
                            }
                            if points.len() > 1 {
                                self.canvas.push(Annotation::Pen {
                                    points,
                                    color,
                                    width,
                                });
                            }
                        }
                        Tool::Mosaic => {
                            let r = Rect::new(start.x, start.y, p.x - start.x, p.y - start.y)
                                .normalized();
                            if r.w > 2 && r.h > 2 {
                                self.canvas.push(Annotation::Mosaic {
                                    rect: r,
                                    block: self.canvas.mosaic_block,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        self.dirty = true;
    }

    fn on_dbl_click(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        let p = self.i2l_point(c);
        match self.mode {
            Mode::Select => {
                if let Some(sel) = self.selection {
                    if sel.contains(p) {
                        self.enter_edit();
                    }
                }
            }
            Mode::Edit => {
                self.finish(OverlayAction::Confirm);
            }
        }
    }

    fn on_key_down(&mut self, wp: WPARAM) {
        let vk = (wp.0 as u32) & 0xFF;
        let ctrl = unsafe { GetKeyState(0x11) } < 0; // VK_CONTROL
        match vk {
            0x1B => {
                // ESC
                if self.mode == Mode::Edit {
                    self.mode = Mode::Select;
                    self.dirty = true;
                } else {
                    self.cancel();
                }
            }
            0x0D => {
                // ENTER
                if self.mode == Mode::Select {
                    if self.selection.is_some() {
                        self.enter_edit();
                    }
                } else {
                    self.finish(OverlayAction::Confirm);
                }
            }
            0x5A if ctrl => {
                // Ctrl+Z
                self.canvas.undo();
                self.dirty = true;
            }
            0x59 if ctrl => {
                // Ctrl+Y
                self.canvas.redo();
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_toolbar(&mut self, kind: ItemKind) {
        match kind {
            ItemKind::Tool(t) => self.canvas.tool = t,
            ItemKind::Color(c) => self.canvas.color = c,
            ItemKind::WidthInfo | ItemKind::FontInfo => {}
            ItemKind::WidthDown => self.canvas.width = (self.canvas.width - 1.0).max(1.0),
            ItemKind::WidthUp => self.canvas.width = (self.canvas.width + 1.0).min(40.0),
            ItemKind::FontDown => self.canvas.font_size = (self.canvas.font_size - 2.0).max(8.0),
            ItemKind::FontUp => self.canvas.font_size = (self.canvas.font_size + 2.0).min(200.0),
            ItemKind::Undo => self.canvas.undo(),
            ItemKind::Redo => self.canvas.redo(),
            ItemKind::AskAi => {
                self.open_ai_input();
            }
            ItemKind::Translate => {
                if let Some(image) = self.selection_image() {
                    self.submit_ai(AiRequest::Translate { image });
                }
            }
            ItemKind::AiMosaic => {
                if let Some(image) = self.selection_image() {
                    self.submit_ai(AiRequest::Mosaic { image });
                }
            }
            ItemKind::ExtractText => {
                if let Some(image) = self.selection_image() {
                    self.submit_ocr(image);
                }
            }
            ItemKind::LongShot => {
                self.finish(OverlayAction::LongShot);
                return;
            }
            ItemKind::Settings => {
                if let Some(h) = self.settings_handler.as_ref() {
                    h();
                }
            }
            ItemKind::Complete => {
                self.finish(OverlayAction::Confirm);
                return;
            }
            ItemKind::Copy => {
                self.finish(OverlayAction::Copy);
                return;
            }
            ItemKind::Save => {
                self.finish(OverlayAction::Save);
                return;
            }
            ItemKind::Pin => {
                self.finish(OverlayAction::Pin);
                return;
            }
            ItemKind::Exit => {
                self.cancel();
                return;
            }
            ItemKind::Toggle => {
                self.toolbar.collapsed = !self.toolbar.collapsed;
                self.relayout_toolbar();
            }
        }
        self.dirty = true;
    }

    fn pick_color(&mut self, c: Point) {
        if let Some(bg) = &self.bg {
            if let Some(col) = sample_color(bg, c.x, c.y) {
                self.canvas.color = Rgba { a: 255, ..col };
            }
        }
        self.dirty = true;
    }

    // ---------------- 文字输入 ----------------

    /// 选区内的图像（裁剪后，物理像素）。
    fn selection_image(&self) -> Option<RgbaImage> {
        let sel = self.selection?;
        let region = self.l2i(sel);
        if region.is_empty() {
            return None;
        }
        let bg = self.bg.as_ref()?;
        Some(bg.crop(region))
    }

    fn open_text_input(&mut self, at: Point) {
        self.text_anchor = at;
        let p = self.l2i_point(at);
        let x = self.cfg.virtual_origin.x + p.x;
        let y = self.cfg.virtual_origin.y + p.y;
        let h = ((self.canvas.font_size * 2.0) as i32 + 12).max(30);
        let bg = self.canvas.color;
        let fg = contrast_color(bg);
        self.spawn_edit(x, y, 280, h, bg, fg, TextPurpose::Annotation);
        self.dirty = true;
    }

    /// 「问 AI」：弹一个输入框收集用户的问题。
    fn open_ai_input(&mut self) {
        let Some(sel) = self.selection else {
            return;
        };
        let scale = self.cfg.scale;
        let w_l = 420.min(sel.w.max(240));
        let x_l = sel.x + (sel.w - w_l) / 2;
        // 优先放在选区上方；顶到屏幕边缘就放到选区内部靠上
        let y_l = if sel.y > 56 { sel.y - 48 } else { sel.y + 8 };
        let p = self.mapper().rect(Rect::new(x_l, y_l, w_l, 0));
        let x = self.cfg.virtual_origin.x + p.x;
        let y = self.cfg.virtual_origin.y + p.y;
        let w = (w_l as f32 * scale) as i32;
        let h = (36.0 * scale) as i32;
        self.spawn_edit(
            x,
            y,
            w,
            h,
            Rgba::rgb(0x1E, 0x1E, 0x1E),
            Rgba::WHITE,
            TextPurpose::AiPrompt,
        );
        self.dirty = true;
    }

    /// 创建一个浮在浮窗之上的 EDIT 输入框。
    #[allow(clippy::too_many_arguments)]
    fn spawn_edit(&self, x: i32, y: i32, w: i32, h: i32, bg: Rgba, fg: Rgba, purpose: TextPurpose) {
        let class = win::wide("EDIT");
        let title = win::wide("");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WINDOW_STYLE(WS_POPUP.0 | WS_BORDER.0 | 0x0080), // ES_AUTOHSCROLL
                x,
                y,
                w,
                h,
                None,
                None,
                Some(win::module_handle()),
                None,
            )
        };
        let Ok(hwnd) = hwnd else {
            return;
        };

        let proc: TextProc = text_wnd_proc;
        let old = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as usize as isize) } as usize;
        let brush = unsafe { CreateSolidBrush(COLORREF(bg.to_colorref())) };
        let fg = COLORREF(fg.to_colorref());
        TEXT_PROCS.with(|m| {
            m.borrow_mut().insert(
                hwnd.0 as isize,
                TextInfo {
                    old: unsafe { std::mem::transmute::<usize, TextProc>(old) },
                    brush,
                    fg,
                    purpose,
                },
            );
        });

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
        }
    }

    /// 发起一次 AI 请求：交给 app 层注册的处理器（它应当立刻返回，结果异步回投）。
    fn submit_ai(&mut self, req: AiRequest) {
        self.ai_title = match &req {
            AiRequest::Ask { .. } => "问 AI".to_string(),
            AiRequest::Translate { .. } => "翻译".to_string(),
            AiRequest::Mosaic { .. } => "AI 打码".to_string(),
        };
        self.ai_reply = None;
        self.ai_pending = true;
        self.dirty = true;
        if let Some(h) = self.ai_handler.as_ref() {
            h(req);
        } else {
            self.ai_reply = Some(AiReply::Error("未配置 AI 服务（缺少 API key？）".into()));
            self.ai_pending = false;
        }
    }

    /// 发起一次本地 OCR（提取文字）：**不需要 API key**。
    ///
    /// 结果用同一套回投机制（[`post_ai_reply`]）送回，标题为「提取文字」。
    fn submit_ocr(&mut self, image: RgbaImage) {
        self.ai_title = "提取文字".to_string();
        self.ai_reply = None;
        self.ai_pending = true;
        self.dirty = true;
        if let Some(h) = self.ocr_handler.as_ref() {
            h(image);
        } else {
            self.ai_reply = Some(AiReply::Error(
                "本机没有可用的 OCR（未安装语言包？）".into(),
            ));
            self.ai_pending = false;
        }
    }

    /// 应用一次 AI 结果。
    fn apply_ai_reply(&mut self, reply: AiReply) {
        self.ai_pending = false;
        // 打码结果：直接变成画布上的马赛克标注（会立刻在像素层生效）
        if let AiReply::MosaicRegions(regions) = &reply {
            if let Some(sel) = self.selection {
                let origin = self.l2i(sel);
                let block = self.canvas.mosaic_block;
                for r in regions {
                    let img_rect = Rect::new(origin.x + r.x, origin.y + r.y, r.w, r.h);
                    let logical = self.i2l(img_rect);
                    if logical.w > 1 && logical.h > 1 {
                        self.canvas.push(Annotation::Mosaic {
                            rect: logical,
                            block,
                        });
                    }
                }
            }
        }
        self.ai_reply = Some(reply);
        self.dirty = true;
    }

    fn apply_text_result(&mut self, res: TextResult) {
        match res {
            TextResult::Commit(text) => {
                let text = text.trim_end_matches(['\r', '\n']).to_string();
                if !text.is_empty() {
                    let color = self.canvas.color;
                    let size = self.canvas.font_size;
                    self.canvas.push(Annotation::Text {
                        at: self.text_anchor,
                        text,
                        color,
                        size,
                    });
                }
            }
            TextResult::Cancel => {}
            TextResult::AiCommit(text) => {
                let text = text.trim().to_string();
                if !text.is_empty() {
                    if let Some(image) = self.selection_image() {
                        self.submit_ai(AiRequest::Ask {
                            question: text,
                            image,
                        });
                    }
                }
            }
            TextResult::AiCancel => {}
        }
        self.dirty = true;
    }
}

// ---------------------------------------------------------------- 文字输入

type TextProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 输入框的用途（决定 ENTER / ESC 的语义）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum TextPurpose {
    /// 标注文字
    Annotation,
    /// 「问 AI」的提问
    AiPrompt,
}

#[derive(Clone, Copy)]
struct TextInfo {
    old: TextProc,
    brush: windows::Win32::Graphics::Gdi::HBRUSH,
    fg: COLORREF,
    purpose: TextPurpose,
}

enum TextResult {
    Commit(String),
    Cancel,
    /// 「问 AI」的提问内容
    AiCommit(String),
    AiCancel,
}

thread_local! {
    static TEXT_RESULT: RefCell<Option<TextResult>> = const { RefCell::new(None) };
    static TEXT_PROCS: RefCell<HashMap<isize, TextInfo>> = RefCell::new(HashMap::new());
}

fn set_text_result(r: Option<TextResult>) {
    TEXT_RESULT.with(|c| {
        *c.borrow_mut() = r;
    });
}

fn take_text_result() -> Option<TextResult> {
    TEXT_RESULT.with(|c| c.borrow_mut().take())
}

unsafe extern "system" fn text_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = (wp.0 as u32) & 0xFF;
            let purpose =
                TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).map(|i| i.purpose));
            if vk == 0x0D {
                // ENTER → 提交
                let mut buf = [0u16; 1024];
                let n = GetWindowTextW(hwnd, &mut buf).clamp(0, buf.len() as i32 - 1) as usize;
                let text = String::from_utf16_lossy(&buf[..n]);
                let result = match purpose {
                    Some(TextPurpose::AiPrompt) => TextResult::AiCommit(text),
                    _ => TextResult::Commit(text),
                };
                set_text_result(Some(result));
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            } else if vk == 0x1B {
                // ESC → 取消
                let result = match purpose {
                    Some(TextPurpose::AiPrompt) => TextResult::AiCancel,
                    _ => TextResult::Cancel,
                };
                set_text_result(Some(result));
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            if (wp.0 as u32) == 0x0D {
                return LRESULT(0); // 吃掉回车
            }
        }
        WM_CTLCOLOREDIT => {
            let info = TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
            if let Some(info) = info {
                let hdc = HDC(wp.0 as *mut core::ffi::c_void);
                unsafe {
                    SetTextColor(hdc, info.fg);
                }
                return LRESULT(info.brush.0 as isize);
            }
        }
        WM_DESTROY => {
            let info = TEXT_PROCS.with(|m| m.borrow_mut().remove(&(hwnd.0 as isize)));
            if let Some(info) = info {
                unsafe {
                    let _ = DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(info.brush.0));
                }
            }
        }
        _ => {}
    }

    let info = TEXT_PROCS.with(|m| m.borrow().get(&(hwnd.0 as isize)).copied());
    match info {
        Some(info) => unsafe { CallWindowProcW(Some(info.old), hwnd, msg, wp, lp) },
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
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

// ---------------------------------------------------------------- 工具函数

/// 把 `src` 的 `rect` 区域拷到 `dst` 的同位置。
fn copy_region(dst: &mut RgbaImage, src: &RgbaImage, rect: Rect) {
    let r = rect.normalized();
    let x0 = r.x.max(0) as u32;
    let y0 = r.y.max(0) as u32;
    let x1 = r.right().min(src.w as i32).min(dst.w as i32) as u32;
    let y1 = r.bottom().min(src.h as i32).min(dst.h as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for y in y0..y1 {
        let s = ((y * src.w + x0) * 4) as usize;
        let d = ((y * dst.w + x0) * 4) as usize;
        let n = ((x1 - x0) * 4) as usize;
        dst.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
    }
}

#[inline]
fn shift_rect(r: Rect, d: Point) -> Rect {
    Rect::new(r.x + d.x, r.y + d.y, r.w, r.h)
}

#[inline]
fn shift_point(p: Point, d: Point) -> Point {
    Point::new(p.x + d.x, p.y + d.y)
}

/// 8 个控制点的位置（物理像素）。
fn handle_rects(sel: Rect, size: i32) -> Vec<(Handle, Rect)> {
    let half = size / 2;
    let l = sel.x;
    let t = sel.y;
    let r = sel.right() - 1;
    let b = sel.bottom() - 1;
    let cx = sel.x + sel.w / 2;
    let cy = sel.y + sel.h / 2;
    let mk = |x: i32, y: i32| Rect::new(x - half, y - half, size, size);
    vec![
        (Handle::Tl, mk(l, t)),
        (Handle::T, mk(cx, t)),
        (Handle::Tr, mk(r, t)),
        (Handle::L, mk(l, cy)),
        (Handle::R, mk(r, cy)),
        (Handle::Bl, mk(l, b)),
        (Handle::B, mk(cx, b)),
        (Handle::Br, mk(r, b)),
    ]
}

/// 按控制点缩放选区。
fn resize_rect(orig: Rect, h: Handle, p: Point) -> Rect {
    let mut l = orig.x;
    let mut t = orig.y;
    let mut r = orig.right();
    let mut b = orig.bottom();
    match h {
        Handle::Tl => {
            l = p.x;
            t = p.y;
        }
        Handle::T => t = p.y,
        Handle::Tr => {
            r = p.x;
            t = p.y;
        }
        Handle::L => l = p.x,
        Handle::R => r = p.x,
        Handle::Bl => {
            l = p.x;
            b = p.y;
        }
        Handle::B => b = p.y,
        Handle::Br => {
            r = p.x;
            b = p.y;
        }
    }
    Rect::new(l, t, r - l, b - t).normalized()
}

/// 文字颜色取黑或白（取决于背景亮度）。
fn contrast_color(c: Rgba) -> Rgba {
    let lum = 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
    if lum > 140.0 {
        Rgba::rgb(0, 0, 0)
    } else {
        Rgba::WHITE
    }
}

// ---------------------------------------------------------------- 自测

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::RECT;

    /// 带明显颜色梯度的测试图（便于校验裁剪位置）。
    fn test_bg(w: u32, h: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, Rgba::rgb(x as u8, y as u8, 0x40));
            }
        }
        img
    }

    /// 把 (x, y) 打包成 lParam（低位 x，高位 y）。
    fn lparam(x: i32, y: i32) -> LPARAM {
        LPARAM(((((y as u32) & 0xffff) << 16) | ((x as u32) & 0xffff)) as isize)
    }

    fn send(hwnd: HWND, msg: u32, x: i32, y: i32) {
        unsafe {
            SendMessageW(hwnd, msg, Some(WPARAM(1)), Some(lparam(x, y)));
        }
    }

    /// 拖拽：从 (x0,y0) 按下，移动到 (x1,y1)，松开。
    fn drag(ov: &Overlay, x0: i32, y0: i32, x1: i32, y1: i32) {
        send(ov.hwnd, WM_LBUTTONDOWN, x0, y0);
        send(ov.hwnd, WM_MOUSEMOVE, x1, y1);
        send(ov.hwnd, WM_LBUTTONUP, x1, y1);
    }

    fn key(ov: &Overlay, vk: u32) {
        unsafe {
            SendMessageW(
                ov.hwnd,
                WM_KEYDOWN,
                Some(WPARAM(vk as usize)),
                Some(LPARAM(0)),
            );
        }
    }

    fn click(ov: &Overlay, x: i32, y: i32) {
        send(ov.hwnd, WM_LBUTTONDOWN, x, y);
        send(ov.hwnd, WM_LBUTTONUP, x, y);
    }

    /// 工具栏上某个按钮的中心（客户区像素）。
    fn toolbar_point(ov: &Overlay, kind: ItemKind) -> Point {
        let it = ov
            .state
            .toolbar
            .items
            .iter()
            .find(|i| i.kind == kind)
            .expect("工具栏按钮不存在");
        let center = Point::new(it.rect.x + it.rect.w / 2, it.rect.y + it.rect.h / 2);
        ov.state.l2i_point(center)
    }

    fn make_overlay(origin: Point) -> Overlay {
        let mut ov = Overlay::new(OverlayConfig {
            virtual_origin: origin,
            scale: 1.0,
            hide_from_capture: false,
        })
        .expect("Overlay::new 失败");
        ov.show(&test_bg(800, 600)).expect("show 失败");
        ov
    }

    /// 泵一遍消息队列（模拟浮窗的消息循环收到一条消息）。
    fn pump() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// 鼠标（客户区 = 屏幕 − 窗口原点 = 图像内）坐标 → 图像内物理像素坐标，
    /// 并且 `bg.crop(region)` 能裁出正确的区域。
    #[test]
    fn mouse_coords_map_to_image_pixels_and_crop_correctly() {
        let origin = Point::new(137, 59); // 非零虚拟屏原点
        let bg = test_bg(800, 600);
        let mut ov = Overlay::new(OverlayConfig {
            virtual_origin: origin,
            scale: 1.0,
            hide_from_capture: false,
        })
        .unwrap();
        ov.show(&bg).expect("show 失败");

        // 浮窗确实放在虚拟屏原点上
        let mut wr = RECT::default();
        unsafe { GetWindowRect(ov.hwnd, &mut wr).unwrap() };
        assert_eq!((wr.left, wr.top), (origin.x, origin.y));

        // 拖出一个选区（客户区坐标 = 图像内像素）
        drag(&ov, 210, 160, 410, 330);

        let region = ov.state.l2i(ov.state.selection.expect("没有选区"));
        assert_eq!(
            region,
            Rect::new(210, 160, 200, 170),
            "region 必须是图像内物理像素"
        );

        // 屏幕坐标 → 图像内坐标：图像内 = 屏幕 − 捕获帧左上角
        let screen = Point::new(wr.left + 210, wr.top + 160);
        assert_eq!(
            Point::new(screen.x - origin.x, screen.y - origin.y),
            Point::new(210, 160)
        );

        // crop 出来的必须是原图对应区域
        let cropped = bg.crop(region);
        assert_eq!((cropped.w, cropped.h), (200, 170));
        assert_eq!(cropped.pixel(0, 0), bg.pixel(210, 160));
        assert_eq!(cropped.pixel(199, 169), bg.pixel(409, 329));

        ov.destroy();
    }

    /// 四个 OverlayAction 都能从工具栏真实触发并正确返回。
    #[test]
    fn toolbar_actions_return_expected_action() {
        for (kind, expect) in [
            (ItemKind::Complete, OverlayAction::Confirm),
            (ItemKind::Copy, OverlayAction::Copy),
            (ItemKind::Save, OverlayAction::Save),
            (ItemKind::Pin, OverlayAction::Pin),
        ] {
            let mut ov = make_overlay(Point::new(0, 0));
            drag(&ov, 210, 160, 410, 330);
            key(&ov, 0x0D); // Enter → 编辑态（工具栏出现）
            let p = toolbar_point(&ov, kind);
            click(&ov, p.x, p.y);
            match ov.run().unwrap() {
                OverlayOutcome::Selected { region, action, .. } => {
                    assert_eq!(action, expect, "动作不匹配");
                    assert_eq!(region, Rect::new(210, 160, 200, 170));
                }
                OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
            }
            ov.destroy();
        }
    }

    /// Esc → Cancelled。
    #[test]
    fn escape_cancels() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x1B);
        assert!(matches!(ov.run().unwrap(), OverlayOutcome::Cancelled));
        ov.destroy();
    }

    /// 画布里的标注坐标必须是 **裁剪后图像内的像素**（相对选区左上角）。
    #[test]
    fn canvas_annotations_are_region_relative_pixels() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330); // 选区左上角 = (210,160)
        key(&ov, 0x0D); // 进入编辑态
        ov.state.canvas.tool = Tool::Rect;
        drag(&ov, 250, 200, 350, 260); // 在选区内画一个矩形

        let p = toolbar_point(&ov, ItemKind::Complete);
        click(&ov, p.x, p.y);

        match ov.run().unwrap() {
            OverlayOutcome::Selected { region, canvas, .. } => {
                assert_eq!(region, Rect::new(210, 160, 200, 170));
                let shape = canvas
                    .items
                    .iter()
                    .find_map(|a| match a {
                        Annotation::Shape { rect, .. } => Some(*rect),
                        _ => None,
                    })
                    .expect("没有矩形标注");
                // 屏幕 (250,200)-(350,260) → 相对选区左上角 (210,160) = (40,40)-(140,100)
                assert_eq!(shape, Rect::new(40, 40, 100, 60));
            }
            OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
        }
        ov.destroy();
    }

    // ---------------- AI 接入 ----------------

    /// 缩放 != 1 时，标注坐标仍必须落在「裁剪后图像的物理像素」上。
    ///
    /// 回归测试：曾经 `display_canvas` 漏乘 `scale`，本机主屏 1.25 缩放下
    /// 所有标注整体偏移 25%。
    #[test]
    fn canvas_annotations_are_physical_pixels_under_scaled_dpi() {
        let mut ov = Overlay::new(OverlayConfig {
            virtual_origin: Point::new(-1038, -1440),
            scale: 1.25,
            hide_from_capture: false,
        })
        .unwrap();
        ov.show(&test_bg(2000, 1600)).expect("show 失败");

        drag(&ov, 250, 200, 350, 260); // 选区（客户区像素）
        key(&ov, 0x0D); // 进入编辑态
        ov.state.canvas.tool = Tool::Rect;
        drag(&ov, 300, 250, 400, 310); // 画矩形

        let canvas = ov.state.display_canvas();
        let shape = canvas
            .items
            .iter()
            .find_map(|a| match a {
                Annotation::Shape { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("没有矩形标注");
        // 选区物理 (250,200)-(350,260)；矩形物理 (300,250)-(400,310)
        // → 相对选区左上角 = (50,50)-(150,110)
        assert_eq!(shape, Rect::new(50, 50, 100, 60));
        ov.destroy();
    }

    /// 工具栏上有「问 AI / 翻译 / AI 打码」三个按钮；
    /// 没装 AI 处理器时点击会给出明确报错（而不是静默无反应）。
    #[test]
    fn ai_toolbar_buttons_exist_and_report_missing_handler() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D); // 进入编辑态（工具栏出现）

        for kind in [ItemKind::AskAi, ItemKind::Translate, ItemKind::AiMosaic] {
            let p = toolbar_point(&ov, kind);
            assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");
        }

        let p = toolbar_point(&ov, ItemKind::Translate);
        click(&ov, p.x, p.y);
        assert!(
            matches!(ov.state.ai_reply, Some(AiReply::Error(_))),
            "没装 handler 时应给出错误提示"
        );
        assert!(!ov.state.ai_pending);
        ov.destroy();
    }

    /// 工具栏上的「提取文字」按钮：装了 OCR 处理器时把**选区图像**交给它，
    /// 没装时给出明确报错（而不是静默无反应）。
    #[test]
    fn extract_text_button_exists_and_reports_missing_handler() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D); // 进入编辑态（工具栏出现）

        let p = toolbar_point(&ov, ItemKind::ExtractText);
        assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");

        click(&ov, p.x, p.y);
        assert_eq!(ov.state.ai_title, "提取文字", "标题应是「提取文字」");
        assert!(
            matches!(ov.state.ai_reply, Some(AiReply::Error(_))),
            "没装 handler 时应给出错误提示"
        );
        assert!(!ov.state.ai_pending);
        ov.destroy();
    }

    /// 「提取文字」把**选区裁剪后的图像**交给 OCR 处理器，
    /// 回投的文本会显示在结果面板里。
    #[test]
    fn extract_text_button_passes_selection_image_to_handler() {
        use std::sync::{Arc, Mutex};

        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330); // 选区 (210,160) 200x170
        key(&ov, 0x0D);

        let seen: Arc<Mutex<Option<RgbaImage>>> = Arc::new(Mutex::new(None));
        let sink = seen.clone();
        ov.set_ocr_handler(move |img| {
            *sink.lock().unwrap() = Some(img);
        });

        let p = toolbar_point(&ov, ItemKind::ExtractText);
        click(&ov, p.x, p.y);

        let img = seen.lock().unwrap().take().expect("OCR 处理器没被调用");
        assert_eq!(
            (img.w, img.h),
            (200, 170),
            "交给 OCR 的必须是选区裁剪后的图像"
        );
        // 没回投结果时面板处于 pending
        assert!(ov.state.ai_pending, "等待回投时应是 pending");

        // 模拟后台线程回投
        post_ai_reply(ov.hwnd(), AiReply::Text("识别出来的文字".into()));
        pump();
        assert_eq!(
            ov.state.ai_reply,
            Some(AiReply::Text("识别出来的文字".into()))
        );
        assert!(!ov.state.ai_pending);
        ov.destroy();
    }

    /// 工具栏上的「滚动截图」按钮：点击后以 `OverlayAction::LongShot` 收尾，
    /// 选区原样带回给 app 层（app 层拿它去跑滚动拼接）。
    #[test]
    fn long_shot_button_returns_long_shot_action() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D);

        let p = toolbar_point(&ov, ItemKind::LongShot);
        assert!(p.x >= 0 && p.y >= 0, "按钮位置异常: {p:?}");
        click(&ov, p.x, p.y);

        match ov.run().unwrap() {
            OverlayOutcome::Selected { region, action, .. } => {
                assert_eq!(action, OverlayAction::LongShot, "动作不匹配");
                assert_eq!(region, Rect::new(210, 160, 200, 170));
            }
            OverlayOutcome::Cancelled => panic!("不该是 Cancelled"),
        }
        ov.destroy();
    }

    /// 浮窗退出后**不能**给下一个消息循环留一个 `WM_QUIT`。
    ///
    /// 坑：`finish()` 投 `WM_QUIT`，但事件循环是**先看到 outcome 才 break** 的，
    /// 那条 `WM_QUIT` 留在队列里 → 紧接着的长截图按键循环一 `GetMessageW` 就退出，
    /// 长截图功能形同虚设（只截到第一帧）。
    #[test]
    fn overlay_exit_leaves_no_pending_quit_message() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D);
        let p = toolbar_point(&ov, ItemKind::LongShot);
        click(&ov, p.x, p.y);
        let out = ov.run().unwrap();
        assert!(
            matches!(out, OverlayOutcome::Selected { .. }),
            "应该正常收尾而不是 Cancelled"
        );

        let mut msg = MSG::default();
        let pending =
            unsafe { PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_NOREMOVE).as_bool() };
        assert!(!pending, "WM_QUIT 残留会让下一个消息循环立刻退出");
        ov.destroy();
    }

    /// 后台线程回投的 AI 结果能被浮窗收到并应用。
    #[test]
    fn ai_reply_posted_from_thread_is_applied() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330);
        key(&ov, 0x0D);

        // 模拟后台线程：post_ai_reply 把结果塞进消息队列
        post_ai_reply(ov.hwnd(), AiReply::Text("这是回答".into()));
        pump();

        assert_eq!(ov.state.ai_reply, Some(AiReply::Text("这是回答".into())));
        assert!(!ov.state.ai_pending);
        ov.destroy();
    }

    /// AI 打码的结果会变成画布上的马赛克标注（坐标相对裁剪后图像左上角）。
    #[test]
    fn ai_mosaic_reply_creates_canvas_mosaic_annotations() {
        let mut ov = make_overlay(Point::new(0, 0));
        drag(&ov, 210, 160, 410, 330); // 选区 (210,160)
        key(&ov, 0x0D);

        post_ai_reply(
            ov.hwnd(),
            AiReply::MosaicRegions(vec![MosaicRect {
                x: 10,
                y: 20,
                w: 30,
                h: 40,
            }]),
        );
        pump();

        let canvas = ov.state.display_canvas();
        let regions = canvas.mosaic_regions();
        assert_eq!(regions.len(), 1, "应该有一个马赛克区域");
        // 模型给的是「相对裁剪图左上角」的偏移
        assert_eq!(regions[0].0, Rect::new(10, 20, 30, 40));
        ov.destroy();
    }
}
