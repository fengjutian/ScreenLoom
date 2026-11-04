//! 基础几何类型。
//!
//! 约定：坐标一律为 **i32 像素**。逻辑坐标（窗口/浮窗使用）与物理坐标（图像像素）
//! 的换算只能通过 [`Rect::to_physical`]，避免混合 DPI 下散落各处的手写乘法。

/// 二维点。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn offset(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }
}

/// 尺寸。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

impl Size {
    pub const fn new(w: i32, h: i32) -> Self {
        Self { w, h }
    }
}

/// 矩形。允许携带负宽高（拖拽过程中的临时状态），使用前先 [`Rect::normalized`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(self) -> i32 {
        self.y + self.h
    }

    /// 纠正负宽高，输出左上角 + 正尺寸。
    pub fn normalized(self) -> Rect {
        let (x, w) = if self.w < 0 {
            (self.x + self.w, -self.w)
        } else {
            (self.x, self.w)
        };
        let (y, h) = if self.h < 0 {
            (self.y + self.h, -self.h)
        } else {
            (self.y, self.h)
        };
        Rect { x, y, w, h }
    }

    pub fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersect(self, other: Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let r = self.right().min(other.right());
        let b = self.bottom().min(other.bottom());
        if r > x && b > y {
            Some(Rect::new(x, y, r - x, b - y))
        } else {
            None
        }
    }

    pub fn center(self) -> Point {
        Point::new(self.x + self.w / 2, self.y + self.h / 2)
    }

    /// 逻辑坐标 → 物理像素。`origin` 为虚拟屏幕逻辑原点，`scale` 为缩放比。
    pub fn to_physical(self, origin: Point, scale: f32) -> Rect {
        let s = if scale > 0.0 { scale } else { 1.0 };
        Rect {
            x: origin.x + (self.x as f32 * s).round() as i32,
            y: origin.y + (self.y as f32 * s).round() as i32,
            w: (self.w as f32 * s).round() as i32,
            h: (self.h as f32 * s).round() as i32,
        }
    }

    /// 物理像素 → 逻辑坐标（`to_physical` 的逆运算）。
    pub fn to_logical(self, origin: Point, scale: f32) -> Rect {
        let s = if scale > 0.0 { scale } else { 1.0 };
        Rect {
            x: (((self.x - origin.x) as f32) / s).round() as i32,
            y: (((self.y - origin.y) as f32) / s).round() as i32,
            w: (self.w as f32 / s).round() as i32,
            h: (self.h as f32 / s).round() as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_fixes_negative_size() {
        let r = Rect::new(100, 80, -40, -20).normalized();
        assert_eq!(r, Rect::new(60, 60, 40, 20));
        assert!(!r.is_empty());
    }

    #[test]
    fn intersect_returns_none_when_disjoint() {
        assert!(Rect::new(0, 0, 10, 10)
            .intersect(Rect::new(20, 20, 5, 5))
            .is_none());
        let i = Rect::new(0, 0, 10, 10)
            .intersect(Rect::new(5, 5, 10, 10))
            .unwrap();
        assert_eq!(i, Rect::new(5, 5, 5, 5));
    }

    #[test]
    fn physical_roundtrip_keeps_pixels() {
        let origin = Point::new(-1920, 0);
        let logical = Rect::new(100, 50, 200, 100);
        let physical = logical.to_physical(origin, 1.5);
        assert_eq!(physical, Rect::new(-1920 + 150, 75, 300, 150));
        let back = physical.to_logical(origin, 1.5);
        assert_eq!(back, logical);
    }
}
