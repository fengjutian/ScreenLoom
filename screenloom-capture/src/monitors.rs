//! 显示器枚举、虚拟屏幕范围与 DPI 换算。
//!
//! 所有矩形都是 **物理像素**、**虚拟屏幕坐标系**（原点 = 主显示器左上角）。

use screenloom_core::{Error, Point, Rect, Result, Size};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, GetWindowRect, IsWindow, MONITORINFOF_PRIMARY, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::MonitorInfo;

/// 让进程感知 Per-Monitor DPI v2。
///
/// 非 DPI 感知的进程里 `GetMonitorInfoW` / `GetSystemMetrics` 返回的是被系统
/// **虚拟化**过的逻辑坐标，无法满足「bounds 一律物理像素」的要求，所以这里
/// 尽力把进程切到 Per-Monitor-Aware v2。
///
/// 该调用每个进程只能生效一次：若 UI 层已经设置过，这里会失败并被忽略
/// （不会把已有设置改回去）。
pub fn ensure_dpi_awareness() {
    // SAFETY: 纯查询式设置，失败即忽略。
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 枚举所有显示器。
///
/// 顺序即 [`MonitorInfo::index`]，与 `CaptureTarget::Monitor(index)` 一致。
pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
    Ok(enumerate_monitors()?.0)
}

/// 虚拟屏幕左上角（物理像素，可能为负）。
pub fn virtual_screen_origin() -> Point {
    ensure_dpi_awareness();
    // SAFETY: GetSystemMetrics 对 SM_X/YVIRTUALSCREEN 无副作用。
    unsafe {
        Point::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    }
}

/// 虚拟屏幕总尺寸（物理像素）。
pub fn virtual_screen_size() -> Size {
    ensure_dpi_awareness();
    // SAFETY: GetSystemMetrics 对 SM_CX/CYVIRTUALSCREEN 无副作用。
    unsafe {
        Size::new(
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// 显示器句柄列表，顺序与 [`list_monitors`] 一致（WGC 的 `CreateForMonitor` 需要）。
pub(crate) fn monitor_handles() -> Result<Vec<HMONITOR>> {
    Ok(enumerate_monitors()?.1)
}

/// 窗口在虚拟屏幕坐标系中的物理像素矩形。
pub(crate) fn window_rect_physical(hwnd: HWND) -> Result<Rect> {
    // SAFETY: hwnd 由调用方保证有效；IsWindow 先做存在性检查。
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(Error::Capture(format!("窗口句柄无效: {:?}", hwnd.0)));
    }
    let mut r = RECT::default();
    // SAFETY: r 是有效的 RECT 指针。
    unsafe { GetWindowRect(hwnd, &mut r) }
        .map_err(|e| Error::Capture(format!("GetWindowRect 失败: {e}")))?;
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    if w <= 0 || h <= 0 {
        return Err(Error::Capture("窗口尺寸非法（可能已最小化）".into()));
    }
    Ok(Rect::new(r.left, r.top, w, h))
}

/// 一次枚举，同时拿到信息列表与句柄列表（保证两者下标一致）。
fn enumerate_monitors() -> Result<(Vec<MonitorInfo>, Vec<HMONITOR>)> {
    ensure_dpi_awareness();

    let mut ctx = MonitorEnumCtx {
        infos: Vec::new(),
        handles: Vec::new(),
    };
    // SAFETY: 回调只会往 ctx 里 push，ctx 在 EnumDisplayMonitors 返回前一直存活。
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_monitor_proc),
            LPARAM(&mut ctx as *mut MonitorEnumCtx as isize),
        )
    };
    if !ok.as_bool() {
        return Err(Error::Capture("EnumDisplayMonitors 失败".into()));
    }
    if ctx.infos.is_empty() {
        return Err(Error::Capture("未检测到任何显示器".into()));
    }
    Ok((ctx.infos, ctx.handles))
}

struct MonitorEnumCtx {
    infos: Vec<MonitorInfo>,
    handles: Vec<HMONITOR>,
}

/// `EnumDisplayMonitors` 回调。
unsafe extern "system" fn enum_monitor_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _lprc: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let ctx = &mut *(data.0 as *mut MonitorEnumCtx);

    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };
    // SAFETY: hmon 来自系统枚举，mi 指向有效内存。
    if !unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
        return TRUE;
    }

    let r = mi.rcMonitor;
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    if w <= 0 || h <= 0 {
        return TRUE;
    }

    let index = ctx.infos.len();
    ctx.infos.push(MonitorInfo {
        index,
        bounds: Rect::new(r.left, r.top, w, h),
        is_primary: mi.dwFlags & MONITORINFOF_PRIMARY != 0,
        scale: monitor_scale(hmon, w),
    });
    ctx.handles.push(hmon);
    TRUE
}

/// 显示器缩放比 = 物理宽 / 逻辑宽，逻辑宽由 `GetDpiForMonitor` 换算。
fn monitor_scale(hmon: HMONITOR, physical_w: i32) -> f32 {
    let mut dpi_x: u32 = 96;
    let mut dpi_y: u32 = 96;
    // SAFETY: dpi_x/dpi_y 是有效指针。
    let ok = unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.is_ok();
    if !ok || dpi_x == 0 || physical_w <= 0 {
        return 1.0;
    }
    // 逻辑宽 = 物理宽 * 96 / dpi_x；scale = 物理宽 / 逻辑宽。
    let logical_w = physical_w as f32 * 96.0 / dpi_x as f32;
    if logical_w <= 0.0 {
        return 1.0;
    }
    physical_w as f32 / logical_w
}
