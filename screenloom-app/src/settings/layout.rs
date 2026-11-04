//! 设置的**几何布局**：由 [`Form`] 算出面板 / 行 / 控件矩形，以及视口与滚动条。
//!
//! 纯数据 + 纯算术，不碰 Win32（只用到 [`Rect`] 与 [`px`]）。
//! 所有坐标都是**内容坐标**（`y` 从 0 起）；滚动由 `scroll.rs` 施加偏移。

use super::*;

// ---------------------------------------------------------------- 尺寸常量（逻辑像素）

/// 客户区宽度。
pub(crate) const WIN_W: i32 = 640;
/// 客户区四周留白。
pub(crate) const PAD: i32 = 20;
/// 面板内边距（左右）。
pub(crate) const PANEL_PAD_X: i32 = 18;
/// 面板内边距（上）。
pub(crate) const PANEL_PAD_TOP: i32 = 10;
/// 面板内边距（下）。
pub(crate) const PANEL_PAD_BOTTOM: i32 = 14;
/// 分组标题高度。
pub(crate) const HEADER_H: i32 = 30;
/// 行高。
pub(crate) const ROW_H: i32 = 30;
/// 行间距。
pub(crate) const ROW_GAP: i32 = 6;
/// 分组间距。
pub(crate) const SECTION_GAP: i32 = 12;
/// 标签列宽。
pub(crate) const LABEL_W: i32 = 196;
/// 标签列与控件列间距。
pub(crate) const CTRL_GAP: i32 = 16;
/// 底部按钮区与内容的间距。
pub(crate) const FOOTER_GAP: i32 = 16;
/// 输入框高度。
pub(crate) const FIELD_H: i32 = 30;
/// 按钮高度。
pub(crate) const BTN_H: i32 = 32;
/// 按钮宽度。
pub(crate) const BTN_W: i32 = 96;
/// 「浏览…」按钮宽度。
pub(crate) const BROWSE_W: i32 = 76;
/// 下拉菜单单项高度。
pub(crate) const MENU_ITEM_H: i32 = 30;
/// 客户区最小高度（再小连底栏都放不下）。
pub(crate) const MIN_CLIENT_H: i32 = 480;
/// 滚动条宽度（逻辑像素）。
pub(crate) const SCROLLBAR_W: i32 = 6;

// ---------------------------------------------------------------- 布局结果

/// 一个控件的最终几何。
#[derive(Clone, Copy)]
pub(crate) struct WidgetGeom {
    pub(crate) field: Field,
    pub(crate) kind: Kind,
    /// 控件本体矩形（输入框 / 下拉框 / 复选框方块），**内容坐标**。
    pub(crate) rect: Rect,
    /// 复选框方块右侧的文字。
    pub(crate) text: &'static str,
    /// 命中测试矩形（比控件本体大，鼠标点文字也算命中）。
    pub(crate) hit: Rect,
}

/// 一行的最终几何。
#[derive(Clone)]
pub(crate) struct RowGeom {
    pub(crate) label: &'static str,
    pub(crate) label_rect: Rect,
    pub(crate) widgets: Vec<WidgetGeom>,
    pub(crate) trailing: Option<(usize, &'static str, Rect)>,
}

/// 一个分组的面板几何。
#[derive(Clone)]
pub(crate) struct PanelGeom {
    pub(crate) title: &'static str,
    pub(crate) note: &'static str,
    pub(crate) rect: Rect,
}

/// 整体布局。
#[derive(Clone)]
pub(crate) struct Layout {
    pub(crate) panels: Vec<PanelGeom>,
    pub(crate) rows: Vec<RowGeom>,
    /// 内容高度（最后一个面板的底边，内容坐标）。
    pub(crate) content_h: i32,
    /// 客户区高度（已按工作区夹取）。
    pub(crate) client_h: i32,
    /// 可滚动视口（客户区坐标，不含底部按钮）。
    pub(crate) viewport: Rect,
    /// 滚动条轨道。
    pub(crate) scrollbar: Rect,
    /// 底部按钮（固定，不随滚动移动）。
    pub(crate) cancel: Rect,
    pub(crate) save: Rect,
}

impl Layout {
    /// 最大滚动偏移。
    pub(crate) fn scroll_max(&self, k: f32) -> i32 {
        (self.content_h + px(PAD, k) - self.viewport.h).max(0)
    }

