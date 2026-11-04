//! 窗口选择器：列出当前屏幕上**可捕获**的顶层窗口，让用户挑一个。
//!
//! 两种用途（[`PickerMode`]）：
//!
//! * [`PickerMode::PickTarget`] —— 单选：选一个窗口作为截图目标，
//!   上层拿到后把选区跳到那个窗口。
//! * [`PickerMode::PickExclusions`] —— 多选：勾若干窗口，
//!   上层存进配置，之后截图时把这些窗口从截图里去掉。
//!
//! 弹窗是**模态**的：在自己的消息循环里跑，窗口关掉之前 [`run`] 不返回。
//! 退出时必须抽干线程消息队列里残留的 `WM_QUIT` —— 否则调用方
//! （浮窗 / 托盘）的下一个消息循环会立刻拿到它，整个程序静默退出。
//! 这个坑本项目已经踩过一次，见 `screenloom-ui/src/overlay.rs`。
//!
//! UI 全部用原生控件（`LISTBOX` + `BUTTON`），字体走系统默认，
//! 不引入字体管理。

use screenloom_capture::WindowInfo;
use screenloom_core::Rect;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;

/// 窗口类名。
const CLASS_NAME: &str = "ScreenloomWindowPicker";

/// 「确定」按钮 id。
const ID_OK: usize = 1001;
/// 「取消」按钮 id。
const ID_CANCEL: usize = 1002;
/// 列表框 id。
const ID_LIST: usize = 1003;

/// 逻辑宽度（96 DPI 基准的物理像素）。
const WIN_W: i32 = 460;
/// 逻辑高度。
const WIN_H: i32 = 520;
/// 客户区四周留白。
const PAD: i32 = 8;
/// 底部按钮高度。
const BTN_H: i32 = 28;
/// 底部按钮宽度。
const BTN_W: i32 = 84;

/// 窗口选择器的用途。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerMode {
    /// 选一个窗口作为截图目标（单选）。
    PickTarget,
    /// 勾选若干窗口，从截图里排除（多选）。
    PickExclusions,
}

/// 弹出窗口选择器。
///
/// 返回 `Some(vec)` = 用户点了「确定」；`None` = 用户点了「取消」或直接关窗。
/// `PickTarget` 模式下 vec 至多 1 个元素；`PickExclusions` 模式下是勾选的若干个。
///
/// 实现要求：
/// - 弹窗必须是**模态**的：在自己的消息循环里跑，关闭前不返回。
/// - 退出时**必须抽干残留的 `WM_QUIT`**（用 `PeekMessageW` 只抽 WM_QUIT），
///   否则会把调用方（浮窗 / 托盘）的消息循环一起干掉。
///   本项目已有这个坑，见 `screenloom-ui/src/overlay.rs` 的 `drain_quit_message`。
pub fn run(mode: PickerMode) -> Option<Vec<WindowInfo>> {
    // DPI 感知必须在建窗之前设好，否则控件尺寸算不准、文字发虚。
    screenloom_capture::ensure_dpi_awareness();

    let items = sort_windows(screenloom_capture::list_capture_windows());

    // 状态放在堆上，生命周期覆盖整个消息循环；窗口过程通过
    // GWLP_USERDATA 拿到它。
    let mut state = Box::new(State {
        hwnd: HWND::default(),
        list: HWND::default(),
        mode,
        items,
        result: None,
    });
    let state_ptr = &mut *state as *mut State;

    if !register_class() {
        return None;
    }
    let class = wide(CLASS_NAME);
    let title = wide("选择窗口");

    let (x, y, ww, wh, cw, ch) = placement();

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP | WS_BORDER,
            x,
            y,
            ww,
            wh,
            None,
            None,
            Some(module_handle()),
            Some(state_ptr as *const core::ffi::c_void),
        )
    }
    .ok()?;
    state.hwnd = hwnd;

    build_controls(hwnd, &mut state, cw, ch);

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }

    // ---- 模态：自己的消息循环，窗口关掉之前不返回 ----
    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            // WM_QUIT：确定 / 取消时-post 的。
            break;
        }
        // 让 Esc / Enter / Tab 表现得像标准对话框
        // （Enter → 默认按钮「确定」，Esc → IDCANCEL）。
        if unsafe { IsDialogMessageW(hwnd, &msg) }.as_bool() {
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // 窗口被外部销毁了（调用方关的）——别把线程卡死在这儿。
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            break;
        }
    }

    // 抽干队列里残留的 WM_QUIT（只抽 WM_QUIT）。
    drain_quit_message();

    // 窗口销毁放在这里，不在窗口过程里：窗口过程里销毁窗口会重入窗口过程
    // （WM_DESTROY → 同一个 State 的第二个 &mut），是 UB。
    if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        let _ = unsafe { DestroyWindow(hwnd) };
    }

    state.result.take()
}

