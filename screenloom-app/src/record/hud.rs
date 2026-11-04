//! REC HUD：置顶小条，显示计时/状态，提供暂停与停止。
//!
//! 设计约束：
//! - `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`：不抢焦点、不进 Alt+Tab；
//! - `WDA_EXCLUDEFROMCAPTURE`：自己不被录进画面；
//! - 只读 [`SharedStats`] 的原子量，不直接碰 worker/session。

use std::sync::Arc;

use screenloom_core::{Error, Result};
use screenloom_ui::gdiplus::Graphics;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateRoundRectRgn, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint,
    SetWindowRgn, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::controller::{HUD_CMD_PAUSE, HUD_CMD_STOP, WM_RECORDER_HUD};
use super::stats::{Phase, SharedStats};

const CLASS_NAME: &str = "ScreenloomRecorderHud";
const TIMER_ID: usize = 1;
const HUD_W: i32 = 286;
const HUD_H: i32 = 64;

struct HudContext {
    stats: Arc<SharedStats>,
    controller: HWND,
    /// 创建时的 DPI 缩放（物理/逻辑）
    scale: f32,
    /// 暂停按钮 / 停止按钮的客户区矩形（物理像素）
    pause_rect: RECT,
    stop_rect: RECT,
}

/// REC 浮窗句柄。
pub struct RecorderHud {
    hwnd: HWND,
}

impl RecorderHud {
    /// 创建并显示 HUD。`controller` 接收按钮命令（[`WM_RECORDER_HUD`]）。
    pub fn show(stats: Arc<SharedStats>, controller: HWND) -> Result<RecorderHud> {
        register_class()?;
        let dpi = unsafe { GetDpiForSystem() };
        let k = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let w = (HUD_W as f32 * k).round() as i32;
        let h = (HUD_H as f32 * k).round() as i32;
        let (x, y) = bottom_right(w, h);

        let ctx = Box::new(HudContext {
            stats,
            controller,
            scale: k,
            pause_rect: RECT {
                left: w - (100.0 * k) as i32,
                top: (12.0 * k) as i32,
                right: w - (56.0 * k) as i32,
                bottom: h - (12.0 * k) as i32,
            },
            stop_rect: RECT {
                left: w - (48.0 * k) as i32,
                top: (12.0 * k) as i32,
                right: w - (8.0 * k) as i32,
                bottom: h - (12.0 * k) as i32,
            },
        });

        let class = wide(CLASS_NAME);
        let title = wide("screenloom 录屏");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(module_handle()),
                Some(Box::into_raw(ctx) as *const core::ffi::c_void),
            )
        }
        .map_err(|e| Error::Window(format!("创建录制 HUD 失败: {e}")))?;

        // 圆角裁剪 + 不录到自己
        let rgn = unsafe { CreateRoundRectRgn(0, 0, w + 1, h + 1, (14.0 * k) as i32, (14.0 * k) as i32) };
        if !rgn.0.is_null() {
            unsafe {
                let _ = SetWindowRgn(hwnd, Some(rgn), true);
            }
        }
        let _ = screenloom_capture::set_window_capture_excluded(hwnd, true);
        unsafe {
            let _ = SetTimer(Some(hwnd), TIMER_ID, 250, None);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }

        Ok(RecorderHud { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for RecorderHud {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.hwnd)).as_bool() {
                let _ = KillTimer(Some(self.hwnd), TIMER_ID);
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

fn bottom_right(w: i32, h: i32) -> (i32, i32) {
    // 用鼠标所在显示器的 work area，避免盖住任务栏。
    let pt = POINT::default();
    let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetMonitorInfoW(mon, &mut mi) }.as_bool();
    let work = if ok {
        mi.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        }
    };
    (work.right - w - 24, work.bottom - h - 24)
}

fn register_class() -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }.is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(hud_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册录制 HUD 窗口类失败".into()));
    }
    Ok(())
}

