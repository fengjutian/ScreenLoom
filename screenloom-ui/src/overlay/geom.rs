//! 坐标换算（逻辑 ↔ 图像 ↔ 屏幕）与几何工具：控制点、缩放、区域搬运。

use super::*;

impl OverlayState {
    // ---------------- 坐标换算 ----------------

    /// 逻辑矩形 → 图像（物理像素）矩形。
    pub(crate) fn l2i(&self, r: Rect) -> Rect {
        let d = r.to_physical(self.cfg.virtual_origin, self.cfg.scale);
        Rect::new(
            d.x - self.cfg.virtual_origin.x,
            d.y - self.cfg.virtual_origin.y,
            d.w,
            d.h,
        )
    }

    /// 图像（物理像素）矩形 → 逻辑矩形。
    pub(crate) fn i2l(&self, r: Rect) -> Rect {
        let d = Rect::new(
            r.x + self.cfg.virtual_origin.x,
            r.y + self.cfg.virtual_origin.y,
            r.w,
            r.h,
        );
        d.to_logical(self.cfg.virtual_origin, self.cfg.scale)
    }

    pub(crate) fn l2i_point(&self, p: Point) -> Point {
        let r = self.l2i(Rect::new(p.x, p.y, 0, 0));
        Point::new(r.x, r.y)
    }

    pub(crate) fn i2l_point(&self, p: Point) -> Point {
        let r = self.i2l(Rect::new(p.x, p.y, 0, 0));
        Point::new(r.x, r.y)
    }

    /// 逻辑坐标下的虚拟屏幕矩形。
    pub(crate) fn logical_screen(&self) -> Rect {
        let (w, h) = self.surface;
        self.i2l(Rect::new(0, 0, w, h))
    }

    pub(crate) fn handle_px(&self) -> i32 {
        (8.0 * self.cfg.scale).round() as i32
    }

    pub(crate) fn mapper(&self) -> Mapper {
        Mapper {
            origin: self.cfg.virtual_origin,
            scale: self.cfg.scale,
            dx: 0,
            dy: 0,
        }
    }

    pub(crate) fn relayout_toolbar(&mut self) {
        let screen = self.logical_screen();
        self.toolbar.layout(screen);
        self.dump_toolbar_geometry();
    }

    /// 调试用：把工具栏每个按钮的**物理像素**矩形打到 stderr。
    ///
    /// 设 `SCREENLOOM_DEBUG_TOOLBAR=1` 才输出，正常运行零开销。
    /// 自动化测试靠它拿到按钮的真实坐标来点击，避免在脚本里复刻布局算法。
    pub(crate) fn dump_toolbar_geometry(&self) {
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
}

// ---------------------------------------------------------------- 工具函数

/// 把 `src` 的 `rect` 区域拷到 `dst` 的同位置。
pub(crate) fn copy_region(dst: &mut RgbaImage, src: &RgbaImage, rect: Rect) {
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
pub(crate) fn shift_rect(r: Rect, d: Point) -> Rect {
    Rect::new(r.x + d.x, r.y + d.y, r.w, r.h)
}

#[inline]
pub(crate) fn shift_point(p: Point, d: Point) -> Point {
    Point::new(p.x + d.x, p.y + d.y)
}

/// 8 个控制点的位置（物理像素）。
pub(crate) fn handle_rects(sel: Rect, size: i32) -> Vec<(Handle, Rect)> {
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
pub(crate) fn resize_rect(orig: Rect, h: Handle, p: Point) -> Rect {
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
pub(crate) fn contrast_color(c: Rgba) -> Rgba {
    let lum = 0.299 * c.r as f32 + 0.587 * c.g as f32 + 0.114 * c.b as f32;
    if lum > 140.0 {
        Rgba::rgb(0, 0, 0)
    } else {
        Rgba::WHITE
    }
}