// ---------------------------------------------------------------- 状态

/// 弹窗的状态。
struct State {
    /// 弹窗本身。
    hwnd: HWND,
    /// 列表框。
    list: HWND,
    /// 当前模式（单选 / 多选）。
    mode: PickerMode,
    /// 列表里的每一项对应的窗口（顺序与列表一致）。
    items: Vec<WindowInfo>,
    /// 用户点了「确定」时的结果（点「取消」时是 None）。
    result: Option<Vec<WindowInfo>>,
}

// ---------------------------------------------------------------- 窗口过程

unsafe extern "system" fn picker_wnd_proc(
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
        WM_COMMAND => {
            let id = (wp.0 & 0xFFFF) as usize;
            let code = ((wp.0 >> 16) & 0xFFFF) as u32;
            if id == ID_OK {
                confirm(st);
                return LRESULT(0);
            }
            // IDCANCEL 是 IsDialogMessage 处理 Esc 时发过来的。
            if id == ID_CANCEL || id == IDCANCEL.0 as usize {
                cancel(st);
                return LRESULT(0);
            }
            // 单选模式：双击某一项 = 确定。
            if id == ID_LIST && code == LBN_DBLCLK && st.mode == PickerMode::PickTarget {
                confirm(st);
                return LRESULT(0);
            }
        }

        WM_CLOSE => {
            cancel(st);
            return LRESULT(0);
        }

        WM_KEYDOWN => {
            // 焦点在弹窗自己身上时的兜底（焦点在列表框 / 按钮上时走不到这里）。
            if wp.0 as u16 == VK_ESCAPE.0 as u16 {
                cancel(st);
                return LRESULT(0);
            }
            if wp.0 as u16 == VK_RETURN.0 as u16 {
                confirm(st);
                return LRESULT(0);
            }
        }

        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 「确定」：收集勾选结果，结束模态循环。
fn confirm(st: &mut State) {
    st.result = Some(collect_selection(st));
    finish();
}

/// 「取消」：不产生结果，结束模态循环。
fn cancel(st: &mut State) {
    st.result = None;
    finish();
}

/// 结束模态循环：post 一条 WM_QUIT，`run()` 里的 `GetMessageW` 会返回 0。
fn finish() {
    unsafe {
        PostQuitMessage(0);
    }
}

/// 抽掉线程消息队列里残留的 `WM_QUIT`（只抽 WM_QUIT，别把别的消息一起吞掉）。
fn drain_quit_message() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {}
    }
}

// ---------------------------------------------------------------- 控件

