//! 设置窗口：**完全自绘**的深色界面，数据驱动的表单 + 滚动视口。
//!
//! 自绘的部分：窗口底色、分组面板、分组标题、输入框外框、复选框、
//! 下拉框本体、下拉菜单、底部按钮外观、滚动条。
//!
//! 仍然用原生控件的部分（为了 IME / 剪贴板 / 撤销 / 无障碍）：
//!
//! * **文本输入**用原生 `EDIT`，但底色与文字色改成深色，圆角外框由宿主自绘。
//! * **按钮**用原生 `BUTTON` + `BS_OWNERDRAW`，外观自绘，
//!   但 `BM_CLICK`、键盘焦点、屏幕阅读器照旧可用（端到端脚本依赖这一点）。
//!
//! 复选框与下拉框是**纯自绘**控件，自己实现命中测试、焦点环与键盘操作。
//!
//! 表单模型见 [`form`]（ADR-001：`doc/设置表单引擎与滚动方案.md`）；
//! 滚动数学见 [`scroll`]。

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
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_END, VK_ESCAPE,
    VK_HOME, VK_NEXT, VK_PRIOR, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass, BIF_EDITBOX,
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use screenloom_core::{Config, Error, Result};
use screenloom_ui::gdiplus::Graphics;

use crate::theme::{self, Rect};

const CLASS_NAME: &str = "ScreenloomSettingsWindow";
/// 录屏设置窗口类名（保持独立：端到端脚本按类名找窗口）。
const RECORD_CLASS_NAME: &str = "ScreenloomRecordSettings";
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
const WM_APP_COMBO_CLOSE: u32 = WM_APP + 26;
/// 子控件 → 宿主：鼠标不在按钮上了。
const WM_APP_HOVER_CLEAR: u32 = WM_APP + 28;

// ---------------------------------------------------------------- 子模块

mod combo;
mod controls;
mod form;
mod layout;
mod paint;
mod scroll;
mod state;

pub(crate) use combo::*;
pub(crate) use controls::*;
pub(crate) use form::*;
pub(crate) use layout::*;
pub(crate) use paint::*;
pub(crate) use scroll::*;
pub(crate) use state::*;

// ---------------------------------------------------------------- 对外 API

/// 打开截图设置窗口。
///
/// 返回 `Some(配置)` = 用户点了「保存」（且已写回配置文件）；
/// `None` = 取消。
pub fn open(path: &Path) -> Result<Option<Config>> {
    open_form(path, FormKind::Capture, &[])
}

/// 打开录屏设置窗口。
pub fn open_record(path: &Path) -> Result<Option<Config>> {
    let monitors = screenloom_capture::list_monitors().unwrap_or_default();
    open_form(path, FormKind::Record, &monitors)
}

