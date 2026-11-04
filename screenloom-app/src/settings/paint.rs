//! 设置窗口的自绘：窗口底色、分组面板、输入框外框、复选框、下拉框、滚动条与焦点环。
//!
//! 所有几何都是**内容坐标**；绘制时统一加上 `-scroll.y` 并裁剪到视口。

use super::*;

// ---------------------------------------------------------------- 构建控件

/// 输入框底色刷子（`WM_CTLCOLOREDIT` 用）—— 颜色来自**当前主题**。
pub(crate) fn field_brush() -> HBRUSH {
    let (r, g, b) = theme::rgb(theme::current().field_bg);
    unsafe { CreateSolidBrush(COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)) }
}

/// 主题热重载后重建输入框底色刷子。
pub(crate) fn refresh_field_brush(st: &mut State) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(st.field_brush.0));
    }
    st.field_brush = field_brush();
}

/// 主题变了：重建画刷 + 让**所有**子窗口重绘。
///
/// 父窗口的 `WM_PAINT` 不覆盖子控件区域 —— 不逐个 invalidate，
/// 子控件（按钮 / 输入框）会保留旧配色。
pub(crate) fn repaint_all(st: &mut State) {
    refresh_field_brush(st);
    invalidate(st);
    for h in st.buttons.values() {
        unsafe {
            let _ = InvalidateRect(Some(*h), None, false);
        }
    }
    for h in st.edits.values() {
        unsafe {
            let _ = InvalidateRect(Some(*h), None, false);
        }
    }
    if !st.combo_hwnd.0.is_null() {
        unsafe {
            let _ = InvalidateRect(Some(st.combo_hwnd), None, false);
        }
    }
}

// ---------------------------------------------------------------- 绘制

pub(crate) fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// 内容坐标 → 屏幕（客户区）坐标。
pub(crate) fn shift(r: Rect, dy: i32) -> Rect {
    Rect::new(r.x, r.y + dy, r.w, r.h)
}

/// 画整个界面（双缓冲）。
pub(crate) fn paint(hwnd: HWND, st: &State) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let (w, h) = client_size(hwnd);
    if w <= 0 || h <= 0 {
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        return;
    }

    let mem = unsafe { CreateCompatibleDC(Some(hdc)) };
    let bmp = unsafe { CreateCompatibleBitmap(hdc, w, h) };
    if !mem.0.is_null() && !bmp.0.is_null() {
        unsafe {
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            if let Ok(mut g) = Graphics::from_hdc(mem) {
                g.setup_quality();
                draw_scene(&mut g, st, w, h);
            }
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            let _ = SelectObject(mem, old);
        }
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
        }
    }

    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

/// 画界面内容（不含原生按钮 —— 那些走 owner-draw）。
pub(crate) fn draw_scene(g: &mut Graphics, st: &State, w: i32, h: i32) {
    let k = st.k;
    let viewport = st.layout.viewport;
    let dy = -st.scroll.y;

    g.fill_rect(0.0, 0.0, w as f32, h as f32, theme::current().bg);

    // 滚动区裁剪：内容不会画到底栏上
    g.clip_rect(
        viewport.x as f32,
        viewport.y as f32,
        viewport.w as f32,
        viewport.h as f32,
    );

    // 分组面板 + 标题
    for p in &st.layout.panels {
        let r = shift(p.rect, dy);
        if !r.intersects(&viewport) {
            continue;
        }
        theme::panel(g, r, k);
        theme::section_header(
            g,
            r.x + px(PANEL_PAD_X, k),
            r.y + px(PANEL_PAD_TOP, k),
            p.title,
            p.note,
            k,
        );
    }

    // 行
    let focused_edit = focused_edit(st);
    for row in &st.layout.rows {
        let label_rect = shift(row.label_rect, dy);
        let row_visible = label_rect.intersects(&viewport);
        if !row_visible
            && row
                .widgets
                .iter()
                .all(|wg| !shift(wg.hit, dy).intersects(&viewport))
        {
            continue;
        }

        if !row.label.is_empty() {
            let f = px(13, k) as f32;
            let (_, th) = g.measure_text(row.label, f, false);
            let ty = label_rect.y as f32 + (label_rect.h as f32 - th) / 2.0 - px(1, k) as f32;
            g.draw_text(
                label_rect.x as f32,
                ty,
                row.label,
                f,
                theme::current().text_dim,
                false,
            );
        }
        for wg in &row.widgets {
            let r = shift(wg.rect, dy);
            match wg.kind {
                Kind::Edit | Kind::Password => {
                    let focused = focused_edit == Some(wg.field);
                    let hovered = st.hover_field == Some(wg.field);
                    theme::input_frame(g, r, focused, hovered, k);
                }
                Kind::Check => {
                    let on = st.bool_of(wg.field);
                    let hov = st.hover_field == Some(wg.field);
                    theme::checkbox(g, r.x, r.y, r.w, on, hov, k);
                    // 文字
                    let f = px(13, k) as f32;
                    let tx = r.right() + px(8, k);
                    let (_, th) = g.measure_text(wg.text, f, false);
                    let ty = r.y as f32 + (r.h as f32 - th) / 2.0 - px(1, k) as f32;
                    g.draw_text(tx as f32, ty, wg.text, f, theme::current().text, false);
                    if st.focus_field == Some(wg.field) {
                        theme::focus_ring(g, shift(wg.hit, dy), k);
                    }
                }
                Kind::Combo => {
                    let sel = st.choice_of(wg.field);
                    let items = st.choice_items(wg.field);
                    let text = items.get(sel).map(String::as_str).unwrap_or("");
                    let open = st
                        .combo
                        .as_ref()
                        .map(|c| c.field == wg.field)
                        .unwrap_or(false);
                    theme::combo_box(
                        g,
                        r,
                        text,
                        open,
                        st.hover_field == Some(wg.field),
                        st.focus_field == Some(wg.field),
                        k,
                    );
                }
            }
        }
    }

    g.clear_clip();
    draw_scrollbar(g, st);
}

