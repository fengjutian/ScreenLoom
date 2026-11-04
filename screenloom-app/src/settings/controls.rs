//! 原生子控件：`EDIT` / `BUTTON` 的创建、字体、文本读写、位置同步与子类化窗口过程。

use super::*;

/// UTF-16（带结尾 NUL）。
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn build_controls(parent: HWND, st: &mut State) {
    let font = make_font(st.k);
    let state_ptr = &mut *st as *mut State;

    // 输入框（保持**创建顺序** = 字段顺序，端到端脚本按下标取控件）
    let mut edits: Vec<(Field, Rect, bool)> = Vec::new();
    for row in &st.layout.rows {
        for wg in &row.widgets {
            match wg.kind {
                Kind::Edit => edits.push((wg.field, wg.rect, false)),
                Kind::Password => edits.push((wg.field, wg.rect, true)),
                _ => {}
            }
        }
    }
    for (field, rect, password) in edits {
        let h = create_edit(parent, rect, password, font, st.k);
        st.edits.insert(field, h);
        unsafe {
            let _ = SetWindowSubclass(h, Some(child_subclass), 1, 0);
        }
    }

    // 行尾按钮（「浏览…」）
    let mut trailing: Vec<(usize, &'static str, Rect)> = Vec::new();
    for row in &st.layout.rows {
        if let Some(t) = row.trailing {
            trailing.push(t);
        }
    }
    for (id, text, rect) in trailing {
        let h = create_button(parent, text, rect, id, font, state_ptr);
        st.buttons.insert(id, h);
    }

    // 底部按钮（固定，不随滚动）
    let save = create_button(parent, "保存", st.layout.save, ID_SAVE, font, state_ptr);
    st.buttons.insert(ID_SAVE, save);
    let cancel = create_button(parent, "取消", st.layout.cancel, ID_CANCEL, font, state_ptr);
    st.buttons.insert(ID_CANCEL, cancel);
}

/// 把子控件摆到「内容坐标 + 滚动偏移」处。
///
/// 用 `BeginDeferWindowPos` 批量移动，避免逐个 `SetWindowPos` 的中间态闪烁。
/// 底栏按钮位置固定，不参与。
pub(crate) fn sync_children(st: &State) {
    let dy = -st.scroll.y;

    let mut jobs: Vec<(HWND, Rect)> = Vec::new();
    for (field, hwnd) in &st.edits {
        if let Some(r) = st.layout.widget_rect(*field) {
            jobs.push((*hwnd, edit_inner(r, st.k)));
        }
    }
    for row in &st.layout.rows {
        if let Some((id, _, r)) = row.trailing {
            if let Some(hwnd) = st.buttons.get(&id) {
                jobs.push((*hwnd, r));
            }
        }
    }

    unsafe {
        if let Ok(mut hdwp) = BeginDeferWindowPos(jobs.len() as i32) {
            for (hwnd, r) in &jobs {
                match DeferWindowPos(
                    hdwp,
                    *hwnd,
                    None,
                    r.x,
                    r.y + dy,
                    r.w,
                    r.h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                ) {
                    Ok(next) => hdwp = next,
                    Err(_) => return,
                }
            }
            let _ = EndDeferWindowPos(hdwp);
        } else {
            for (hwnd, r) in &jobs {
                let _ = SetWindowPos(
                    *hwnd,
                    None,
                    r.x,
                    r.y + dy,
                    r.w,
                    r.h,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
    }
}

/// 输入框在自绘外框里的内缩矩形（两端留 11px 给圆角与描边）。
pub(crate) fn edit_inner(r: Rect, k: f32) -> Rect {
    Rect::new(
        r.x + px(11, k),
        r.y + px(4, k),
        (r.w - px(22, k)).max(1),
        (r.h - px(8, k)).max(1),
    )
}

/// 文本输入框。
///
/// 不加 `WS_BORDER` / `WS_EX_CLIENTEDGE` —— 外框由宿主自绘圆角，
/// 框内底色由 `WM_CTLCOLOREDIT` 设成和自绘底色**完全一致**的深色，
/// 这样矩形输入框和圆角外框看不出接缝。
pub(crate) fn create_edit(parent: HWND, r: Rect, password: bool, font: HFONT, k: f32) -> HWND {
    let class = wide("EDIT");
    let style = WS_CHILD.0
        | WS_VISIBLE.0
        | WS_TABSTOP.0
        | ES_AUTOHSCROLL as u32
        | if password { ES_PASSWORD as u32 } else { 0 };
    let inner = edit_inner(r, k);
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(style),
            inner.x,
            inner.y,
            inner.w,
            inner.h,
            Some(parent),
            None,
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        );
    }
    h
}

/// 自绘按钮：外观、悬停、按下、焦点环全部自己画。
///
/// * `WS_EX_NOACTIVATE` —— 按钮**永不**持有键盘焦点，
///   否则系统会画一个丑陋的虚线焦点框，而且父窗口重绘不覆盖子控件区域，
///   那个框会一直留在屏幕上。
/// * `BM_CLICK` 照样支持（端到端脚本用它点「保存」）。
pub(crate) fn create_button(
    parent: HWND,
    text: &str,
    r: Rect,
    id: usize,
    font: HFONT,
    state: *mut State,
) -> HWND {
    let class = wide(FLAT_BTN_CLASS);
    let t = wide(text);
    let h = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            r.x,
            r.y,
            r.w,
            r.h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(module_handle()),
            Some(state as *const core::ffi::c_void),
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        );
    }
    h
}