/// 打开一张表单（截图 / 录屏共用同一套窗口逻辑）。
fn open_form(
    path: &Path,
    kind: FormKind,
    monitors: &[screenloom_capture::MonitorInfo],
) -> Result<Option<Config>> {
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

    let form = match kind {
        FormKind::Capture => capture_form(&cfg),
        FormKind::Record => record_form(&cfg, monitors),
    };
    let form_kind = form.kind;

    // 工作区高度：屏幕高度减去任务栏余量，再留 80 逻辑像素给标题栏/边距
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let max_client_h = (screen_h - px(80, k)).max(px(MIN_CLIENT_H, k));
    let layout = compute_layout(&form, k, max_client_h);
    let mut scroll = Scroll::default();
    scroll.set_extent(layout.scroll_max(k), layout.viewport.h);

    let mut state = Box::new(State {
        hwnd: HWND::default(),
        kind,
        form,
        draft: cfg,
        path: path.to_path_buf(),
        layout,
        scroll,
        k,
        edits: HashMap::new(),
        buttons: HashMap::new(),
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

    register_class(form_kind)?;
    register_popup_class()?;
    register_flat_button_class()?;
    let class = wide(form_kind.class_name());
    let title = wide(form_kind.title());

    // 客户区尺寸 → 整窗尺寸（必须补标题栏 + 边框，否则底部按钮会被裁掉）
    let (ww, wh) = frame_adjusted(px(WIN_W, k), state.layout.client_h, dpi);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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
    build_controls(hwnd, &mut state);
    state.fill_text();
    sync_children(&state);
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
            let _ = DispatchMessageW(&msg);
        }
        if state.saved || !window_alive(hwnd) {
            break;
        }
    }

    // `state.result` 是在点「保存」时**当场**从控件读出来的
    // （见窗口过程 ID_SAVE 分支）—— 那时控件还活着。
    // 这里不要再读控件，窗口已经销毁。
    let saved = state.saved;
    let cfg = state.result.take().unwrap_or_else(|| state.draft.clone());
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

fn register_class(kind: FormKind) -> Result<()> {
    let name = wide(kind.class_name());
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
        // 不给背景刷：整块客户区都由我们自己画；裁剪子控件避免滚动闪烁
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
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
    let title = wide("选择保存目录");
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

/// 「浏览…」按钮对应的字段（每张表单最多一个）。
fn browse_field(st: &State) -> Option<Field> {
    st.layout
        .rows
        .iter()
        .find(|r| r.trailing.map(|t| t.0) == Some(ID_BROWSE))
        .and_then(|r| r.widgets.first())
        .map(|w| w.field)
}

/// 滚动之后：关掉下拉（位置已失效）、同步子控件、重绘。
fn after_scroll(st: &mut State, moved: bool) {
    if !moved {
        return;
    }
    if st.combo.is_some() {
        close_combo(st);
    }
    sync_children(st);
    invalidate(st);
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
                    // **先读控件值再销毁窗口**：DestroyWindow 之后子控件就没了。
                    if let Some(msg) = st.validate() {
                        let t = wide(&msg);
                        let title = wide(st.kind.title());
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
                    st.result = Some(st.take_result());
                    st.saved = true;
                }
                ID_CANCEL => {
                    st.saved = false;
                }
                ID_BROWSE => {
                    if let Some(field) = browse_field(st) {
                        if let Some(dir) = pick_folder(hwnd) {
                            if let Some(edit) = st.edits.get(&field).copied() {
                                set_text(edit, &dir.display().to_string());
                            }
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

        // ---- 滚轮 ----
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
            if st.scroll.wheel(delta, st.k) {
                after_scroll(st, true);
            }
            return LRESULT(0);
        }

        // ---- 自绘控件的鼠标 ----
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;

            // 拖动滚动条
            if st.scroll.drag.is_some() {
                let track = st.layout.scrollbar;
                if st.scroll.drag_to(track, y, st.k) {
                    sync_children(st);
                }
                invalidate(st);
                return LRESULT(0);
            }

            // 悬停滚动条
            let over_bar = st.scroll.overflows() && st.layout.scrollbar.contains(x, y);
            if over_bar != st.scroll.hover {
                st.scroll.hover = over_bar;
                invalidate(st);
            }

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

            // 滚动条优先：滑块起拖，轨道翻页
            if st.scroll.overflows() && st.layout.scrollbar.contains(x, y) {
                let track = st.layout.scrollbar;
                if st.scroll.begin_drag(track, y, st.k) {
                    unsafe {
                        let _ = SetCapture(hwnd);
                    }
                } else if st.scroll.click_track(track, y, st.k) {
                    sync_children(st);
                }
                invalidate(st);
                return LRESULT(0);
            }

            // 点到自绘控件外面 → 收起下拉 + 去掉自绘焦点
            if let Some(hit) = hit_widget(st, x, y) {
                match st.kind_of(hit) {
                    Kind::Check => {
                        let cur = st.bool_of(hit);
                        st.set_bool(hit, !cur);
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

        WM_LBUTTONUP => {
            if st.scroll.drag.is_some() {
                st.scroll.drag = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
                invalidate(st);
            }
            return LRESULT(0);
        }

        // ---- 键盘（焦点在宿主 / 自绘控件上时）----
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
            if vk == VK_PRIOR.0 as u32 {
                let moved = st.scroll.page(-1);
                after_scroll(st, moved);
                return LRESULT(0);
            }
            if vk == VK_NEXT.0 as u32 {
                let moved = st.scroll.page(1);
                after_scroll(st, moved);
                return LRESULT(0);
            }
            if vk == VK_HOME.0 as u32 {
                let moved = st.scroll.home();
                after_scroll(st, moved);
                return LRESULT(0);
            }
            if vk == VK_END.0 as u32 {
                let moved = st.scroll.end();
                after_scroll(st, moved);
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
                    st.result = Some(st.take_result());
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
                    if st.kind_of(f) == Kind::Check {
                        let cur = st.bool_of(f);
                        st.set_bool(f, !cur);
                        invalidate(st);
                    }
                }
                return LRESULT(0);
            }
            if vk == VK_UP.0 as u32 || vk == VK_DOWN.0 as u32 {
                if let Some(f) = st.focus_field {
                    if st.kind_of(f) == Kind::Combo {
                        let len = st.choice_items(f).len();
                        if len > 0 {
                            let cur = st.choice_of(f);
                            let next = if vk == VK_UP.0 as u32 {
                                cur.saturating_sub(1)
                            } else {
                                (cur + 1).min(len - 1)
                            };
                            st.set_choice(f, next);
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
                st.result = Some(st.take_result());
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
                st.set_choice(field, sel);
            }
            close_combo(st);
            return LRESULT(0);
        }
        WM_APP_COMBO_CLOSE => {
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

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> Form {
        capture_form(&Config::default())
    }

    fn fake_monitors() -> Vec<screenloom_capture::MonitorInfo> {
        vec![
            screenloom_capture::MonitorInfo {
                index: 0,
                bounds: screenloom_core::Rect::new(0, 0, 1920, 1080),
                is_primary: true,
                scale: 1.0,
            },
            screenloom_capture::MonitorInfo {
                index: 1,
                bounds: screenloom_core::Rect::new(1920, 0, 2560, 1440),
                is_primary: false,
                scale: 1.5,
            },
        ]
    }

    /// 用表单自带的访问器写一个文本字段。
    fn set_text_field(
        form: &Form,
        field: Field,
        cfg: &mut Config,
        val: &str,
    ) -> std::result::Result<(), String> {
        match &form.widget(field).expect("字段不在表单里").access {
            Access::Text { set, .. } => set(cfg, val),
            _ => panic!("不是文本字段: {field:?}"),
        }
    }

    fn text_of(form: &Form, field: Field, cfg: &Config) -> String {
        match &form.widget(field).expect("字段不在表单里").access {
            Access::Text { get, .. } => get(cfg),
            _ => panic!("不是文本字段: {field:?}"),
        }
    }

    #[test]
    fn every_field_has_a_widget() {
        let capture = capture();
        let record = record_form(&Config::default(), &fake_monitors());
        let mut seen = std::collections::HashSet::new();
        for form in [&capture, &record] {
            for w in form.widgets() {
                assert!(seen.insert(w.field), "字段重复: {:?}", w.field);
            }
        }
        for f in Field::ALL {
            assert!(seen.contains(f), "字段没有界面: {:?}", f);
        }
        assert_eq!(seen.len(), Field::ALL.len(), "有多余的控件");
        // 焦点顺序必须覆盖所有字段
        for form in [&capture, &record] {
            let items = form.focus_items();
            for w in form.widgets() {
                assert!(
                    items.contains(&FocusItem::Field(w.field)),
                    "字段不在焦点顺序里: {:?}",
                    w.field
                );
            }
        }
        assert_eq!(capture.widgets().count(), 13, "截图表单字段数量变了");
        assert_eq!(record.widgets().count(), 10, "录屏表单字段数量变了");
    }

    #[test]
    fn numeric_fields_are_range_checked() {
        let form = capture();
        let mut cfg = Config::default();

        // 合法值
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "240").is_ok());
        assert!(set_text_field(&form, Field::LongMax, &mut cfg, "0").is_ok());
        assert!(set_text_field(&form, Field::LongMax, &mut cfg, "8888").is_ok());
        assert!(set_text_field(&form, Field::AiTimeout, &mut cfg, "120").is_ok());

        // 非数字 / 负数
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "abc").is_err());
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "-5").is_err());
        assert!(set_text_field(&form, Field::AiTimeout, &mut cfg, "0").is_err());

        // 超大值必须被挡住（否则长截图会卡死）
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "999999").is_err());
        assert!(set_text_field(&form, Field::LongMax, &mut cfg, "999999999").is_err());
        assert!(set_text_field(&form, Field::AiTimeout, &mut cfg, "999999").is_err());

        // 边界内 / 边界外
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "8").is_ok());
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "4000").is_ok());
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "7").is_err());
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "4001").is_err());
    }

    #[test]
    fn invalid_value_keeps_original() {
        let form = capture();
        let mut cfg = Config::default();
        cfg.long_screenshot_search = 240;
        assert!(set_text_field(&form, Field::LongSearch, &mut cfg, "abc").is_err());
        assert_eq!(cfg.long_screenshot_search, 240, "非法值不能改坏原值");
    }

    #[test]
    fn hotkey_must_not_be_empty() {
        let form = capture();
        let mut cfg = Config::default();
        let err = set_text_field(&form, Field::Hotkey, &mut cfg, "   ").unwrap_err();
        assert!(err.contains("热键"));
        assert_eq!(cfg.hotkey, Config::default().hotkey);
    }

    #[test]
    fn combo_items_match_expected_order() {
        let form = capture();
        let copy = form.choice_items(Field::CopyFormat);
        assert_eq!(
            copy.iter().map(String::as_str).collect::<Vec<_>>(),
            COPY_FORMAT_ITEMS
        );
        let ai = form.choice_items(Field::AiProvider);
        assert_eq!(
            ai.iter().map(String::as_str).collect::<Vec<_>>(),
            AI_PROVIDER_ITEMS
        );
    }

    #[test]
    fn checkbox_widgets_carry_their_own_text() {
        // 复选框的文字画在方块右边，所以必须自带文字。
        for form in [capture(), record_form(&Config::default(), &fake_monitors())] {
            for w in form.widgets() {
                if w.kind == Kind::Check {
                    assert!(!w.text.is_empty(), "复选框必须自带文字: {:?}", w.field);
                }
            }
        }
    }

    #[test]
    fn form_roundtrip_preserves_config() {
        let mut cfg = Config::default();
        cfg.hotkey = "Ctrl+Alt+Z".to_string();
        cfg.hide_self = false;
        cfg.privacy_mode = true;
        cfg.toolbar_collapsed = true;
        cfg.copy_format = screenloom_core::CopyFormat::Dib;
        cfg.default_dir = Some(PathBuf::from("D:\\shots"));
        cfg.long_screenshot_max = 12_345;
        cfg.long_screenshot_search = 512;
        cfg.ai_provider = "deepseek".to_string();
        cfg.ai_model = "deepseek-chat".to_string();
        cfg.ai_endpoint = "https://example.com/v1".to_string();
        cfg.ai_api_key = "sk-test".to_string();
        cfg.ai_timeout = 30;
        cfg.record_fps = 60;
        cfg.record_bitrate = 8_500_000;
        cfg.record_countdown = 5;
        cfg.record_dir = Some(PathBuf::from("D:\\videos"));
        cfg.record_monitor = 1;
        cfg.record_cursor = false;
        cfg.record_show_hud = false;
        cfg.record_open_folder = false;

        for form in [capture_form(&cfg), record_form(&cfg, &fake_monitors())] {
            let mut out = cfg.clone();
            for w in form.widgets() {
                match &w.access {
                    Access::Text { get, set } => {
                        let text = get(&cfg);
                        set(&mut out, &text).expect("回写失败");
                    }
                    Access::Bool { get, set } => set(&mut out, get(&cfg)),
                    Access::Choice { get, set, items } => set(&mut out, get(&cfg, items.len())),
                }
            }
            assert_eq!(
                format!("{cfg:?}"),
                format!("{out:?}"),
                "表单往返后配置变了: {:?}",
                form.kind
            );
        }
    }

    #[test]
    fn bitrate_roundtrip() {
        let form = record_form(&Config::default(), &fake_monitors());
        let mut cfg = Config::default();
        cfg.record_bitrate = 8_000_000;
        assert_eq!(text_of(&form, Field::Bitrate, &cfg), "8.0");
        assert!(set_text_field(&form, Field::Bitrate, &mut cfg, "8.0").is_ok());
        assert_eq!(cfg.record_bitrate, 8_000_000);
        assert!(set_text_field(&form, Field::Bitrate, &mut cfg, "0.05").is_err());
        assert!(set_text_field(&form, Field::Bitrate, &mut cfg, "100").is_ok());
        assert_eq!(cfg.record_bitrate, 100_000_000);
    }

    #[test]
    fn monitor_choice_maps_auto_and_index() {
        let form = record_form(&Config::default(), &fake_monitors());
        let items = form.choice_items(Field::Monitor);
        // 自动 + 2 台显示器
        assert_eq!(items.len(), 3);
        let mut cfg = Config::default();
        cfg.record_monitor = -1;
        assert_eq!(choose(&form, Field::Monitor, &cfg), 0);
        cfg.record_monitor = 1;
        assert_eq!(choose(&form, Field::Monitor, &cfg), 2);
        // 下标越界要夹住
        cfg.record_monitor = 99;
        assert_eq!(choose(&form, Field::Monitor, &cfg), 2);
    }

    fn choose(form: &Form, field: Field, cfg: &Config) -> usize {
        match &form.widget(field).unwrap().access {
            Access::Choice { get, items, .. } => get(cfg, items.len()),
            _ => panic!("不是下拉字段"),
        }
    }

    #[test]
    fn layout_is_self_consistent() {
        // 布局的纵向推进必须和实际画出来的一致：
        // 面板底边不能越过下一个面板顶边，按钮不能越出客户区。
        let k = 1.25f32;
        let l = compute_layout(&capture(), k, i32::MAX / 2);
        for pair in l.panels.windows(2) {
            assert!(pair[0].rect.bottom() <= pair[1].rect.y, "面板之间重叠了");
        }
        assert!(l.save.bottom() <= l.client_h, "保存按钮超出客户区");
        assert!(l.cancel.bottom() <= l.client_h, "取消按钮超出客户区");
        assert!(l.content_h < l.client_h, "内容高度必须小于客户区");
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
        let a = compute_layout(&capture(), 1.0, i32::MAX / 2);
        let b = compute_layout(&capture(), 1.25, i32::MAX / 2);
        assert!(b.client_h > a.client_h, "高 DPI 下高度应变大");
        let ratio = b.client_h as f32 / a.client_h as f32;
        assert!((ratio - 1.25).abs() < 0.03, "缩放比例不对: {ratio}");
    }

    #[test]
    fn default_size_unchanged() {
        // 回归：内容不溢出时，窗口尺寸必须与 v2（无滚动）逐像素一致。
        assert_eq!(compute_layout(&capture(), 1.0, i32::MAX / 2).client_h, 652);
        assert_eq!(compute_layout(&capture(), 1.25, i32::MAX / 2).client_h, 829);
    }

    #[test]
    fn scroll_extent_zero_when_content_fits() {
        let l = compute_layout(&capture(), 1.25, i32::MAX / 2);
        assert_eq!(l.scroll_max(1.25), 0, "内容没溢出却出现了滚动范围");
    }

    #[test]
    fn window_size_includes_title_bar_and_border() {
        let l = compute_layout(&capture(), 1.0, i32::MAX / 2);
        let (win_w, win_h) = frame_adjusted(px(WIN_W, 1.0), l.client_h, 96);
        assert!(win_w >= px(WIN_W, 1.0));
        assert!(
            win_h > l.client_h,
            "窗口高度必须大于客户区高度（标题栏+边框没算进去）"
        );
    }

    #[test]
    fn fits_1080p_work_area_at_150_percent() {
        // 1080p 屏幕扣除任务栏后约 1040 物理像素；设置窗口（含标题栏）必须放得下。
        let l = compute_layout(&capture(), 1.5, i32::MAX / 2);
        let (_, wh) = frame_adjusted(px(WIN_W, 1.5), l.client_h, 144);
        assert!(
            wh <= 1040,
            "150% DPI 下设置窗口高 {wh}px，超出 1080p 工作区"
        );
    }

    #[test]
    fn browse_button_reserves_space_for_the_edit() {
        // 「默认保存目录」这一行：输入框必须给「浏览…」按钮让出位置。
        let l = compute_layout(&capture(), 1.0, i32::MAX / 2);
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
        let l = compute_layout(&capture(), 1.0, i32::MAX / 2);
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

    // ---- 滚动数学 ----

    #[test]
    fn scroll_clamps_to_max() {
        let mut s = Scroll::default();
        s.set_extent(300, 200);
        assert!(s.scroll_to(-50));
        assert_eq!(s.y, 0);
        assert!(s.scroll_to(999));
        assert_eq!(s.y, 300);
    }

    #[test]
    fn wheel_step_is_three_rows() {
        let mut s = Scroll::default();
        s.set_extent(1000, 500);
        assert!(s.wheel(-120, 1.0)); // 向下滚
        assert_eq!(s.y, 3 * ROW_H);
        assert!(s.wheel(120, 1.0));
        assert_eq!(s.y, 0);
        // 小 delta 先累加，不满一格不动
        assert!(!s.wheel(60, 1.0));
        assert_eq!(s.y, 0);
        assert!(s.wheel(60, 1.0));
    }

    #[test]
    fn ensure_visible_minimal_shift() {
        let mut s = Scroll::default();
        s.set_extent(2000, 500);
        // 已在视口内：不动
        assert!(!s.ensure_visible(Rect::new(0, 100, 10, 20), 1.0));
        assert_eq!(s.y, 0);
        // 下方超出：刚好露出来 + 8px 边距
        assert!(s.ensure_visible(Rect::new(0, 600, 10, 20), 1.0));
        assert_eq!(s.y, 620 + 8 - 500);
        // 上方超出
        assert!(s.ensure_visible(Rect::new(0, 0, 10, 20), 1.0));
        assert_eq!(s.y, 0);
    }

    #[test]
    fn thumb_geometry_within_track() {
        let mut s = Scroll::default();
        s.set_extent(1000, 500);
        let track = Rect::new(0, 0, 6, 500);
        let t = s.thumb(track, 1.0).expect("溢出时必须有滑块");
        assert!(t.h >= 24, "滑块太矮: {}", t.h);
        assert!(t.y >= track.y && t.bottom() <= track.bottom());
        // 滚到底：滑块贴底
        s.scroll_to(1000);
        let t = s.thumb(track, 1.0).unwrap();
        assert_eq!(t.bottom(), track.bottom());
    }

    #[test]
    fn no_thumb_when_content_fits() {
        let mut s = Scroll::default();
        s.set_extent(0, 500);
        assert!(s.thumb(Rect::new(0, 0, 6, 500), 1.0).is_none());
        assert!(!s.overflows());
    }
}
