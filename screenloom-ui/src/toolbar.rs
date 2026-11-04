//! 屏幕底部工具栏：工具切换 / 调色板 / 线宽 / 字号 / 撤销 / 完成 / 复制 / 保存 / 置顶 / 退出。
//!
//! 工具栏的几何全部用 **逻辑坐标** 表达，绘制时再换算成物理像素，
//! 这样在不同 DPI ���幕上按钮大小一致。

use windows::Win32::Graphics::GdiPlus::PointF;

use screenloom_core::{Canvas, Rect, Rgba, Tool, PALETTE};

use crate::gdiplus::{self, Graphics};

/// 工具栏逻辑尺寸常量。
const PANEL_H: i32 = 40;
const PAD: i32 = 8;
const BTN: i32 = 28;
const STEP_BTN: i32 = 24;
const SWATCH: i32 = 18;
const SWATCH_GAP: i32 = 4;
const SEP: i32 = 10;
const TOGGLE_W: i32 = 26;
const FONT_UI: f32 = 12.0;

/// 工具栏上的一个可点元素。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemKind {
    /// 切换绘制工具
    Tool(Tool),
    /// 选择颜色
    Color(Rgba),
    /// 线宽显示区（不可点）
    WidthInfo,
    /// 线宽 -
    WidthDown,
    /// 线宽 +
    WidthUp,
    /// 字号显示区（不可点）
    FontInfo,
    /// 字号 -
    FontDown,
    /// 字号 +
    FontUp,
    Undo,
    Redo,
    /// 问 AI（图片 + 用户输入的问题）
    AskAi,
    /// 翻译图片里的文字
    Translate,
    /// AI 一键打码（模型标出敏感区域）
    AiMosaic,
    /// 提取文字（本地 OCR，不需要 API key）
    ExtractText,
    /// 滚动截图（对当前选区做滚动拼接）
    LongShot,
    /// 识别选区里的二维码（本地解码，不需要 API key）
    QrCode,
    /// 消除杂物（对选区内的物体做内容填充）
    Erase,
    /// 窗口选择器（列出可捕获窗口，挑一个作为选区）
    WindowPick,
    /// 打开设置窗口
    Settings,
    /// 完成 —— 结束编辑并把结果交回 app 层
    Complete,
    /// 复制到剪贴板
    Copy,
    /// 保存图片
    Save,
    /// 置顶在桌面
    Pin,
    /// 退出（丢弃）
    Exit,
    /// 折叠 / 展开工具栏
    Toggle,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Item {
    pub kind: ItemKind,
    pub rect: Rect,
}

/// 工具栏状态。
pub(crate) struct Toolbar {
    /// 是否折叠（折叠后只剩一个展开按钮）
    pub collapsed: bool,
    /// 工具栏整体矩形（逻辑坐标）
    pub rect: Rect,
    pub items: Vec<Item>,
}

impl Default for Toolbar {
    fn default() -> Self {
        Toolbar {
            collapsed: false,
            rect: Rect::new(0, 0, 0, 0),
            items: Vec::new(),
        }
    }
}

impl Toolbar {
    /// 按屏幕尺寸重排（逻辑坐标）。
    pub fn layout(&mut self, screen: Rect) {
        if self.collapsed {
            // 折叠：只留一个展开按钮
            let w = TOGGLE_W + PAD * 2;
            let x = screen.x + (screen.w - w) / 2;
            let y = screen.bottom() - PANEL_H - 10;
            self.rect = Rect::new(x, y, w, PANEL_H);
            self.items = vec![Item {
                kind: ItemKind::Toggle,
                rect: Rect::new(x + PAD, y + (PANEL_H - BTN) / 2, TOGGLE_W, BTN),
            }];
            return;
        }
        let avail = screen.w - 16;
        let natural = measure(1.0);
        let factor = if natural > avail && natural > 0 {
            ((avail as f32) / (natural as f32)).clamp(0.55, 1.0)
        } else {
            1.0
        };
        let width = measure(factor);
        let x = screen.x + (screen.w - width) / 2;
        let y = screen.bottom() - PANEL_H - 10;
        self.rect = Rect::new(x, y, width, PANEL_H);
        self.items = build(self.rect, factor);
    }

    /// 命中测试（逻辑坐标）。
    pub fn hit(&self, p: screenloom_core::Point) -> Option<ItemKind> {
        self.items
            .iter()
            .find(|it| it.rect.contains(p))
            .map(|it| it.kind)
    }

