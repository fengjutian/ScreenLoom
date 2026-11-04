//! 滚动视口：偏移夹取、滚轮 / 键盘 / 拖动映射、滑块几何。
//!
//! 只做数学，不碰 Win32 —— 所有方法都能单测。

use super::*;

/// 滚动条最小滑块高度（逻辑像素）。
const MIN_THUMB_H: i32 = 24;
/// 滚轮一格滚几行。
const WHEEL_ROWS: i32 = 3;

/// 视口滚动状态。
pub(crate) struct Scroll {
    /// 当前偏移（内容坐标，0..=max）。
    pub(crate) y: i32,
    /// 最大偏移（内容高度 - 视口高度，夹到 >= 0）。
    pub(crate) max: i32,
    /// 视口高度（物理像素）。
    pub(crate) viewport_h: i32,
    /// 鼠标是否悬停在滑块上。
    pub(crate) hover: bool,
    /// 拖动中：鼠标 y - 滑块顶边。
    pub(crate) drag: Option<i32>,
    /// 滚轮余数（高精度触控板的小 delta 累加）。
    acc: i32,
}

impl Default for Scroll {
    fn default() -> Self {
        Self {
            y: 0,
            max: 0,
            viewport_h: 0,
            hover: false,
            drag: None,
            acc: 0,
        }
    }
}

impl Scroll {
    /// 设置内容范围（窗口创建 / 主题变化后调用）。
    pub(crate) fn set_extent(&mut self, max: i32, viewport_h: i32) {
        self.max = max.max(0);
        self.viewport_h = viewport_h.max(0);
        self.clamp();
    }

    /// 夹到 `0..=max`。
    pub(crate) fn clamp(&mut self) {
        self.y = self.y.clamp(0, self.max);
    }

    /// 是否真的需要滚动条。
    pub(crate) fn overflows(&self) -> bool {
        self.max > 0
    }

    pub(crate) fn scroll_to(&mut self, y: i32) -> bool {
        let old = self.y;
        // 返回“请求是否改变了目标位置”，而不是“夹取后是否真的动了”：
        // 在顶端继续向上滚时仍要吃掉这次滚轮事件并让宿主重绘。
        let changed = y != old;
        self.y = y;
        self.clamp();
        changed
    }

    /// 滚轮。`delta` 是 `WM_MOUSEWHEEL` 的 120 倍数值，正值 = 向上。
    /// 返回是否真的移动了。
    pub(crate) fn wheel(&mut self, delta: i32, k: f32) -> bool {
        self.acc += delta;
        let steps = self.acc / 120;
        self.acc -= steps * 120;
        if steps == 0 {
            return false;
        }
        let step = px(ROW_H * WHEEL_ROWS, k);
        self.scroll_to(self.y - steps * step)
    }

    /// 翻页。`dir` = +1 向下 / -1 向上。
    pub(crate) fn page(&mut self, dir: i32) -> bool {
        let step = (self.viewport_h * 4 / 5).max(1);
        self.scroll_to(self.y + dir * step)
    }

    pub(crate) fn home(&mut self) -> bool {
        self.scroll_to(0)
    }

    pub(crate) fn end(&mut self) -> bool {
        self.scroll_to(self.max)
    }

    /// 让内容坐标里的矩形完整出现在视口里（最小移动量，留边距）。
    pub(crate) fn ensure_visible(&mut self, r: Rect, k: f32) -> bool {
        let margin = px(8, k);
        let top = r.y - margin;
        let bottom = r.bottom() + margin;
        if top < self.y {
            return self.scroll_to(top);
        }
        if bottom > self.y + self.viewport_h {
            return self.scroll_to(bottom - self.viewport_h);
        }
        false
    }

    /// 滑块矩形。内容不溢出时返回 `None`。
    pub(crate) fn thumb(&self, track: Rect, k: f32) -> Option<Rect> {
        if !self.overflows() || track.h <= 0 {
            return None;
        }
        let total = self.viewport_h + self.max;
        if total <= 0 {
            return None;
        }
        let min_h = px(MIN_THUMB_H, k);
        let th = (track.h * self.viewport_h / total).max(min_h).min(track.h);
        let travel = (track.h - th).max(1);
        let ty = track.y + travel * self.y / self.max.max(1);
        Some(Rect::new(track.x, ty, track.w, th))
    }

    /// 开始拖滑块（`y` 为客户端坐标）。返回是否命中滑块。
    pub(crate) fn begin_drag(&mut self, track: Rect, y: i32, k: f32) -> bool {
        let Some(t) = self.thumb(track, k) else {
            return false;
        };
        if y >= t.y && y < t.bottom() {
            self.drag = Some(y - t.y);
            true
        } else {
            false
        }
    }

    /// 拖动到客户端坐标 `y`。
    pub(crate) fn drag_to(&mut self, track: Rect, y: i32, k: f32) -> bool {
        let Some(off) = self.drag else {
            return false;
        };
        let Some(t) = self.thumb(track, k) else {
            return false;
        };
        let travel = (track.h - t.h).max(1);
        let ty = (y - off).clamp(track.y, track.y + travel);
        let target = (ty - track.y) * self.max / travel;
        self.scroll_to(target)
    }

    /// 点击轨道（非滑块）：上/下翻一页。
    pub(crate) fn click_track(&mut self, track: Rect, y: i32, k: f32) -> bool {
        let Some(t) = self.thumb(track, k) else {
            return false;
        };
        if y < t.y {
            self.page(-1)
        } else {
            self.page(1)
        }
    }
}