/// 触发一个按钮（等价于原生按钮的 `BN_CLICKED`）。
pub(crate) fn activate_button(parent: HWND, id: usize) {
    unsafe {
        let _ = SendMessageW(
            parent,
            WM_COMMAND,
            Some(WPARAM(id)),
            Some(LPARAM(parent.0 as isize)),
        );
    }
}

/// 界面字体。
pub(crate) fn make_font(k: f32) -> HFONT {
    let h = -px(13, k);
    let face = wide("Microsoft YaHei");
    unsafe {
        CreateFontW(
            h,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

// ---------------------------------------------------------------- 控件读写

pub(crate) fn set_text(hwnd: HWND, text: &str) {
    if hwnd.0.is_null() {
        return;
    }
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
    }
}

pub(crate) fn get_text(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) }.clamp(0, buf.len() as i32 - 1) as usize;
    String::from_utf16_lossy(&buf[..n])
}

pub(crate) fn register_flat_button_class() -> Result<()> {
    let name = wide(FLAT_BTN_CLASS);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(flat_button_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册自绘按钮窗口类失败".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------- 子控件子类化

/// 原生输入框的子类过程：Tab / 回车 / Esc 统一交给宿主，焦点变化通知重画。
pub(crate) unsafe extern "system" fn child_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _refdata: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            if vk == VK_TAB.0 as u32 {
                let dir = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                    -1i32
                } else {
                    1i32
                };
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(
                    Some(parent),
                    WM_APP_FOCUS_NEXT,
                    WPARAM(dir as usize),
                    LPARAM(0),
                );
                return LRESULT(0);
            }
            if vk == VK_ESCAPE.0 as u32 {
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(Some(parent), WM_APP_CANCEL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            // 回车 = 保存
            if vk == VK_RETURN.0 as u32 {
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(Some(parent), WM_APP_SAVE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            // Tab 不要被插进文本框
            if wp.0 == 9 {
                return LRESULT(0);
            }
        }
        // 滚轮转给宿主：单行 EDIT 自己不会滚，别让它把消息吃掉。
        WM_MOUSEWHEEL => {
            let parent = GetParent(hwnd).unwrap_or_default();
            let _ = PostMessageW(Some(parent), WM_MOUSEWHEEL, wp, lp);
            return LRESULT(0);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let parent = GetParent(hwnd).unwrap_or_default();
            let _ = PostMessageW(Some(parent), WM_APP_REDRAW, WPARAM(0), LPARAM(0));
            if msg == WM_SETFOCUS {
                set_light_caret(hwnd);
            }
        }
        WM_MOUSEMOVE => {
            // 鼠标不在按钮上了
            let parent = GetParent(hwnd).unwrap_or_default();
            let _ = PostMessageW(Some(parent), WM_APP_HOVER_CLEAR, WPARAM(0), LPARAM(0));
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// 深色输入框需要浅色插入符，否则黑底上根本看不见。
pub(crate) fn set_light_caret(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    let h = (r.bottom - r.top).max(4);
    let brush = unsafe { CreateSolidBrush(COLORREF(0x00_FF_FF_FF)) };
    unsafe {
        let _ = CreateCaret(hwnd, Some(HBITMAP(brush.0)), 1, h);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

// ---------------------------------------------------------------- 自绘按钮窗口过程

/// 自绘按钮的窗口过程。
///
/// 按钮自己画外观、自己处理悬停 / 按下 / 释放，
/// `BM_CLICK` 也照样响应（端到端脚本靠它点「保存」）。
pub(crate) unsafe extern "system" fn flat_button_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut State;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;
    let id = GetDlgCtrlID(hwnd) as usize;

    match msg {
        WM_ERASEBKGND => return LRESULT(1),

        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            if let Ok(mut g) = Graphics::from_hdc(hdc) {
                g.setup_quality();
                let rect = Rect::new(0, 0, r.right - r.left, r.bottom - r.top);
                let style = if id == ID_SAVE {
                    theme::BtnStyle::Primary
                } else {
                    theme::BtnStyle::Secondary
                };
                let state = if st.pressed_button == Some(id) {
                    theme::BtnState::Pressed
                } else if st.hover_button == Some(id) {
                    theme::BtnState::Hover
                } else {
                    theme::BtnState::Normal
                };
                // 按钮背后：行尾按钮在面板上，底部按钮在窗口底色上
                let behind = {
                    let t = theme::current();
                    if id == ID_BROWSE {
                        t.panel
                    } else {
                        t.bg
                    }
                };
                let text = get_text(hwnd);
                theme::button(&mut g, rect, &text, style, state, st.k, behind);
                if st.focus_button == Some(id) {
                    theme::focus_ring(&mut g, rect, st.k);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }

        WM_MOUSEMOVE => {
            if st.hover_button != Some(id) {
                st.hover_button = Some(id);
                invalidate(st);
            }
            return LRESULT(0);
        }

        WM_LBUTTONDOWN => {
            st.pressed_button = Some(id);
            unsafe {
                let _ = SetCapture(hwnd);
            }
            invalidate(st);
            return LRESULT(0);
        }

        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let pressed = st.pressed_button.take();
            st.pressed_button = None;
            invalidate(st);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            if pressed == Some(id) && x >= 0 && y >= 0 && x < r.right && y < r.bottom {
                activate_button(st.hwnd, id);
            }
            return LRESULT(0);
        }

        // `BM_CLICK`：和原生按钮一样，外部脚本可以拿它来点按钮
        BM_CLICK => {
            activate_button(st.hwnd, id);
            return LRESULT(0);
        }

        WM_SETFOCUS | WM_KILLFOCUS => {
            // 按钮不持有焦点（WS_EX_NOACTIVATE），焦点环由宿主自绘
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 移动焦点（dir = +1 / -1）。
pub(crate) fn move_focus(st: &mut State, dir: i32) {
    let items = st.form.focus_items();
    let n = items.len() as i32;
    if n == 0 {
        return;
    }
    let cur = focus_index(st).map(|v| v as i32).unwrap_or(-1);
    let mut next = cur + dir;
    if next < 0 {
        next = n - 1;
    }
    if next >= n {
        next = 0;
    }
    focus_to(st, next as usize);
}