    /// 绘制。
    pub fn paint(&self, g: &mut Graphics, canvas: &Canvas, m: &Mapper) {
        let r = m.rect(self.rect);
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        g.fill_round_rect(x, y, w, h, 8.0, 0xEB_1E_1E_1E);
        g.draw_round_rect(x, y, w, h, 8.0, 0x66_00_00_00, 1.0);

        for it in &self.items {
            let ir = m.rect(it.rect);
            match it.kind {
                ItemKind::Tool(t) => self.paint_tool(g, t, ir, canvas),
                ItemKind::Color(c) => self.paint_swatch(g, c, ir, canvas),
                ItemKind::WidthInfo | ItemKind::FontInfo => {
                    self.paint_info(g, it.kind, ir, canvas);
                }
                ItemKind::WidthDown | ItemKind::WidthUp | ItemKind::FontDown | ItemKind::FontUp => {
                    self.paint_step(g, it.kind, ir);
                }
                ItemKind::Undo | ItemKind::Redo => {
                    let label = if it.kind == ItemKind::Undo {
                        "撤销"
                    } else {
                        "重做"
                    };
                    let enabled = if it.kind == ItemKind::Undo {
                        canvas.can_undo()
                    } else {
                        canvas.can_redo()
                    };
                    self.paint_button(g, label, ir, m, enabled);
                }
                ItemKind::AskAi => self.paint_button(g, "问 AI", ir, m, true),
                ItemKind::Translate => self.paint_button(g, "翻译", ir, m, true),
                ItemKind::AiMosaic => self.paint_button(g, "AI打码", ir, m, true),
                ItemKind::ExtractText => self.paint_button(g, "提取文字", ir, m, true),
                ItemKind::LongShot => self.paint_button(g, "滚动截图", ir, m, true),
                ItemKind::QrCode => self.paint_button(g, "识别二维码", ir, m, true),
                ItemKind::Erase => self.paint_button(g, "消除杂物", ir, m, true),
                ItemKind::WindowPick => self.paint_button(g, "窗口选择", ir, m, true),
                ItemKind::Settings => self.paint_button(g, "设置", ir, m, true),
                ItemKind::Complete => self.paint_button(g, "完成", ir, m, true),
                ItemKind::Copy => self.paint_button(g, "复制", ir, m, true),
                ItemKind::Save => self.paint_button(g, "保存图片", ir, m, true),
                ItemKind::Pin => self.paint_button(g, "置顶在桌面", ir, m, true),
                ItemKind::Exit => self.paint_button(g, "退出", ir, m, true),
                ItemKind::Toggle => {
                    let c = gdiplus::argb(Rgba::rgb(0xCC, 0xCC, 0xCC));
                    let glyph = if self.collapsed { "»" } else { "«" };
                    let (tw, th) = g.measure_text(glyph, FONT_UI * m.scale, false);
                    g.draw_text(
                        ir.x as f32 + (ir.w as f32 - tw) / 2.0,
                        ir.y as f32 + (ir.h as f32 - th) / 2.0,
                        glyph,
                        FONT_UI * m.scale,
                        c,
                        false,
                    );
                }
            }
        }
    }

    fn paint_tool(&self, g: &mut Graphics, tool: Tool, r: Rect, canvas: &Canvas) {
        let active = canvas.tool == tool;
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        if active {
            g.fill_round_rect(x, y, w, h, 5.0, 0x66_00_C3_75);
        }
        let c = if active {
            gdiplus::argb(Rgba::rgb(0x00, 0xC3, 0x75))
        } else {
            gdiplus::argb(Rgba::rgb(0xD8, 0xD8, 0xD8))
        };
        let pad = 6.0;
        let (ix, iy, iw, ih) = (x + pad, y + pad, w - pad * 2.0, h - pad * 2.0);
        match tool {
            Tool::Rect => g.draw_round_rect(ix, iy, iw, ih, 2.0, c, 1.6),
            Tool::Ellipse => g.draw_ellipse(ix, iy, iw, ih, c, 1.6),
            Tool::Arrow => draw_arrow(g, ix, iy, iw, ih, c),
            Tool::Pen => draw_pen(g, ix, iy, iw, ih, c),
            Tool::Text => draw_text_glyph(g, ix, iy, iw, ih, c),
            Tool::Mosaic => draw_mosaic(g, ix, iy, iw, ih, c),
            Tool::ColorPicker => draw_eyedropper(g, ix, iy, iw, ih, c),
            Tool::Crop => draw_brackets(g, ix, iy, iw, ih, c),
        }
    }