/// 建控件：列表框 + 「确定」「取消」两个按钮。
fn build_controls(hwnd: HWND, st: &mut State, cw: i32, ch: i32) {
    let multi = if st.mode == PickerMode::PickExclusions {
        LBS_MULTIPLESEL as u32
    } else {
        0
    };
    let list_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | WS_BORDER.0
            | WS_TABSTOP.0
            | LBS_NOTIFY as u32
            | multi,
    );
    let list_class = wide("LISTBOX");
    let list = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(list_class.as_ptr()),
            PCWSTR::null(),
            list_style,
            PAD,
            PAD,
            cw - PAD * 2,
            ch - PAD * 2 - BTN_H - PAD,
            Some(hwnd),
            Some(HMENU(ID_LIST as *mut core::ffi::c_void)),
            Some(module_handle()),
            None,
        )
    };
    let Ok(list) = list else {
        return;
    };
    st.list = list;

    // 填列表（顺序 = sort_windows 的顺序）。
    for item in &st.items {
        let text = wide(&item_label(item));
        unsafe {
            let _ = SendMessageW(list, LB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
        }
    }

    // 底部按钮：「取消」在右，「确定」在它左边（默认按钮）。
    let by = ch - PAD - BTN_H;
    let bx = cw - PAD - BTN_W;
    make_button(hwnd, ID_CANCEL, "取消", bx, by, BTN_W, BTN_H, false);
    make_button(hwnd, ID_OK, "确定", bx - BTN_W - PAD, by, BTN_W, BTN_H, true);
}

/// 建一个原生按钮。
fn make_button(
    parent: HWND,
    id: usize,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    default: bool,
) -> HWND {
    let class = wide("BUTTON");
    let title = wide(text);
    let style = WINDOW_STYLE(
        WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | if default { BS_DEFPUSHBUTTON as u32 } else { 0 },
    );
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default()
}

// ---------------------------------------------------------------- 选择结果

/// 读出当前被选中的窗口。
fn collect_selection(st: &mut State) -> Vec<WindowInfo> {
    let picked = selected_indices(st.list, st.mode);
    // 把值**移**出去（不要求 WindowInfo: Clone）。
    let mut items = std::mem::take(&mut st.items);
    let mut out = Vec::with_capacity(picked.len());
    // 倒序删除，保证返回顺序与列表一致。
    for i in picked.into_iter().rev() {
        if i < items.len() {
            out.push(items.remove(i));
        }
    }
    out.reverse();
    out
}

