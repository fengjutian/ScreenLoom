//! 工具栏动作：按钮分发、吸管取色、复制色值。

use super::*;

impl OverlayState {
    pub(crate) fn on_toolbar(&mut self, kind: ItemKind) {
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
            ItemKind::QrCode => {
                if let Some(image) = self.selection_image() {
                    self.submit_qr(image);
                }
            }
            ItemKind::Erase => {
                // 进入「消除杂物」模式：下一次拖拽 = 框住要消除的物体
                self.erase_mode = true;
                self.ai_reply = None;
                self.ai_pending = false;
                self.ai_title = "消除杂物".to_string();
            }
            ItemKind::WindowPick => {
                if let Some(h) = self.window_picker_handler.as_ref() {
                    h();
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

    pub(crate) fn pick_color(&mut self, c: Point) {
        if let Some(bg) = &self.bg {
            if let Some(col) = sample_color(bg, c.x, c.y) {
                let col = Rgba { a: 255, ..col };
                self.canvas.color = col;
                // 色值面板：显示 HEX / RGB，可 Ctrl+C 复制
                self.color_panel = Some(ColorPanelState {
                    color: col,
                    anchor: c,
                    copied: false,
                });
            }
        }
        self.dirty = true;
    }

    /// 复制当前色值（HEX）到剪贴板。
    ///
    /// 剪贴板属于 app 层，所以这里只发请求。
    pub(crate) fn copy_color_value(&mut self) {
        let Some(cp) = self.color_panel.as_ref() else {
            return;
        };
        let hex = cp.color.to_hex();
        if let Some(h) = self.clipboard_handler.as_ref() {
            h(hex);
        }
        if let Some(cp) = self.color_panel.as_mut() {
            cp.copied = true;
        }
        self.dirty = true;
    }
}