    fn paint_swatch(&self, g: &mut Graphics, c: Rgba, r: Rect, canvas: &Canvas) {
        let active = canvas.color == c;
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        g.fill_round_rect(x, y, w, h, 3.0, gdiplus::argb(c));
        if active {
            g.draw_round_rect(x - 2.0, y - 2.0, w + 4.0, h + 4.0, 4.0, 0xFF_FF_FF_FF, 1.6);
        }
    }

    /// 线宽 / 字号 的数值显示区（不可点）。
    fn paint_info(&self, g: &mut Graphics, kind: ItemKind, r: Rect, canvas: &Canvas) {
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let (label, value) = match kind {
            ItemKind::WidthInfo => ("宽", format!("{}", canvas.width.round() as i32)),
            _ => ("号", format!("{}", canvas.font_size.round() as i32)),
        };
        let c = gdiplus::argb(Rgba::rgb(0xD8, 0xD8, 0xD8));
        let (_tw, th) = g.measure_text(label, 11.0, false);
        g.draw_text(x + 2.0, y + (h - th) / 2.0, label, 11.0, c, false);
        let (vw, vh) = g.measure_text(&value, 11.0, false);
        g.draw_text(
            x + w - vw - 2.0,
            y + (h - vh) / 2.0,
            &value,
            11.0,
            gdiplus::argb(Rgba::rgb(0xFF, 0xFF, 0xFF)),
            false,
        );
    }

    /// "−" / "+" 按钮。
    fn paint_step(&self, g: &mut Graphics, kind: ItemKind, r: Rect) {
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let glyph = match kind {
            ItemKind::WidthDown | ItemKind::FontDown => "-",
            _ => "+",
        };
        let c = gdiplus::argb(Rgba::rgb(0xD8, 0xD8, 0xD8));
        let (tw, th) = g.measure_text(glyph, 13.0, false);
        g.draw_text(
            x + (w - tw) / 2.0,
            y + (h - th) / 2.0,
            glyph,
            13.0,
            c,
            false,
        );
    }

    fn paint_button(&self, g: &mut Graphics, label: &str, r: Rect, m: &Mapper, enabled: bool) {
        let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        let color = if enabled {
            gdiplus::argb(Rgba::rgb(0xE6, 0xE6, 0xE6))
        } else {
            gdiplus::argb(Rgba::rgb(0x77, 0x77, 0x77))
        };
        let size = FONT_UI * m.scale;
        let (tw, th) = g.measure_text(label, size, false);
        g.draw_text(
            x + (w - tw) / 2.0,
            y + (h - th) / 2.0,
            label,
            size,
            color,
            false,
        );
    }
}

// ---------------------------------------------------------------- 工具图标
//
// 全部用**几何图形**画（不依赖字体、不依赖位图资源），保证任意 DPI 下都清晰。
// 每个图标都画在 `(x, y, w, h)` 的内框里。

/// GDI+ 多边形顶点。
fn pt(x: f32, y: f32) -> PointF {
    PointF { X: x, Y: y }
}

