//! AI / OCR / 二维码：请求投递与回复落地（后台线程回投见 `mod.rs`）。

use super::*;

impl OverlayState {
    // ---------------- 文字输入 ----------------

    /// 选区内的图像（裁剪后，物理像素）。
    pub(crate) fn selection_image(&self) -> Option<RgbaImage> {
        let sel = self.selection?;
        let region = self.l2i(sel);
        if region.is_empty() {
            return None;
        }
        let bg = self.bg.as_ref()?;
        Some(bg.crop(region))
    }

    /// 「问 AI」：弹一个输入框收集用户的问题。
    pub(crate) fn open_ai_input(&mut self) {
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

    /// 发起一次 AI 请求：交给 app 层注册的处理器（它应当立刻返回，结果异步回投）。
    pub(crate) fn submit_ai(&mut self, req: AiRequest) {
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
    pub(crate) fn submit_ocr(&mut self, image: RgbaImage) {
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

    /// 发起一次二维码识别：**本地解码，不需要 API key**。
    ///
    /// 结果用同一套回投机制（[`post_ai_reply`]）送回，标题为「二维码」。
    pub(crate) fn submit_qr(&mut self, image: RgbaImage) {
        self.ai_title = "二维码".to_string();
        self.ai_reply = None;
        self.ai_pending = true;
        self.dirty = true;
        if let Some(h) = self.qr_handler.as_ref() {
            h(image);
        } else {
            self.ai_reply = Some(AiReply::Error("本机没有可用的二维码解码器".into()));
            self.ai_pending = false;
        }
    }

    /// 应用一次 AI 结果。
    pub(crate) fn apply_ai_reply(&mut self, reply: AiReply) {
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

    /// 应用「窗口选择器」的结果：把选区切到用户选中的那个窗口。
    ///
    /// `region` 是**背景帧内**的物理像素矩形。
    pub(crate) fn apply_window_picked(&mut self, region: Rect) {
        if region.is_empty() {
            return;
        }
        let logical = self.i2l(region);
        if logical.w < 2 || logical.h < 2 {
            return;
        }
        self.selection = Some(logical);
        self.erase_mode = false;
        self.mode = Mode::Edit;
        self.relayout_toolbar();
        self.dirty = true;
    }
}
