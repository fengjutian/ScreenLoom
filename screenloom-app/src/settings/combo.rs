//! 自绘下拉框：展开 / 收起、弹出窗口过程、候选菜单命中测试。

use super::*;

pub(crate) fn register_popup_class() -> Result<()> {
    let name = wide(POPUP_CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(popup_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册下拉菜单窗口类失败".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------- 下拉菜单

/// 打开某个下拉框。
pub(crate) fn open_combo(st: &mut State, field: Field) {
    // 复制一份候选项：菜单打开期间表单可能已经换成新的（不会，但保持简单）
    let items = st.choice_items(field).to_vec();
    if items.is_empty() {
        return;
    }
    let selected = st.choice_of(field);
    // 控件矩形（内容坐标）→ 客户区坐标
    let Some(r) = st.layout.widget_rect(field) else {
        return;
    };
    let ctrl = Rect::new(r.x, r.y - st.scroll.y, r.w, r.h);

    let k = st.k;
    let item_h = px(MENU_ITEM_H, k);
    let menu_h = items.len() as i32 * item_h + px(8, k);
    let menu_w = ctrl.w;

    // 客户区 → 屏幕坐标（下方放不下就往上开）
    let mut below = POINT {
        x: ctrl.x,
        y: ctrl.bottom(),
    };
    let mut above = POINT {
        x: ctrl.x,
        y: ctrl.y,
    };
    unsafe {
        let _ = ClientToScreen(st.hwnd, &mut below);
        let _ = ClientToScreen(st.hwnd, &mut above);
    }
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let (menu_x, menu_y) = if below.y + menu_h > screen_h {
        (above.x, above.y - menu_h - px(4, k))
    } else {
        (below.x, below.y + px(4, k))
    };

    st.combo = Some(ComboState {
        field,
        items,
        selected,
        hover: None,
    });

    // 创建 / 复用弹出窗口
    let class = wide(POPUP_CLASS_NAME);
    let hwnd = if st.combo_hwnd.0.is_null() {
        let h = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                menu_x,
                menu_y,
                menu_w,
                menu_h,
                Some(st.hwnd),
                None,
                Some(module_handle()),
                Some(&mut *st as *mut State as *const core::ffi::c_void),
            )
        }
        .unwrap_or_default();
        st.combo_hwnd = h;
        h
    } else {
        st.combo_hwnd
    };

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            menu_x,
            menu_y,
            menu_w,
            menu_h,
            SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    invalidate(st);
}

/// 关掉下拉菜单。
pub(crate) fn close_combo(st: &mut State) {
    st.combo = None;
    if !st.combo_hwnd.0.is_null() && window_alive(st.combo_hwnd) {
        unsafe {
            let _ = DestroyWindow(st.combo_hwnd);
        }
    }
    st.combo_hwnd = HWND::default();
    unsafe {
        let _ = SetFocus(Some(st.hwnd));
    }
    invalidate(st);
}

/// 下拉菜单窗口过程。
pub(crate) unsafe extern "system" fn popup_wnd_proc(
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

    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let (w, h) = client_size(hwnd);
            if let Ok(mut g) = Graphics::from_hdc(hdc) {
                g.setup_quality();
                if let Some(c) = &st.combo {
                    let r = Rect::new(0, 0, w, h);
                    let item_h = px(MENU_ITEM_H, st.k);
                    let refs: Vec<&str> = c.items.iter().map(String::as_str).collect();
                    theme::menu(&mut g, r, &refs, c.selected, c.hover, item_h, st.k);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let idx = item_index(st, x, y);
            if let Some(c) = &mut st.combo {
                c.hover = idx;
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let idx = item_index(st, x, y);
            if let Some(i) = idx {
                unsafe {
                    let _ = PostMessageW(Some(st.hwnd), WM_APP_COMBO_SELECT, WPARAM(i), LPARAM(0));
                }
            }
            return LRESULT(0);
        }
        WM_KILLFOCUS => {
            unsafe {
                let _ = PostMessageW(Some(st.hwnd), WM_APP_COMBO_CLOSE, WPARAM(0), LPARAM(0));
            }
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 下拉菜单里第几项（坐标 → 下标）。
pub(crate) fn item_index(st: &State, _x: i32, y: i32) -> Option<usize> {
    let c = st.combo.as_ref()?;
    let item_h = px(MENU_ITEM_H, st.k);
    let top = px(4, st.k);
    if y < top {
        return None;
    }
    let i = ((y - top) / item_h) as usize;
    if i < c.items.len() {
        Some(i)
    } else {
        None
    }
}
