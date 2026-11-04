//! 画布：标注集合 + 撤销/重做 + 当前工具状态。
//!
//! 对齐微信 `wx::WxShotEditormenu` 的 `canUndo` / `push` / `graphChanged` 语义。

use crate::annotation::{Annotation, Tool};
use crate::geometry::Rect;
use crate::image::Rgba;

/// 画布状态。
#[derive(Clone, Debug)]
pub struct Canvas {
    /// 标注列表（按绘制顺序）
    pub items: Vec<Annotation>,
    /// 撤销栈：每项是一次操作前的 items 快照
    pub undo: Vec<Vec<Annotation>>,
    /// 重做栈
    pub redo: Vec<Vec<Annotation>>,
    /// 当前工具
    pub tool: Tool,
    /// 当前颜色
    pub color: Rgba,
    /// 当前笔画宽度
    pub width: f32,
    /// 当前字号
    pub font_size: f32,
    /// 马赛克块大小
    pub mosaic_block: u32,
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new()
    }
}

impl Canvas {
    pub fn new() -> Self {
        Canvas {
            items: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            tool: Tool::Rect,
            color: Rgba::RED,
            width: 3.0,
            font_size: 18.0,
            mosaic_block: 12,
        }
    }

    /// 追加一条标注，自动记录撤销快照。
    pub fn push(&mut self, a: Annotation) {
        self.undo.push(self.items.clone());
        self.redo.clear();
        self.items.push(a);
    }

    /// 撤销一步。
    pub fn undo(&mut self) {
        if let Some(prev) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.items, prev));
        }
    }

    /// 重做一步。
    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.items, next));
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.push(self.items.clone());
        self.redo.clear();
        self.items.clear();
    }

    /// 移除完全落在裁剪框外的标注。
    pub fn crop_to(&mut self, r: Rect) {
        self.undo.push(self.items.clone());
        self.redo.clear();
        self.items.retain(|a| a.intersects(r));
    }

    /// 收集所有马赛克区域（渲染前统一处理像素）。
    pub fn mosaic_regions(&self) -> Vec<(Rect, u32)> {
        self.items
            .iter()
            .filter_map(|a| match a {
                Annotation::Mosaic { rect, block } => Some((*rect, *block)),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::ShapeKind;

    fn shape() -> Annotation {
        Annotation::Shape {
            kind: ShapeKind::Rect,
            rect: Rect::new(0, 0, 10, 10),
            color: Rgba::RED,
            width: 2.0,
        }
    }

    #[test]
    fn push_then_undo_then_redo() {
        let mut c = Canvas::new();
        assert!(!c.can_undo());
        c.push(shape());
        assert_eq!(c.items.len(), 1);
        assert!(c.can_undo());
        c.undo();
        assert_eq!(c.items.len(), 0);
        assert!(c.can_redo());
        c.redo();
        assert_eq!(c.items.len(), 1);
    }

    #[test]
    fn new_push_clears_redo() {
        let mut c = Canvas::new();
        c.push(shape());
        c.undo();
        assert!(c.can_redo());
        c.push(shape());
        assert!(!c.can_redo());
    }

    #[test]
    fn crop_drops_unrelated_items() {
        let mut c = Canvas::new();
        c.push(Annotation::Shape {
            kind: ShapeKind::Rect,
            rect: Rect::new(100, 100, 10, 10),
            color: Rgba::RED,
            width: 1.0,
        });
        c.crop_to(Rect::new(0, 0, 50, 50));
        assert!(c.items.is_empty());
    }
}
