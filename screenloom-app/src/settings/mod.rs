//! 设置窗口：**完全自绘**的深色界面。
//!
//! 自绘的部分：窗口底色、分组面板、分组标题、输入框外框、复选框、
//! 下拉框本体、下拉菜单、底部按钮外观。
//!
//! 仍然用原生控件的部分（为了 IME / 剪贴板 / 撤销 / 无障碍）：
//!
//! * **文本输入**用原生 `EDIT`，但底色与文字色改成深色，圆角外框由宿主自绘。
//! * **按钮**用原生 `BUTTON` + `BS_OWNERDRAW`，外观自绘，
//!   但 `BM_CLICK`、键盘焦点、屏幕阅读器照旧可用（端到端脚本依赖这一点）。
//!
//! 复选框与下拉框是**纯自绘**控件，自己实现命中测试、焦点环与键盘操作。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, InvalidateRect, SelectObject, SetBkColor,
    SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, FF_DONTCARE, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass, BIF_EDITBOX,
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use screenloom_core::{Config, CopyFormat, Error, Result};
use screenloom_ui::gdiplus::Graphics;

use crate::theme::{self, Rect};

const CLASS_NAME: &str = "ScreenloomSettingsWindow";
const POPUP_CLASS_NAME: &str = "ScreenloomComboPopup";
/// 自绘按钮的窗口类名。
const FLAT_BTN_CLASS: &str = "ScreenloomFlatButton";

/// 「保存」按钮 id。
const ID_SAVE: usize = 1001;
/// 「取消」按钮 id。
const ID_CANCEL: usize = 1002;
/// 「浏览…」按钮 id。
const ID_BROWSE: usize = 1003;

/// 主题热重载定时器 id（每 400ms 查一次覆盖文件的修改时间）。
const TIMER_THEME: usize = 1;

// ---------------------------------------------------------------- 自定义消息

/// 子控件 → 宿主：请求移动焦点（wp = +1 / -1）。
const WM_APP_FOCUS_NEXT: u32 = WM_APP + 21;
/// 子控件 → 宿主：请求保存。
const WM_APP_SAVE: u32 = WM_APP + 22;
/// 子控件 → 宿主：请求取消。
const WM_APP_CANCEL: u32 = WM_APP + 23;
/// 子控件 → 宿主：状态变了，重画。
const WM_APP_REDRAW: u32 = WM_APP + 24;
/// 下拉菜单 → 宿主：选中了第 n 项。
const WM_APP_COMBO_SELECT: u32 = WM_APP + 25;
/// 下拉菜单 → 宿主：菜单关闭了。
const WM_APP_COMBO_CLOSED: u32 = WM_APP + 26;
/// 子控件 → 宿主：鼠标不在按钮上了。
const WM_APP_HOVER_CLEAR: u32 = WM_APP + 28;

// ---------------------------------------------------------------- 子模块
//
// 拆分自原本的单文件 `settings.rs`：布局模型 / 窗口状态 / 自绘 / 原生控件 /
// 下拉框 / 校验各管一摊。对外仍然只有 `open()` 一个入口。

mod combo;
mod controls;
mod layout;
mod paint;
mod state;
mod validate;

pub(crate) use combo::*;
pub(crate) use controls::*;
pub(crate) use layout::*;
pub(crate) use paint::*;
pub(crate) use state::*;
pub(crate) use validate::*;

// ---------------------------------------------------------------- 对外 API