/// 列表里被选中的下标。
fn selected_indices(list: HWND, mode: PickerMode) -> Vec<usize> {
    let mut out = Vec::new();
    if list.0.is_null() {
        return out;
    }
    unsafe {
        let count = SendMessageW(list, LB_GETCOUNT, None, None).0 as i32;
        if count <= 0 {
            return out;
        }
        match mode {
            PickerMode::PickTarget => {
                let cur = SendMessageW(list, LB_GETCURSEL, None, None).0 as i32;
                if cur >= 0 && cur < count {
                    out.push(cur as usize);
                }
            }
            PickerMode::PickExclusions => {
                for i in 0..count {
                    let sel = SendMessageW(list, LB_GETSEL, Some(WPARAM(i as usize)), None).0;
                    if sel != 0 {
                        out.push(i as usize);
                    }
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- 纯逻辑

/// 排序：面积从大到小；标题为空的排最后。
///
/// 用户一般先找最大的那个窗口，所以按面积降序。
fn sort_windows(mut v: Vec<WindowInfo>) -> Vec<WindowInfo> {
    v.sort_by(|a, b| {
        let ea = a.title.trim().is_empty();
        let eb = b.title.trim().is_empty();
        match (ea, eb) {
            (true, true) => area(b).cmp(&area(a)),
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            (false, false) => area(b).cmp(&area(a)),
        }
    });
    v
}

/// 窗口面积（物理像素²）。
fn area(w: &WindowInfo) -> i64 {
    let ww = w.bounds.w.max(0) as i64;
    let hh = w.bounds.h.max(0) as i64;
    ww * hh
}

/// 列表里一项的文字：`标题 · 类名 (宽x高)`。
fn item_label(w: &WindowInfo) -> String {
    let title = if w.title.trim().is_empty() {
        "(无标题)"
    } else {
        w.title.as_str()
    };
    let class = w.class_name.as_str();
    let ww = w.bounds.w;
    let hh = w.bounds.h;
    format!("{title} · {class} ({ww}x{hh})")
}

// ---------------------------------------------------------------- 摆放

/// 弹窗的摆放：居中于主显示器。
///
/// 返回 `(x, y, 整窗宽, 整窗高, 客户区宽, 客户区高)`。
fn placement() -> (i32, i32, i32, i32, i32, i32) {
    let dpi = unsafe { GetDpiForSystem() };
    let k = dpi as f32 / 96.0;
    let cw = (WIN_W as f32 * k).round() as i32;
    let ch = (WIN_H as f32 * k).round() as i32;

    // 客户区 → 整窗。WS_POPUP|WS_BORDER 只有边框，没有标题栏。
    let mut r = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    let _ = unsafe {
        AdjustWindowRectExForDpi(
            &mut r,
            WS_POPUP | WS_BORDER,
            false,
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            dpi,
        )
    };
    let ww = r.right - r.left;
    let wh = r.bottom - r.top;

    let screen = primary_bounds();
    let x = screen.x + (screen.w - ww) / 2;
    let y = screen.y + (screen.h - wh) / 2;
    (x, y, ww, wh, cw, ch)
}

/// 主显示器在虚拟屏坐标系里的矩形（物理像素）。
fn primary_bounds() -> Rect {
    if let Ok(mons) = screenloom_capture::list_monitors() {
        if let Some(m) = mons.iter().find(|m| m.is_primary) {
            return m.bounds;
        }
    }
    // 兜底：主显示器在虚拟屏坐标里的原点恒为 (0,0)。
    unsafe {
        Rect::new(
            0,
            0,
            GetSystemMetrics(SM_CXSCREEN),
            GetSystemMetrics(SM_CYSCREEN),
        )
    }
}

// ---------------------------------------------------------------- 基础工具

/// UTF-16（带结尾 NUL）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 进程实例句柄。
fn module_handle() -> HINSTANCE {
    HINSTANCE(
        unsafe { GetModuleHandleW(None) }
            .unwrap_or_default()
            .0,
    )
}

/// 注册窗口类（已经注册过就直接返回）。
fn register_class() -> bool {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }.is_ok() {
        return true;
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(picker_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        // 系统窗口底色（COLOR_WINDOW + 1）
        hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut core::ffi::c_void),
        ..WNDCLASSEXW::default()
    };
    unsafe { RegisterClassExW(&wc) != 0 }
}

// ---------------------------------------------------------------- 测试

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(title: &str, class: &str, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            hwnd: HWND(std::ptr::null_mut()),
            pid: 0,
            title: title.to_string(),
            class_name: class.to_string(),
            bounds: Rect::new(0, 0, w, h),
            is_foreground: false,
        }
    }

    #[test]
    fn picker_mode_variants_are_comparable() {
        assert_eq!(PickerMode::PickTarget, PickerMode::PickTarget);
        assert_eq!(PickerMode::PickExclusions, PickerMode::PickExclusions);
        assert_ne!(PickerMode::PickTarget, PickerMode::PickExclusions);
    }

    #[test]
    fn sort_windows_orders_by_area_desc() {
        let v = vec![
            mk("小", "A", 10, 10),   // 100
            mk("大", "B", 100, 100), // 10000
            mk("中", "C", 50, 50),   // 2500
        ];
        let s = sort_windows(v);
        let titles: Vec<&str> = s.iter().map(|w| w.title.as_str()).collect();
        assert_eq!(titles, vec!["大", "中", "小"]);
    }

    #[test]
    fn sort_windows_puts_untitled_last() {
        let v = vec![
            mk("", "A", 100, 100), // 无标题，但面积最大
            mk("有标题", "B", 10, 10),
        ];
        let s = sort_windows(v);
        assert_eq!(s[0].title, "有标题");
        assert_eq!(s[1].title, "");
    }

    #[test]
    fn item_label_uses_placeholder_for_empty_title() {
        let w = mk("", "Chrome_WidgetWin_1", 800, 600);
        assert_eq!(item_label(&w), "(无标题) · Chrome_WidgetWin_1 (800x600)");
        let w2 = mk("记事本", "Notepad", 100, 50);
        assert_eq!(item_label(&w2), "记事本 · Notepad (100x50)");
    }

    #[test]
    fn empty_list_is_allowed() {
        // 没有可捕获窗口时也要能正常显示并让用户取消。
        let s = sort_windows(Vec::new());
        assert!(s.is_empty());
    }
}