/// 滚动条：内容不溢出时不画。
fn draw_scrollbar(g: &mut Graphics, st: &State) {
    if !st.scroll.overflows() {
        return;
    }
    let track = st.layout.scrollbar;
    let thumb = st.scroll.thumb(track, st.k);
    let active = st.scroll.hover || st.scroll.drag.is_some();
    theme::scrollbar(g, track, thumb, active, st.k);
}

/// 当前获得键盘焦点的输入框字段。
pub(crate) fn focused_edit(st: &State) -> Option<Field> {
    let h = unsafe { GetFocus() };
    if h.0.is_null() {
        return None;
    }
    st.edits.iter().find(|(_, v)| **v == h).map(|(k, _)| *k)
}

// ---------------------------------------------------------------- 命中测试

/// 命中一个自绘控件（鼠标坐标是客户区坐标，需要换算到内容坐标）。
pub(crate) fn hit_widget(st: &State, x: i32, y: i32) -> Option<Field> {
    if !st.layout.viewport.contains(x, y) {
        return None;
    }
    let cy = y + st.scroll.y;
    for row in &st.layout.rows {
        for wg in &row.widgets {
            if matches!(wg.kind, Kind::Check | Kind::Combo) && wg.hit.contains(x, cy) {
                return Some(wg.field);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- 焦点

/// 当前焦点项在焦点顺序里的下标。
pub(crate) fn focus_index(st: &State) -> Option<usize> {
    let items = st.form.focus_items();
    if let Some(f) = st.focus_field {
        return items.iter().position(|i| *i == FocusItem::Field(f));
    }
    if let Some(id) = st.focus_button {
        return items.iter().position(|i| *i == FocusItem::Button(id));
    }
    let h = unsafe { GetFocus() };
    if h.0.is_null() {
        return None;
    }
    items.iter().position(|item| match item {
        FocusItem::Field(f) => st.edits.get(f) == Some(&h),
        FocusItem::Button(id) => st.buttons.get(id) == Some(&h),
    })
}

/// 把焦点移到第 idx 项；焦点控件不在视口里时自动滚进来。
pub(crate) fn focus_to(st: &mut State, idx: usize) {
    let items = st.form.focus_items();
    let item = match items.get(idx) {
        Some(i) => *i,
        None => return,
    };
    match item {
        FocusItem::Field(f) => {
            st.focus_button = None;
            if let Some(h) = st.edits.get(&f).copied() {
                st.focus_field = None;
                unsafe {
                    let _ = SetFocus(Some(h));
                }
            } else {
                st.focus_field = Some(f);
                unsafe {
                    let _ = SetFocus(Some(st.hwnd));
                }
            }
            if let Some(r) = st.layout.widget_rect(f) {
                if st.scroll.ensure_visible(r, st.k) {
                    sync_children(st);
                }
            }
        }
        FocusItem::Button(id) => {
            // 按钮**不真正持有焦点**（焦点环自己画），
            // 否则系统会画一个丑陋的虚线焦点框。
            st.focus_field = None;
            st.focus_button = Some(id);
            unsafe {
                let _ = SetFocus(Some(st.hwnd));
            }
        }
    }
    invalidate(st);
}

pub(crate) fn invalidate(st: &State) {
    unsafe {
        let _ = InvalidateRect(Some(st.hwnd), None, false);
    }
}