/// 箭头：细杆 + **实心**箭头（尖端在右上）。
fn draw_arrow(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    let tip = (x + w - 1.0, y + 1.0);
    let tail = (x + 1.0, y + h - 1.0);
    let (dx, dy) = (tip.0 - tail.0, tip.1 - tail.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (vx, vy) = (-uy, ux); // 垂直方向

    // 杆画到箭头根部
    let base = (tip.0 - ux * 5.0, tip.1 - uy * 5.0);
    g.draw_line(tail.0, tail.1, base.0, base.1, c, 1.8);

    // 箭头（实心三角，底边垂直于杆）
    let hs = 3.9;
    let back = 6.2;
    let p1 = (tip.0 - ux * back + vx * hs, tip.1 - uy * back + vy * hs);
    let p2 = (tip.0 - ux * back - vx * hs, tip.1 - uy * back - vy * hs);
    g.fill_polygon(&[pt(tip.0, tip.1), pt(p1.0, p1.1), pt(p2.0, p2.1)], c);
}

/// 画笔：**实心笔尖**三角 + 笔杆（与箭头靠形状区分：笔尖是宽三角）。
fn draw_pen(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    let tip = (x + 1.0, y + h - 1.0);
    let (dx, dy) = (w - 2.0, -(h - 2.0));
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (vx, vy) = (-uy, ux);

    let base = (tip.0 + ux * len * 0.55, tip.1 + uy * len * 0.55);
    let hw = 3.2;
    let p1 = (base.0 + vx * hw, base.1 + vy * hw);
    let p2 = (base.0 - vx * hw, base.1 - vy * hw);
    g.fill_polygon(&[pt(tip.0, tip.1), pt(p1.0, p1.1), pt(p2.0, p2.1)], c);

    // 笔杆
    let cap = (tip.0 + ux * len * 0.95, tip.1 + uy * len * 0.95);
    g.draw_line(base.0, base.1, cap.0, cap.1, c, 2.4);
}

/// 文字：几何 **T**（不依赖字体，跨 DPI 更稳）。
fn draw_text_glyph(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    let t = 1.9;
    let bar_y = y + h * 0.17;
    g.draw_line(x + 1.0, bar_y, x + w - 1.0, bar_y, c, t);
    let cx = x + w / 2.0;
    g.draw_line(cx, bar_y, cx, y + h - 1.0, c, t);
}

/// 马赛克：照片框 + 3×3 网格，其中若干格填实（读作「像素化」）。
fn draw_mosaic(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    g.draw_round_rect(x, y, w, h, 1.5, c, 1.3);
    let gx = w / 3.0;
    let gy = h / 3.0;
    for i in 1..3 {
        let f = i as f32;
        g.draw_line(x + gx * f, y + 1.2, x + gx * f, y + h - 1.2, c, 0.9);
        g.draw_line(x + 1.2, y + gy * f, x + w - 1.2, y + gy * f, c, 0.9);
    }
    for (cx, cy) in [(0usize, 0usize), (1, 2), (2, 1)] {
        g.fill_rect(
            x + gx * cx as f32 + 0.7,
            y + gy * cy as f32 + 0.7,
            gx - 1.4,
            gy - 1.4,
            c,
        );
    }
}

/// 取色器：吸管（圆头 + 斜杆）。
fn draw_eyedropper(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    let r = (w.min(h)) * 0.20;
    let bx = x + w - r - 0.5;
    let by = y + r + 0.5;
    g.fill_ellipse(bx - r, by - r, r * 2.0, r * 2.0, c);
    g.draw_line(bx, by, x + 1.2, y + h - 1.2, c, 2.3);
}

/// 裁剪：四角角标（与应用图标同款符号）。
fn draw_brackets(g: &mut Graphics, x: f32, y: f32, w: f32, h: f32, c: u32) {
    let t = 1.8;
    let arm = w.min(h) * 0.34;
    let (x1, y1) = (x + w, y + h);
    // 左上
    g.fill_rect(x, y, arm, t, c);
    g.fill_rect(x, y, t, arm, c);
    // 右上
    g.fill_rect(x1 - arm, y, arm, t, c);
    g.fill_rect(x1 - t, y, t, arm, c);
    // 左下
    g.fill_rect(x, y1 - t, arm, t, c);
    g.fill_rect(x, y1 - arm, t, arm, c);
    // 右下
    g.fill_rect(x1 - arm, y1 - t, arm, t, c);
    g.fill_rect(x1 - t, y1 - arm, t, arm, c);
}

/// 逻辑 → 物理 的换算器（浮窗里用；工具栏内部用不到，只为绘制时的字号缩放）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mapper {
    pub origin: screenloom_core::Point,
    pub scale: f32,
    pub dx: i32,
    pub dy: i32,
}

impl Default for Mapper {
    fn default() -> Self {
        Mapper {
            origin: screenloom_core::Point::new(0, 0),
            scale: 1.0,
            dx: 0,
            dy: 0,
        }
    }
}

impl Mapper {
    pub fn rect(&self, r: Rect) -> Rect {
        let d = r.to_physical(self.origin, self.scale);
        Rect::new(
            d.x - self.origin.x + self.dx,
            d.y - self.origin.y + self.dy,
            d.w,
            d.h,
        )
    }
}

/// 工具栏自然宽度（给定缩放系数）。
fn measure(factor: f32) -> i32 {
    let s = factor;
    let tools = 7.0 * (BTN as f32 * s);
    let palette = 9.0 * ((SWATCH as f32 + SWATCH_GAP as f32) * s);
    let step = 2.0 * (78.0 * s); // 线宽组 + 字号组
    let undo = 2.0 * (40.0 * s);
    let ai = 52.0 * s + 52.0 * s + 68.0 * s;
    // 提取文字 / 滚动截图
    let extra = 68.0 * s + 68.0 * s;
    // 识别二维码 / 消除杂物 / 窗口选择
    let more = 68.0 * s + 68.0 * s + 68.0 * s;
    let actions = 52.0 * s + 44.0 * s + 44.0 * s + 68.0 * s + 82.0 * s + 44.0 * s;
    let toggle = TOGGLE_W as f32 * s;
    let seps = 6.0 * (SEP as f32 * s);
    let pad = 2.0 * (PAD as f32 * s);
    (tools + palette + step + undo + ai + extra + more + actions + toggle + seps + pad) as i32
}

