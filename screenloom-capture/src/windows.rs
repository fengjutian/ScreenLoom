//! 顶层窗口枚举 + 「窗口从屏幕捕获中排除」（WDA / `WDA_EXCLUDEFROMCAPTURE`）。
//!
//! 对应微信 4.x 截图工具的「窗口选择器」（`wx::graphics::window_filter_t`）与
//! 「按窗口排除抓屏」（`GrabWorker::SetExcludeHwnds`）。
//!
//! **坐标约定**：本模块暴露的所有 [`Rect`] 一律是 **物理像素**，且位于
//! **虚拟屏幕坐标系**（原点 = 主显示器左上角，负坐标表示主屏左侧/上方的显示器）。
//! `GetWindowRect` / `DwmGetWindowAttribute` 返回的本来就是虚拟屏物理像素坐标，
//! 因此**不要再**减去 `SM_XVIRTUALSCREEN`（那是边界宽度而非原点，做偏移会让
//! 窗口全部落进桌面空洞）。
//!
//! **平台限制（微软文档）**：
//! - `WDA_EXCLUDEFROMCAPTURE` 只有 **窗口拥有者进程自己** 能设置，对其他进程的
//!   窗口调用会失败。
//! - Windows 会在显示器配置 / DPI 变化 / 独占全屏切换时 **静默清除** 该属性，
//!   健壮用法是每隔约 1 秒重新设置一次（由上层负责）。

use std::panic::{catch_unwind, AssertUnwindSafe};

use screenloom_core::Rect;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowDisplayAffinity, GetWindowLongW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetWindowDisplayAffinity, GWL_EXSTYLE, WINDOW_DISPLAY_AFFINITY,
    WS_EX_TOOLWINDOW,
};

/// `WDA_EXCLUDEFROMCAPTURE` 的常量值（= 0x11）。
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

/// 窗口标题缓冲区上限（UTF-16 字符数）。
const TITLE_BUF_LEN: usize = 512;

/// 窗口类名缓冲区上限（UTF-16 字符数；`GetClassNameW` 上限为 256）。
const CLASS_BUF_LEN: usize = 256;

/// 一个可枚举的顶层窗口。
#[derive(Clone, Debug)]
pub struct WindowInfo {
    /// 窗口句柄
    pub hwnd: HWND,
    /// 拥有该窗口的进程 id
    pub pid: u32,
    /// 窗口标题（可能为空串）
    pub title: String,
    /// 窗口类名
    pub class_name: String,
    /// 窗口在**虚拟屏幕坐标系**中的**物理像素**矩形（窗口边框矩形）
    pub bounds: Rect,
    /// 是否是当前前台窗口
    pub is_foreground: bool,
}

/// 枚举可作为截图目标的顶层窗口。
///
/// 已过滤：不可见 / 已最小化 / 无标题 / 零面积 / 工具窗口(WS_EX_TOOLWINDOW)。
pub fn list_capture_windows() -> Vec<WindowInfo> {
    enumerate_windows(Filter::Capture)
}

/// 枚举**当前进程自己创建**的所有顶层窗口（含无标题、含工具窗口），不做可见性过滤。
///
/// 用于「隐私模式」：把这些窗口从屏幕捕获中排除。
pub fn list_own_windows() -> Vec<WindowInfo> {
    enumerate_windows(Filter::Own)
}

/// 设置 / 取消「该窗口从屏幕捕获中排除」（WDA_EXCLUDEFROMCAPTURE）。
///
/// **重要限制（微软文档）**：只有**窗口拥有者进程自己**能设置该属性，
/// 对其他进程的窗口调用会失败。返回是否成功。
///
/// 注意：Windows 会在显示器配置 / DPI 变化 / 独占全屏切换时**静默清除**该属性，
/// 健壮用法是每隔约 1 秒重新设置一次（上层负责）。
pub fn set_window_capture_excluded(hwnd: HWND, excluded: bool) -> bool {
    let affinity = WINDOW_DISPLAY_AFFINITY(if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        0 // WDA_NONE
    });
    // SAFETY: hwnd 由调用方保证有效；失败时返回 false。
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) }.is_ok()
}

/// 查询窗口当前的「从捕获中排除」属性；查询失败返回 false。
pub fn is_window_capture_excluded(hwnd: HWND) -> bool {
    let mut affinity: u32 = 0;
    // SAFETY: affinity 是有效指针。
    match unsafe { GetWindowDisplayAffinity(hwnd, &mut affinity) } {
        Ok(()) => affinity == WDA_EXCLUDEFROMCAPTURE,
        Err(_) => false,
    }
}

/// 枚举模式。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    /// 只保留「适合作为截图目标」的窗口。
    Capture,
    /// 只保留当前进程自己创建的窗口，不过滤可见性 / 标题。
    Own,
}

/// 枚举上下文（在 FFI 回调期间一直存活）。
struct EnumCtx {
    filter: Filter,
    foreground: HWND,
    own_pid: u32,
    out: Vec<WindowInfo>,
}

fn enumerate_windows(filter: Filter) -> Vec<WindowInfo> {
    // 保证进程是 Per-Monitor DPI 感知，否则窗口坐标会被系统虚拟化成逻辑坐标。
    crate::monitors::ensure_dpi_awareness();

    // SAFETY: GetForegroundWindow 是纯查询。
    let foreground = unsafe { GetForegroundWindow() };
    let mut ctx = EnumCtx {
        filter,
        foreground,
        own_pid: std::process::id(),
        out: Vec::new(),
    };
    // SAFETY: 回调只往 ctx 里 push；ctx 在 EnumWindows 返回前一直存活（栈上）。
    let _ = unsafe {
        EnumWindows(
            Some(enum_window_proc),
            LPARAM(&mut ctx as *mut EnumCtx as isize),
        )
    };
    ctx.out
}

