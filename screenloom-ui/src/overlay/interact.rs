//! 鼠标 / 键盘交互：选区拖拽、控制点缩放、标注绘制、消除杂物。

use super::*;

impl OverlayState {
    // ---------------- 交互 ----------------

    pub(crate) fn on_mouse_down(&mut self, lp: LPARAM) {
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
                if self.erase_mode {
                    // 「消除杂物」：框选要消除的物体
                    self.drag = Some(Drag::Erase { start: p });
                    self.dirty = true;
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

    pub(crate) fn on_mouse_move(&mut self, lp: LPARAM) {
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
            // 「消除杂物」：拖动过程中不预览，松手时一次性框选（见 `on_mouse_up`）
            Some(Drag::Erase { .. }) => {}
            None => {}
        }
        self.dirty = true;
    }

    pub(crate) fn on_mouse_up(&mut self, lp: LPARAM) {
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
                Drag::Erase { start } => {
                    let r = Rect::new(start.x, start.y, p.x - start.x, p.y - start.y).normalized();
                    self.apply_erase(r);
                }
                _ => {}
            }
        }
        self.dirty = true;
    }

    /// 「消除杂物」：把框选的区域用周围内容填掉。
    ///
    /// 走的是**本地多尺度样例填充**（`screenloom_core::inpaint`），
    /// 不需要 API key。填充直接改背景帧，所以最终成图要用
    /// [`OverlayOutcome::Selected::edited`] 那份底图。
    pub(crate) fn apply_erase(&mut self, logical_rect: Rect) {
        self.erase_mode = false;
        let Some(sel) = self.selection else {
            return;
        };
        if logical_rect.w < 4 || logical_rect.h < 4 {
            self.ai_reply = Some(AiReply::Error("框选区域太小".into()));
            self.ai_pending = false;
            self.ai_title = "消除杂物".to_string();
            return;
        }
        // 背景帧内的物理像素矩形，并与选区求交（选区外的部分没意义）
        let region = self
            .l2i(logical_rect)
            .intersect(self.l2i(sel))
            .unwrap_or(Rect::new(0, 0, 0, 0));
        if region.is_empty() {
            self.ai_reply = Some(AiReply::Error("框选区域不在选区内".into()));
            self.ai_pending = false;
            self.ai_title = "消除杂物".to_string();
            return;
        }
        // 超大区域会让本地填充跑很久，直接拒绝
        const MAX_ERASE: i32 = 1200;
        if region.w > MAX_ERASE || region.h > MAX_ERASE {
            self.ai_reply = Some(AiReply::Error(format!(
                "区域太大（{}x{}），请框小于 {MAX_ERASE}x{MAX_ERASE}",
                region.w, region.h
            )));
            self.ai_pending = false;
            self.ai_title = "消除杂物".to_string();
            return;
        }
        let Some(bg) = self.bg.as_ref() else {
            return;
        };
        let filled = inpaint_region(bg, region);
        if let Some(b) = self.bg.as_mut() {
            *b = filled;
        }
        self.bg_modified = true;
        self.ai_reply = Some(AiReply::Text(format!(
            "已消除 {}x{} 的内容",
            region.w, region.h
        )));
        self.ai_pending = false;
        self.ai_title = "消除杂物".to_string();
    }

    pub(crate) fn on_dbl_click(&mut self, lp: LPARAM) {
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

    pub(crate) fn on_key_down(&mut self, wp: WPARAM) {
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
            0x43 if ctrl => {
                // Ctrl+C —— 复制色值（对齐微信「按 Ctrl+C 复制色值」）
                self.copy_color_value();
            }
            _ => {}
        }
    }
}