/// 打开设置窗口。
///
/// 返回 `Some(配置)` = 用户点了「保存」（且已写回配置文件）；
/// `None` = 取消。
pub fn open(path: &Path) -> Result<Option<Config>> {
    let cfg = Config::load(path).unwrap_or_default();

    // 应用 `--theme` 覆盖（给了路径才生效；没给就是内置默认）。
    // 必须在建窗**之前**应用，首帧就是覆盖后的配色。
    theme::reload();

    // 必须是**创建窗口之前**设好 DPI 感知：
    // 否则进程是 DPI-unaware，系统会把窗口整体拉伸（文字发虚），
    // 且 GetDpiForWindow 只会返回 96，控件尺寸算出来就不对。
    ensure_dpi_awareness();
    let dpi = system_dpi();
    let k = dpi as f32 / 96.0;

    // Shell 的目录选择框需要 COM
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();

    let mut state = Box::new(State {
        hwnd: HWND::default(),
        cfg,
        path: path.to_path_buf(),
        edits: HashMap::new(),
        checks: HashMap::new(),
        combos: HashMap::new(),
        buttons: HashMap::new(),
        layout: compute_layout(k),
        k,
        field_brush: field_brush(),
        focus_field: None,
        focus_button: None,
        hover_field: None,
        hover_button: None,
        pressed_button: None,
        combo: None,
        combo_hwnd: HWND::default(),
        saved: false,
        result: None,
    });
    let state_ptr = &mut *state as *mut State;

    register_class()?;
    register_popup_class()?;
    register_flat_button_class()?;
    let class = wide(CLASS_NAME);
    let title = wide("screenloom 设置");

    // 客户区尺寸 → 整窗尺寸（必须补标题栏 + 边框，否则底部按钮会被裁掉）
    let (ww, wh) = frame_adjusted(px(WIN_W, k), state.layout.client_h, dpi);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ww,
            wh,
            None,
            None,
            Some(module_handle()),
            Some(state_ptr as *const core::ffi::c_void),
        )
    }
    .map_err(|e| Error::Window(format!("创建设置窗口失败: {e}")))?;

    state.hwnd = hwnd;
    enable_dark_title(hwnd);
    build_controls(hwnd, &mut state, k);
    state.fill();
    center(hwnd);

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // 主题热重载：定时查覆盖文件的修改时间
        let _ = SetTimer(Some(hwnd), TIMER_THEME, 400, None);
    }

    // 消息循环
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if state.saved || !window_alive(hwnd) {
            break;
        }
    }

    // 注意：`state.result` 是在点「保存」时**当场**从控件读出来的
    // （见窗口过程 ID_SAVE 分支）—— 那时控件还活着。
    // 这里不要再调 collect()，窗口已经销毁，读不到任何值。
    let saved = state.saved;
    let cfg = state.result.take().unwrap_or_else(|| state.cfg.clone());
    if window_alive(hwnd) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    unsafe {
        let _ = DeleteObject(HGDIOBJ(state.field_brush.0));
    }
    if saved {
        if let Err(e) = cfg.save(&state.path) {
            if com {
                unsafe { CoUninitialize() };
            }
            return Err(e);
        }
    }
    if com {
        unsafe { CoUninitialize() };
    }
    Ok(if saved { Some(cfg) } else { None })
}

// ---------------------------------------------------------------- 基础工具

/// 逻辑像素 → 物理像素。
fn px(v: i32, k: f32) -> i32 {
    theme::px(v, k)
}

/// 进程 DPI 感知（Per-Monitor V2）。
///
/// 必须在**创建任何窗口之前**调用。重复调用会失败，忽略即可。
fn ensure_dpi_awareness() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}

/// 当前系统 DPI（最低按 96 处理）。
fn system_dpi() -> u32 {
    unsafe { GetDpiForSystem() }.max(96)
}