    pub(crate) fn widget_rect(&self, field: Field) -> Option<Rect> {
        self.rows
            .iter()
            .flat_map(|r| r.widgets.iter())
            .find(|w| w.field == field)
            .map(|w| w.rect)
    }
}

/// 估算文本宽度（逻辑像素）。
///
/// 布局阶段还没有 GDI+ 上下文，只能按字符宽度粗估：
/// 中文/全角按 1 个字号宽，ASCII 按 0.55 个字号宽。
pub(crate) fn est_text_w(text: &str, size: i32) -> i32 {
    let mut w = 0.0f32;
    for ch in text.chars() {
        w += if (ch as u32) > 0x2E80 {
            size as f32
        } else {
            size as f32 * 0.55
        };
    }
    w.round() as i32
}

/// 计算整个界面的布局。
///
/// `max_client_h` 是屏幕工作区允许的最大客户区高度；内容放不下时窗口封顶，
/// 由滚动条补足（`Layout::scroll_max` > 0）。
pub(crate) fn compute_layout(form: &Form, k: f32, max_client_h: i32) -> Layout {
    let panel_w = px(WIN_W, k) - px(PAD, k) * 2;
    let inner_x = px(PAD, k) + px(PANEL_PAD_X, k);
    let inner_w = panel_w - px(PANEL_PAD_X, k) * 2;
    let label_w = px(LABEL_W, k);
    let ctrl_x = inner_x + label_w + px(CTRL_GAP, k);
    let ctrl_w = inner_w - label_w - px(CTRL_GAP, k);
    let row_h = px(ROW_H, k);
    let field_h = px(FIELD_H, k);

    let mut panels = Vec::new();
    let mut rows = Vec::new();
    let mut y = px(PAD, k);

    for (si, section) in form.sections.iter().enumerate() {
        let panel_y = y;
        let rows_y = panel_y + px(PANEL_PAD_TOP, k) + px(HEADER_H, k);
        let n_rows = section.rows.len() as i32;

        for (n, row) in section.rows.iter().enumerate() {
            let ry = rows_y + n as i32 * (row_h + px(ROW_GAP, k));
            // 有标签 → 控件在控件列；没标签（复选框行）→ 控件从标签列开始
            let (area_x, area_w) = if row.label.is_empty() {
                (inner_x, inner_w)
            } else {
                (ctrl_x, ctrl_w)
            };
            let n_widgets = row.widgets.len().max(1) as i32;

            let mut widgets = Vec::new();
            // 一行只有一个控件 → 占满控件列；多个（复选框）→ 依次紧挨着排
            let mut cursor_x = area_x;
            for w in row.widgets.iter() {
                let (rect, cell, hit_w) = match w.kind {
                    Kind::Check => {
                        let s = px(16, k);
                        let tw = px(est_text_w(w.text, 13), k);
                        let cw = s + px(8, k) + tw;
                        (
                            Rect::new(cursor_x, ry + (row_h - s) / 2, s, s),
                            cw + px(20, k),
                            cw,
                        )
                    }
                    _ => {
                        let cw = if n_widgets == 1 {
                            area_w
                        } else {
                            area_w / n_widgets
                        };
                        (
                            Rect::new(cursor_x, ry + (row_h - field_h) / 2, cw, field_h),
                            cw + px(12, k),
                            cw,
                        )
                    }
                };
                cursor_x += cell;
                widgets.push(WidgetGeom {
                    field: w.field,
                    kind: w.kind,
                    rect,
                    text: w.text,
                    // 命中区比控件本体宽（复选框的文字部分也算命中）
                    hit: Rect::new(rect.x, ry, hit_w, row_h),
                });
            }

            // 行尾按钮（「浏览…」）：输入框要给它让出位置
            let trailing = row.trailing.map(|t| {
                let bw = px(BROWSE_W, k);
                let br = Rect::new(
                    area_x + area_w - bw,
                    ry + (row_h - px(BTN_H, k)) / 2,
                    bw,
                    px(BTN_H, k),
                );
                if let Some(w) = widgets.first_mut() {
                    if matches!(w.kind, Kind::Edit | Kind::Password) {
                        w.rect.w = br.x - px(8, k) - w.rect.x;
                        w.hit.w = w.rect.w;
                    }
                }
                (t.id, t.text, br)
            });

            rows.push(RowGeom {
                label: row.label,
                label_rect: Rect::new(inner_x, ry, label_w, row_h),
                widgets,
                trailing,
            });
        }

        let section_h = px(PANEL_PAD_TOP, k)
            + px(HEADER_H, k)
            + n_rows * row_h
            + (n_rows - 1).max(0) * px(ROW_GAP, k)
            + px(PANEL_PAD_BOTTOM, k);
        panels.push(PanelGeom {
            title: section.title,
            note: section.note,
            rect: Rect::new(px(PAD, k), panel_y, panel_w, section_h),
        });
        y = panel_y + section_h;
        if si + 1 < form.sections.len() {
            y += px(SECTION_GAP, k);
        }
    }

    // 内容高度（不含底栏）
    let content_h = y;
    let footer_h = px(BTN_H, k);
    let pad = px(PAD, k);
    let natural = content_h + px(FOOTER_GAP, k) + footer_h + pad;
    let min_h = px(MIN_CLIENT_H, k);
    let max_h = max_client_h.max(min_h);
    let client_h = natural.clamp(min_h, max_h);

    // 底栏固定：客户区底部往上 PAD + 按钮高
    let footer_y = client_h - pad - footer_h;
    let client_w = px(WIN_W, k);
    let bw = px(BTN_W, k);
    let save = Rect::new(client_w - pad - bw, footer_y, bw, footer_h);
    let cancel = Rect::new(save.x - bw - px(8, k), footer_y, bw, footer_h);

    // 滚动条轨道：画在右侧留白里，不占内容宽度（避免撑宽面板）
    let track_w = px(SCROLLBAR_W, k);
    let scrollbar = Rect::new(
        client_w - pad / 2 - track_w / 2,
        pad,
        track_w,
        (footer_y - pad).max(0),
    );

    Layout {
        panels,
        rows,
        content_h,
        client_h,
        viewport: Rect::new(0, 0, client_w, footer_y),
        scrollbar,
        cancel,
        save,
    }
}