/// 生成工具栏条目（逻辑坐标）。
fn build(rect: Rect, factor: f32) -> Vec<Item> {
    let s = factor;
    let mut items: Vec<Item> = Vec::new();
    let mut x = rect.x + (PAD as f32 * s) as i32;
    let y = rect.y + ((PANEL_H as f32 - BTN as f32 * s) / 2.0) as i32;
    let bh = (BTN as f32 * s) as i32;
    let bw = (BTN as f32 * s) as i32;

    let push = |kind: ItemKind, x: &mut i32, y: i32, w: i32, h: i32, items: &mut Vec<Item>| {
        items.push(Item {
            kind,
            rect: Rect::new(*x, y, w, h),
        });
        *x += w;
    };

    for t in [
        Tool::Rect,
        Tool::Ellipse,
        Tool::Arrow,
        Tool::Pen,
        Tool::Text,
        Tool::Mosaic,
        Tool::ColorPicker,
    ] {
        push(ItemKind::Tool(t), &mut x, y, bw, bh, &mut items);
    }
    x += (SEP as f32 * s) as i32;

    let sw = (SWATCH as f32 * s) as i32;
    let swg = (SWATCH_GAP as f32 * s) as i32;
    for c in PALETTE.iter() {
        push(
            ItemKind::Color(*c),
            &mut x,
            y + (bh - sw) / 2,
            sw,
            sw,
            &mut items,
        );
        x += swg;
    }
    x += (SEP as f32 * s) as i32;

    let step_w = (STEP_BTN as f32 * s) as i32;
    let step_h = (STEP_BTN as f32 * s) as i32;
    let step_y = y + (bh - step_h) / 2;
    // 线宽：[显示区] [-] [+]
    let info_w = (34.0 * s) as i32;
    items.push(Item {
        kind: ItemKind::WidthInfo,
        rect: Rect::new(x, step_y, info_w, step_h),
    });
    x += info_w;
    push(
        ItemKind::WidthDown,
        &mut x,
        step_y,
        step_w,
        step_h,
        &mut items,
    );
    push(
        ItemKind::WidthUp,
        &mut x,
        step_y,
        step_w,
        step_h,
        &mut items,
    );
    x += (SEP as f32 * s) as i32;
    // 字号：[显示区] [-] [+]
    items.push(Item {
        kind: ItemKind::FontInfo,
        rect: Rect::new(x, step_y, info_w, step_h),
    });
    x += info_w;
    push(
        ItemKind::FontDown,
        &mut x,
        step_y,
        step_w,
        step_h,
        &mut items,
    );
    push(ItemKind::FontUp, &mut x, step_y, step_w, step_h, &mut items);
    x += (SEP as f32 * s) as i32;

    let undo_w = (40.0 * s) as i32;
    push(ItemKind::Undo, &mut x, y, undo_w, bh, &mut items);
    push(ItemKind::Redo, &mut x, y, undo_w, bh, &mut items);
    x += (SEP as f32 * s) as i32;

    let a = |t: f32| (t * s) as i32;
    push(ItemKind::AskAi, &mut x, y, a(52.0), bh, &mut items);
    push(ItemKind::Translate, &mut x, y, a(52.0), bh, &mut items);
    push(ItemKind::AiMosaic, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::ExtractText, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::LongShot, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::QrCode, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::Erase, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::WindowPick, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::Settings, &mut x, y, a(52.0), bh, &mut items);
    push(ItemKind::Complete, &mut x, y, a(44.0), bh, &mut items);
    push(ItemKind::Copy, &mut x, y, a(44.0), bh, &mut items);
    push(ItemKind::Save, &mut x, y, a(68.0), bh, &mut items);
    push(ItemKind::Pin, &mut x, y, a(82.0), bh, &mut items);
    push(ItemKind::Exit, &mut x, y, a(44.0), bh, &mut items);

    let tw = (TOGGLE_W as f32 * s) as i32;
    push(ItemKind::Toggle, &mut x, y, tw, bh, &mut items);

    items
}