/// 把**客户区**尺寸换算成**窗口**尺寸（补上标题栏 + 边框）。
fn frame_adjusted(cw: i32, ch: i32, dpi: u32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    unsafe {
        let _ =
            AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WS_EX_DLGMODALFRAME, dpi);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// 深色标题栏（Windows 10 20H1+ / 11）。
fn enable_dark_title(hwnd: HWND) {
    let v: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

// ---------------------------------------------------------------- 杂项

fn register_class() -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(settings_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        // 不给背景刷：整块客户区都由我们自己画
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册设置窗口类失败".into()));
    }
    Ok(())
}

/// 窗口是否还活着。
fn window_alive(hwnd: HWND) -> bool {
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

fn module_handle() -> windows::Win32::Foundation::HINSTANCE {
    windows::Win32::Foundation::HINSTANCE(
        unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
            .unwrap_or_default()
            .0,
    )
}

/// 把窗口摆到主显示器中央。
fn center(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let x = (sw - w) / 2;
    let y = (sh - h) / 2;
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 目录选择框（SHBrowseForFolder）。
fn pick_folder(hwnd: HWND) -> Option<PathBuf> {
    let title = wide("选择截图保存目录");
    let bi = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_NEWDIALOGSTYLE | BIF_RETURNONLYFSDIRS | BIF_EDITBOX,
        ..BROWSEINFOW::default()
    };
    unsafe {
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, &mut buf).as_bool();
        CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
        if !ok {
            return None;
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }
}

// ---------------------------------------------------------------- 窗口过程

unsafe extern "system" fn settings_wnd_proc(
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
        // 自己画背景：擦除阶段什么都不做（避免闪烁）
        WM_ERASEBKGND => return LRESULT(1),

        WM_PAINT => {
            paint(hwnd, st);
            return LRESULT(0);
        }

        // 输入框：深色底 + 浅色字（颜色来自当前主题，热重载后会跟着变）
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            let (tr, tg, tb) = theme::rgb(theme::current().text);
            let (br, bg, bb) = theme::rgb(theme::current().field_bg);
            unsafe {
                SetTextColor(
                    hdc,
                    COLORREF(tr as u32 | (tg as u32) << 8 | (tb as u32) << 16),
                );
                SetBkColor(
                    hdc,
                    COLORREF(br as u32 | (bg as u32) << 8 | (bb as u32) << 16),
                );
            }
            return LRESULT(st.field_brush.0 as isize);
        }

        // 主题热重载：覆盖文件改了 → 重新应用 + 全部重绘
        WM_TIMER => {
            if wp.0 == TIMER_THEME && theme::source_changed() && theme::reload() {
                repaint_all(st);
            }
            return LRESULT(0);
        }

        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as usize;
            match id {
                ID_SAVE => {
                    // **先读控件值再销毁窗口**：DestroyWindow 之后子控件就没了，
                    // GetWindowTextW 只能拿到空串，用户刚改的值会全丢。
                    if let Some(msg) = validate(st) {
                        let t = wide(&msg);
                        let title = wide("screenloom 设置");
                        unsafe {
                            let _ = MessageBoxW(
                                Some(hwnd),
                                PCWSTR(t.as_ptr()),
                                PCWSTR(title.as_ptr()),
                                MB_OK | MB_ICONWARNING,
                            );
                        }
                        return LRESULT(0);
                    }
                    st.result = Some(st.collect());
                    st.saved = true;
                }
                ID_CANCEL => {
                    st.saved = false;
                }
                ID_BROWSE => {
                    if let Some(dir) = pick_folder(hwnd) {
                        if let Some(edit) = st.edits.get(&Field::DefaultDir).copied() {
                            set_text(edit, &dir.display().to_string());
                        }
                    }
                    return LRESULT(0);
                }
                _ => return LRESULT(0),
            }
            // 「保存」和「取消」都是关窗
            if st.saved || id == ID_CANCEL {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }

        // ---- 自绘控件的鼠标 ----
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let hit = hit_widget(st, x, y);
            if hit != st.hover_field {
                st.hover_field = hit;
                invalidate(st);
            }
            if st.hover_button.is_some() {
                st.hover_button = None;
                invalidate(st);
            }
            return LRESULT(0);
        }

        WM_LBUTTONDOWN => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            // 点到自绘控件外面 → 收起下拉 + 去掉自绘焦点
            if let Some(hit) = hit_widget(st, x, y) {
                match kind_of(st, hit) {
                    Kind::Check => {
                        let cur = *st.checks.get(&hit).unwrap_or(&false);
                        st.checks.insert(hit, !cur);
                        st.focus_field = Some(hit);
                        unsafe {
                            let _ = SetFocus(Some(hwnd));
                        }
                        invalidate(st);
                    }
                    Kind::Combo => {
                        st.focus_field = Some(hit);
                        if st.combo.as_ref().map(|c| c.field == hit).unwrap_or(false) {
                            close_combo(st);
                        } else {
                            open_combo(st, hit);
                        }
                    }
                    _ => {}
                }
            } else {
                if st.combo.is_some() {
                    close_combo(st);
                }
                st.focus_field = None;
                st.focus_button = None;
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
                invalidate(st);
            }
            return LRESULT(0);
        }

        // ---- 键盘（焦点在自绘控件上时）----
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            if vk == VK_TAB.0 as u32 {
                let dir = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                    -1i32
                } else {
                    1i32
                };
                move_focus(st, dir);
                return LRESULT(0);
            }
            if vk == VK_ESCAPE.0 as u32 {
                st.saved = false;
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            if vk == VK_RETURN.0 as u32 {
                if let Some(id) = st.focus_button {
                    activate_button(hwnd, id);
                    return LRESULT(0);
                }
                if st.combo.is_some() {
                    close_combo(st);
                } else {
                    st.result = Some(st.collect());
                    st.saved = true;
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            if vk == VK_SPACE.0 as u32 {
                if let Some(id) = st.focus_button {
                    activate_button(hwnd, id);
                    return LRESULT(0);
                }
                if let Some(f) = st.focus_field {
                    if kind_of(st, f) == Kind::Check {
                        let cur = *st.checks.get(&f).unwrap_or(&false);
                        st.checks.insert(f, !cur);
                        invalidate(st);
                    }
                }
                return LRESULT(0);
            }
            if vk == VK_UP.0 as u32 || vk == VK_DOWN.0 as u32 {
                if let Some(f) = st.focus_field {
                    if kind_of(st, f) == Kind::Combo {
                        let items = combo_items(f);
                        if !items.is_empty() {
                            let cur = st.combos.get(&f).copied().unwrap_or(0);
                            let next = if vk == VK_UP.0 as u32 {
                                cur.saturating_sub(1)
                            } else {
                                (cur + 1).min(items.len() - 1)
                            };
                            st.combos.insert(f, next);
                            invalidate(st);
                        }
                    }
                }
                return LRESULT(0);
            }
        }

        // ---- 自定义消息 ----
        WM_APP_FOCUS_NEXT => {
            move_focus(st, wp.0 as i32);
            return LRESULT(0);
        }
        WM_APP_SAVE => {
            if st.combo.is_some() {
                close_combo(st);
            } else {
                st.result = Some(st.collect());
                st.saved = true;
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_APP_CANCEL => {
            st.saved = false;
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_APP_REDRAW => {
            invalidate(st);
            return LRESULT(0);
        }
        WM_APP_HOVER_CLEAR => {
            if st.hover_button.is_some() {
                st.hover_button = None;
                invalidate(st);
            }
            return LRESULT(0);
        }
        WM_APP_COMBO_SELECT => {
            let idx = wp.0;
            if let Some(c) = &st.combo {
                let field = c.field;
                let sel = idx.min(c.items.len().saturating_sub(1));
                st.combos.insert(field, sel);
            }
            close_combo(st);
            return LRESULT(0);
        }
        WM_APP_COMBO_CLOSED => {
            if st.combo.is_some() {
                close_combo(st);
            }
            return LRESULT(0);
        }

        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_a_widget() {
        let mut seen = std::collections::HashSet::new();
        for s in SECTIONS {
            for r in s.rows {
                for w in r.widgets {
                    assert!(seen.insert(w.field), "字段重复: {:?}", w.field);
                }
            }
        }
        // 每个字段都必须出现在焦点顺序里
        for f in [
            Field::Hotkey,
            Field::HideSelf,
            Field::PrivacyMode,
            Field::ToolbarCollapsed,
            Field::CopyFormat,
            Field::DefaultDir,
            Field::LongMax,
            Field::LongSearch,
            Field::AiProvider,
            Field::AiModel,
            Field::AiEndpoint,
            Field::AiApiKey,
            Field::AiTimeout,
        ] {
            assert!(seen.contains(&f), "字段没有界面: {:?}", f);
            assert!(
                FOCUS_ITEMS.contains(&FocusItem::Field(f)),
                "字段不在焦点顺序里: {:?}",
                f
            );
        }
        assert_eq!(seen.len(), 13, "字段数量变了");
    }

    #[test]
    fn numeric_fields_are_range_checked() {
        // 合法值
        assert!(check_numeric(Field::LongSearch, "240").is_none());
        assert!(check_numeric(Field::LongMax, "0").is_none());
        assert!(check_numeric(Field::LongMax, "8888").is_none());
        assert!(check_numeric(Field::AiTimeout, "120").is_none());

        // 非数字 / 负数
        assert!(check_numeric(Field::LongSearch, "abc").is_some());
        assert!(check_numeric(Field::LongSearch, "-5").is_some());
        assert!(check_numeric(Field::AiTimeout, "0").is_some());

        // 超大值必须被挡住（否则长截图会卡死）
        assert!(check_numeric(Field::LongSearch, "999999").is_some());
        assert!(check_numeric(Field::LongMax, "999999999").is_some());
        assert!(check_numeric(Field::AiTimeout, "999999").is_some());

        // 边界内 / 边界外
        assert!(check_numeric(Field::LongSearch, "8").is_none());
        assert!(check_numeric(Field::LongSearch, "4000").is_none());
        assert!(check_numeric(Field::LongSearch, "7").is_some());
        assert!(check_numeric(Field::LongSearch, "4001").is_some());
    }

    #[test]
    fn combo_items_match_expected_order() {
        assert_eq!(COPY_FORMAT_ITEMS, &["PNG + DIB", "仅 PNG", "仅 DIB"]);
        assert_eq!(AI_PROVIDER_ITEMS, &["MiniMax", "DeepSeek"]);
    }

    #[test]
    fn checkbox_widgets_carry_their_own_text() {
        // 复选框的文字画在方块右边，所以必须自带文字。
        for s in SECTIONS {
            for r in s.rows {
                for w in r.widgets {
                    if w.kind == Kind::Check {
                        assert!(!w.text.is_empty(), "复选框必须自带文字: {:?}", w.field);
                    }
                }
            }
        }
    }

    #[test]
    fn layout_is_self_consistent() {
        // 布局的纵向推进必须和实际画出来的一致：
        // 面板底边不能越过下一个面板顶边，按钮不能越出客户区。
        let k = 1.25f32;
        let l = compute_layout(k);
        for pair in l.panels.windows(2) {
            assert!(pair[0].rect.bottom() <= pair[1].rect.y, "面板之间重叠了");
        }
        assert!(l.save.bottom() <= l.client_h, "保存按钮超出客户区");
        assert!(l.cancel.bottom() <= l.client_h, "取消按钮超出客户区");
        assert!(l.client_h > 400 && l.client_h < 1200, "客户区高度不合理");
        // 横向也不能溢出：三个复选框挤一行时必须都能放下。
        let right = px(WIN_W, k) - px(PAD, k);
        for row in &l.rows {
            for w in &row.widgets {
                assert!(
                    w.hit.right() <= right,
                    "控件 {:?} 超出右边界: {} > {}",
                    w.field,
                    w.hit.right(),
                    right
                );
            }
            if let Some((_, _, r)) = row.trailing {
                assert!(r.right() <= right, "行尾按钮超出右边界");
            }
        }
    }

    #[test]
    fn layout_scales_with_dpi() {
        let a = compute_layout(1.0);
        let b = compute_layout(1.25);
        assert!(b.client_h > a.client_h, "高 DPI 下高度应变大");
        // 1.25 倍时高度约等于 1.25 倍（允许取整误差）
        let ratio = b.client_h as f32 / a.client_h as f32;
        assert!((ratio - 1.25).abs() < 0.03, "缩放比例不对: {ratio}");
    }

    #[test]
    fn fits_1080p_work_area_at_150_percent() {
        // 1080p 屏幕扣除任务栏后约 1040 物理像素；设置窗口（含标题栏）必须放得下。
        let l = compute_layout(1.5);
        let (_, wh) = frame_adjusted(px(WIN_W, 1.5), l.client_h, 144);
        assert!(
            wh <= 1040,
            "150% DPI 下设置窗口高 {wh}px，超出 1080p 工作区"
        );
    }

    #[test]
    fn window_size_includes_title_bar_and_border() {
        // 回归测试：`CreateWindowExW` 的宽高是**整窗**尺寸，
        // 而布局算的是**客户区**高度。
        let l = compute_layout(1.0);
        let (win_w, win_h) = frame_adjusted(px(WIN_W, 1.0), l.client_h, 96);
        assert!(win_w >= px(WIN_W, 1.0));
        assert!(
            win_h > l.client_h,
            "窗口高度必须大于客户区高度（标题栏+边框没算进去）"
        );
    }

    #[test]
    fn browse_button_reserves_space_for_the_edit() {
        // 「默认保存目录」这一行：输入框必须给「浏览…」按钮让出位置，
        // 否则两者会叠在一起。
        let l = compute_layout(1.0);
        let row = l
            .rows
            .iter()
            .find(|r| r.trailing.is_some())
            .expect("没有带行尾按钮的行");
        let (_, _, btn) = row.trailing.unwrap();
        let edit = row.widgets.first().expect("没有输入框");
        assert!(
            edit.rect.right() <= btn.x,
            "输入框和行尾按钮重叠：{} >= {}",
            edit.rect.right(),
            btn.x
        );
    }

    #[test]
    fn edits_are_created_in_field_order() {
        // 端到端脚本按下标取 EDIT（0 = 热键，2 = 长截图上限），
        // 创建顺序必须和字段顺序一致，否则脚本会改错字段。
        let l = compute_layout(1.0);
        let mut order: Vec<Field> = Vec::new();
        for row in &l.rows {
            for w in &row.widgets {
                if matches!(w.kind, Kind::Edit | Kind::Password) {
                    order.push(w.field);
                }
            }
        }
        assert_eq!(
            order,
            vec![
                Field::Hotkey,
                Field::DefaultDir,
                Field::LongMax,
                Field::LongSearch,
                Field::AiModel,
                Field::AiEndpoint,
                Field::AiApiKey,
                Field::AiTimeout,
            ]
        );
    }
}