unsafe extern "system" fn hud_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ctx_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut HudContext;
    match msg {
        WM_NCCREATE => {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            let ctx = cs.lpCreateParams as *mut HudContext;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx as isize);
            }
            return LRESULT(1);
        }
        WM_NCDESTROY => {
            if !ctx_ptr.is_null() {
                unsafe {
                    drop(Box::from_raw(ctx_ptr));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
            }
            return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_TIMER if wp.0 == TIMER_ID => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !ctx_ptr.is_null() {
                let ctx = unsafe { &*ctx_ptr };
                paint(hdc, ctx);
            }
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            if ctx_ptr.is_null() {
                return LRESULT(0);
            }
            let ctx = unsafe { &*ctx_ptr };
            let p = point_from_lparam(lp);
            if !in_rect(p, ctx.pause_rect) && !in_rect(p, ctx.stop_rect) {
                unsafe {
                    let _ = ReleaseCapture();
                    let _ = SendMessageW(
                        hwnd,
                        WM_NCLBUTTONDOWN,
                        Some(WPARAM(HTCAPTION as usize)),
                        Some(LPARAM(0)),
                    );
                }
            }
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            if ctx_ptr.is_null() {
                return LRESULT(0);
            }
            let ctx = unsafe { &*ctx_ptr };
            let p = point_from_lparam(lp);
            let cmd = if in_rect(p, ctx.pause_rect) {
                Some(HUD_CMD_PAUSE)
            } else if in_rect(p, ctx.stop_rect) {
                Some(HUD_CMD_STOP)
            } else {
                None
            };
            if let Some(id) = cmd {
                unsafe {
                    let _ = PostMessageW(Some(ctx.controller), WM_RECORDER_HUD, WPARAM(id), LPARAM(0));
                }
            }
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn paint(hdc: windows::Win32::Graphics::Gdi::HDC, ctx: &HudContext) {
    let mut g = match Graphics::from_hdc(hdc) {
        Ok(g) => g,
        Err(_) => return,
    };
    let k = ctx.scale;
    let w = (HUD_W as f32 * k).round();
    let h = (HUD_H as f32 * k).round();

    g.fill_round_rect(0.0, 0.0, w, h, 14.0 * k, 0xF2_1E_1E_1E);
    g.draw_round_rect(0.0, 0.0, w, h, 14.0 * k, 0x66_00_00_00, 1.0);

    let phase = ctx.stats.phase();
    let (dot, label) = match phase {
        Phase::Countdown => (0xFF_F1_C4_0F, "准备录制".to_string()),
        Phase::Recording => (0xFF_E7_4C_3C, format!("● {}", ctx.stats.elapsed_text())),
        Phase::Paused => (0xFF_F1_C4_0F, format!("Ⅱ 已暂停 {}", ctx.stats.elapsed_text())),
        Phase::Stopping => (0xFF_98_A1_AC, "正在保存…".to_string()),
        Phase::Finished => (0xFF_2E_CC_71, "已保存".to_string()),
        Phase::Failed => (0xFF_E7_4C_3C, "录制失败".to_string()),
        Phase::Idle => (0xFF_98_A1_AC, "空闲".to_string()),
    };

    g.fill_ellipse(18.0 * k, 24.0 * k, 12.0 * k, 12.0 * k, dot);
    g.draw_text(40.0 * k, 20.0 * k, &label, 15.0 * k, 0xFF_E7_EA_EE, true);

    // 暂停 / 继续
    let pause_label = if phase == Phase::Paused { "继续" } else { "暂停" };
    button(&mut g, ctx.pause_rect, pause_label, k);
    button(&mut g, ctx.stop_rect, "停止", k);
}

fn button(g: &mut Graphics, r: RECT, label: &str, k: f32) {
    let (x, y, w, h) = (
        r.left as f32,
        r.top as f32,
        (r.right - r.left) as f32,
        (r.bottom - r.top) as f32,
    );
    g.fill_round_rect(x, y, w, h, 6.0 * k, 0xFF_2B_30_36);
    let (tw, th) = g.measure_text(label, 13.0 * k, false);
    g.draw_text(
        x + (w - tw) / 2.0,
        y + (h - th) / 2.0,
        label,
        13.0 * k,
        0xFF_E7_EA_EE,
        false,
    );
}

fn point_from_lparam(lp: LPARAM) -> POINT {
    POINT {
        x: (lp.0 & 0xffff) as u16 as i16 as i32,
        y: ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32,
    }
}

fn in_rect(p: POINT, r: RECT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

fn module_handle() -> windows::Win32::Foundation::HINSTANCE {
    unsafe { windows::Win32::Foundation::HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