/// `EnumWindows` 回调。
unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut EnumCtx);
    // 回调里 panic 会跨 FFI 边界 unwind（UB），这里兜一层。
    let _ = catch_unwind(AssertUnwindSafe(|| collect_window(hwnd, ctx)));
    TRUE
}

/// 收集单个窗口信息（按模式过滤）。
fn collect_window(hwnd: HWND, ctx: &mut EnumCtx) {
    if hwnd.0.is_null() {
        return;
    }

    let mut pid: u32 = 0;
    // SAFETY: pid 是有效指针。
    let _thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return;
    }

    match ctx.filter {
        Filter::Own => {
            if pid != ctx.own_pid {
                return;
            }
        }
        Filter::Capture => {
            // SAFETY: hwnd 来自系统枚举，有效。
            if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
                return;
            }
            if unsafe { IsIconic(hwnd) }.as_bool() {
                return;
            }
            if is_tool_window(hwnd) {
                return;
            }
        }
    }

    let title = window_title(hwnd);
    let bounds = window_bounds(hwnd);

    let bounds = match ctx.filter {
        Filter::Capture => {
            if title.trim().is_empty() {
                return;
            }
            let Some(bounds) = bounds else { return };
            if bounds.w <= 0 || bounds.h <= 0 {
                return;
            }
            bounds
        }
        // 自有窗口不做尺寸过滤：隐藏窗口可能没有有效矩形。
        Filter::Own => bounds.unwrap_or(Rect::new(0, 0, 0, 0)),
    };

    ctx.out.push(WindowInfo {
        hwnd,
        pid,
        title,
        class_name: window_class_name(hwnd),
        bounds,
        is_foreground: hwnd == ctx.foreground,
    });
}

/// 是否是工具窗口（`WS_EX_TOOLWINDOW`）。
fn is_tool_window(hwnd: HWND) -> bool {
    // SAFETY: hwnd 来自系统枚举，有效。
    let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    ex_style & WS_EX_TOOLWINDOW.0 != 0
}

/// 窗口标题（可能为空串）。
fn window_title(hwnd: HWND) -> String {
    // SAFETY: hwnd 来自系统枚举，有效。
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let cap = (len as usize).min(TITLE_BUF_LEN);
    let mut buf = vec![0u16; cap + 1];
    // SAFETY: buf 是有效切片；返回的 n 是**字符数**（不含终止符）。
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    let n = (n as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string()
}

/// 窗口类名。
fn window_class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; CLASS_BUF_LEN];
    // SAFETY: buf 是有效切片；返回的 n 是**字符数**（不含终止符）。
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    let n = (n as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string()
}

/// 窗口在虚拟屏幕坐标系中的物理像素矩形。
///
/// 优先用 DWM 的 `DWMWA_EXTENDED_FRAME_BOUNDS`（去掉阴影的真实可视边框），
/// 失败时回退到 `GetWindowRect`（含阴影的窗口边框矩形）。
fn window_bounds(hwnd: HWND) -> Option<Rect> {
    let mut dwm_rect = RECT::default();
    // SAFETY: dwm_rect 是有效内存。
    let dwm_ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm_rect as *mut RECT as *mut std::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if dwm_ok.is_ok() {
        if let Some(r) = rect_to_rect(dwm_rect) {
            return Some(r);
        }
    }

    let mut rect = RECT::default();
    // SAFETY: rect 是有效内存。
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }
    rect_to_rect(rect)
}

/// `RECT` → [`Rect`]（虚拟屏物理像素，负坐标合法）。宽高非正时返回 `None`。
fn rect_to_rect(rect: RECT) -> Option<Rect> {
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some(Rect::new(rect.left, rect.top, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP,
    };

    #[test]
    fn list_capture_windows_has_positive_size_bounds() {
        let list = list_capture_windows();
        for w in &list {
            assert!(
                w.bounds.w > 0 && w.bounds.h > 0,
                "窗口 {:?} 的 bounds 非法: {:?}",
                w.title,
                w.bounds
            );
            assert!(!w.title.trim().is_empty(), "窗口不应没有标题");
            assert_ne!(w.pid, 0, "窗口应有进程 id");
        }
    }

    #[test]
    fn list_own_windows_belongs_to_current_process() {
        let pid = process::id();
        for w in list_own_windows() {
            assert_eq!(w.pid, pid, "自有窗口的 pid 应等于当前进程 id: {w:?}");
        }
    }

    #[test]
    fn exclude_from_capture_roundtrip_on_own_window() {
        let hwnd = create_test_window();
        assert!(
            !is_window_capture_excluded(hwnd),
            "新建窗口初始不应处于排除状态"
        );
        assert!(
            set_window_capture_excluded(hwnd, true),
            "SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE) 应成功"
        );
        assert!(
            is_window_capture_excluded(hwnd),
            "设置排除后查询应返回 true"
        );
        assert!(
            set_window_capture_excluded(hwnd, false),
            "SetWindowDisplayAffinity(WDA_NONE) 应成功"
        );
        assert!(
            !is_window_capture_excluded(hwnd),
            "取消排除后查询应返回 false"
        );
        // SAFETY: 句柄由本测试创建，仍然有效。
        unsafe { DestroyWindow(hwnd) }.ok();
    }

    /// 创建一个属于当前进程的隐藏顶层窗口（用于 WDA 自检）。
    fn create_test_window() -> HWND {
        // SAFETY: 参数合法；返回的句柄由调用方负责销毁。
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
            .expect("创建测试窗口失败")
        }
    }
}
