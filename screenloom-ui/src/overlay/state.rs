//! 浮窗状态的生命周期：构造、成交图、结束/取消、消息循环收尾。

use super::*;

impl OverlayState {
    pub(crate) fn new(cfg: OverlayConfig) -> Self {
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
            qr_handler: None,
            clipboard_handler: None,
            window_picker_handler: None,
            ai_reply: None,
            ai_pending: false,
            ai_title: "AI".to_string(),
            erase_mode: false,
            bg_modified: false,
            color_panel: None,
        }
    }

    pub(crate) fn build_outcome(&self, action: OverlayAction) -> Option<OverlayOutcome> {
        let sel = self.selection?;
        let region = self.l2i(sel);
        if region.is_empty() {
            return None;
        }
        Some(OverlayOutcome::Selected {
            region,
            canvas: self.display_canvas(),
            action,
            edited: self.edited_pixels(),
        })
    }

    /// 「消除杂物」改过的背景（选区裁剪后，物理像素）。
    ///
    /// 背景没被改过时返回 `None` —— 调用方可以直接用原始帧 `crop(region)`。
    pub(crate) fn edited_pixels(&self) -> Option<RgbaImage> {
        if !self.bg_modified {
            return None;
        }
        let sel = self.selection?;
        let region = self.l2i(sel);
        if region.is_empty() {
            return None;
        }
        let bg = self.bg.as_ref()?;
        Some(bg.crop(region))
    }

    pub(crate) fn finish(&mut self, action: OverlayAction) {
        if let Some(out) = self.build_outcome(action) {
            self.outcome = Some(out);
            unsafe {
                PostQuitMessage(0);
            }
        }
    }

    pub(crate) fn cancel(&mut self) {
        self.outcome = Some(OverlayOutcome::Cancelled);
        unsafe {
            PostQuitMessage(0);
        }
    }

    /// 抽掉线程消息队列里残留的 `WM_QUIT`（只抽 WM_QUIT，别把别的消息一起吞掉）。
    pub(crate) fn drain_quit_message() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {}
        }
    }

    pub(crate) fn enter_edit(&mut self) {
        if self.selection.is_none() {
            return;
        }
        self.mode = Mode::Edit;
        self.relayout_toolbar();
        self.dirty = true;
    }
}
