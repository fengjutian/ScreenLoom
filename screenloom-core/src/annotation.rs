//! 标注数据模型（对齐微信 `canvas::graphics_t` 的概念）。
//!
//! 微信侧对应的证据：反射成员表中的 `canvas::graphics_t`、`penChanged`、`fontChanged`、
//! `fillChanged`、`canUndo`、`paintGraph`、`graph`、`graphChanged`。

use crate::geometry::{Point, Rect};
use crate::image::Rgba;

/// 当前激活的工具。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Rect,
    Ellipse,
    Arrow,
    Pen,
    Text,
    Mosaic,
    ColorPicker,
    Crop,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Arrow => "arrow",
            Tool::Pen => "pen",
            Tool::Text => "text",
            Tool::Mosaic => "mosaic",
            Tool::ColorPicker => "color-picker",
            Tool::Crop => "crop",
        }
    }
}

/// 图形种类（矩形 / 椭圆共用一套包围盒语义）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Rect,
    Ellipse,
}

/// 一条标注。
#[derive(Clone, Debug)]
pub enum Annotation {
    Shape {
        kind: ShapeKind,
        rect: Rect,
        color: Rgba,
        width: f32,
    },
    Arrow {
        from: Point,
        to: Point,
        color: Rgba,
        width: f32,
    },
    Pen {
        points: Vec<Point>,
        color: Rgba,
        width: f32,
    },
    Text {
        at: Point,
        text: String,
        color: Rgba,
        size: f32,
    },
    Mosaic {
        rect: Rect,
        block: u32,
    },
}

impl Annotation {
    /// 该标注覆盖的区域（用于命中测试与裁剪）。
    pub fn bounds(&self) -> Rect {
        match self {
            Annotation::Shape { rect, .. } => rect.normalized(),
            Annotation::Arrow { from, to, .. } => Rect::new(
                from.x.min(to.x),
                from.y.min(to.y),
                (to.x - from.x).abs(),
                (to.y - from.y).abs(),
            ),
            Annotation::Pen { points, .. } => {
                if points.is_empty() {
                    return Rect::new(0, 0, 0, 0);
                }
                let mut r = Rect::new(points[0].x, points[0].y, 0, 0);
                for p in points {
                    let x0 = r.x.min(p.x);
                    let y0 = r.y.min(p.y);
                    let x1 = r.right().max(p.x);
                    let y1 = r.bottom().max(p.y);
                    r = Rect::new(x0, y0, x1 - x0, y1 - y0);
                }
                r
            }
            Annotation::Text { at, text, size, .. } => {
                // 中文按全角估宽，英文按半角
                let w = text
                    .chars()
                    .map(|c| {
                        if c as u32 > 0x2E80 {
                            *size
                        } else {
                            *size * 0.55
                        }
                    })
                    .sum::<f32>();
                Rect::new(at.x, at.y - *size as i32, w as i32, *size as i32 * 2)
            }
            Annotation::Mosaic { rect, .. } => rect.normalized(),
        }
    }

    /// 该标注是否与矩形相交（裁剪用）。
    pub fn intersects(&self, r: Rect) -> bool {
        self.bounds().intersect(r).is_some()
    }
}
