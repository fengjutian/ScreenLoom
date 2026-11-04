//! 自绘：压暗遮罩、选区边框、控制点、AI 面板、色值面板、尺寸标签。

use super::*;

impl OverlayState {
    // ---------------- 绘制 ----------------

    /// 渲染一帧，返回 PARGB 字节（预乘 BGRA）。
    pub(crate) fn render_pargb(&self) -> Result<Vec<u8>> {
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
    pub(crate) fn base_pixels(&self) -> RgbaImage {
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

    pub(crate) fn paint(&self, g: &mut Graphics) {
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
                self.paint_color_panel(g);
                self.paint_erase_hint(g);
            }
        }
    }

    /// 选区外压暗。
    pub(crate) fn paint_dim(&self, g: &mut Graphics, sp: Rect) {
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
    pub(crate) fn paint_ai_panel(&self, g: &mut Graphics) {
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
            Some(AiReply::QrCodes(v)) => {
                if v.is_empty() {
                    (
                        "二维码".to_string(),
                        "选区里没有识别到二维码".to_string(),
                        false,
                    )
                } else {
                    let mut s = format!("识别到 {} 个二维码", v.len());
                    for t in v.iter().take(8) {
                        s.push_str(&format!("\n· {t}"));
                    }
                    ("二维码".to_string(), s, false)
                }
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

    /// 色值面板（吸管取色后出现）。
    pub(crate) fn paint_color_panel(&self, g: &mut Graphics) {
        let Some(cp) = self.color_panel.as_ref() else {
            return;
        };
        let (sw, sh) = self.surface;
        let scale = self.cfg.scale;
        let w = (208.0 * scale).round() as i32;
        let h = (86.0 * scale).round() as i32;
        // 锚点 = 取色位置；面板画在它右下角，超出屏幕就往里挪
        let mut x = cp.anchor.x + (14.0 * scale).round() as i32;
        let mut y = cp.anchor.y + (18.0 * scale).round() as i32;
        if x + w > sw {
            x = (sw - w).max(0);
        }
        if y + h > sh {
            y = (sh - h).max(0);
        }
        colorpanel::paint_color_panel(g, Rect::new(x, y, w, h), scale, cp.color, cp.copied);
    }

    /// 「消除杂物」模式提示。
    pub(crate) fn paint_erase_hint(&self, g: &mut Graphics) {
        if !self.erase_mode {
            return;
        }
        let Some(sel) = self.selection else {
            return;
        };
        let scale = self.cfg.scale;
        let text = "拖动框住要消除的物体";
        let size = 13.0 * scale;
        let (tw, th) = g.measure_text(text, size, false);
        let pad = 8.0 * scale;
        let w = (tw + pad * 2.0) as i32;
        let h = (th + pad * 2.0) as i32;
        let sp = self.l2i(sel);
        let x = sp.x + ((sp.w - w) / 2).max(0);
        let y = (sp.y - h - (8.0 * scale) as i32).max(0);
        g.fill_round_rect(x as f32, y as f32, w as f32, h as f32, 6.0, 0xE0_00_C3_75);
        g.draw_text(
            x as f32 + pad,
            y as f32 + pad,
            text,
            size,
            gdiplus::argb(Rgba::rgb(0x10, 0x10, 0x10)),
            false,
        );
    }

    /// 选区尺寸提示 `W x H`。
    pub(crate) fn paint_size_label(&self, g: &mut Graphics, sp: Rect) {
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
    pub(crate) fn display_canvas(&self) -> Canvas {
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
}
