//! 按**窗口标题**抓取窗口画面并存成 PNG —— 专用于视觉留证。
//!
//! 用法：
//! ```text
//! cargo run --manifest-path ...\Cargo.toml -p screenloom-app --example shot_window -- "screenloom 设置" out.png
//! ```
//!
//! 实现：`EnumWindows` 找标题匹配的窗口 → `GetWindowRect` 取外框 →
//! 用 `PrintWindow(PW_RENDERFULLCONTENT)` 把窗口画进内存 DC → 存 PNG。
//!
//! `PrintWindow` 对分层窗口（DWM 合成过）也能抓到，所以工具栏浮窗/设置窗口都适用。

use std::path::PathBuf;

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
};

use screenloom_core::RgbaImage;

/// `PW_RENDERFULLCONTENT`（windows crate 没导出这个常量，硬编码 2）。
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

/// 找到标题包含 `needle` 的第一个可见顶层窗口。
fn find_window(needle: &str) -> Option<HWND> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let needle = &*(lparam.0 as *const String);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return BOOL(1);
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let n = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if title.contains(needle.as_str()) {
            // 记在全局里：EnumWindows 只能通过 lparam 传值
            FOUND.with(|f| {
                let mut slot = f.borrow_mut();
                if slot.is_none() {
                    *slot = Some(hwnd);
                }
            });
            return BOOL(0); // 停止枚举
        }
        BOOL(1)
    }

    thread_local! {
        static FOUND: std::cell::RefCell<Option<HWND>> = const { std::cell::RefCell::new(None) };
    }

    let needle = needle.to_string();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&needle as *const String as isize));
    }
    FOUND.with(|f| *f.borrow())
}

fn main() {
    // 抓窗前必须先设 DPI 感知：否则 GetWindowRect 返回的是虚拟化坐标，
    // 画出来的位图尺寸和窗口对不上，右侧会被裁掉。
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    let mut args = std::env::args().skip(1);
    let title = args.next().unwrap_or_else(|| "screenloom 设置".to_string());
    let out = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\github\screenloom\e2e_out\window.png"));

    let Some(hwnd) = find_window(&title) else {
        eprintln!("没找到标题含 {title:?} 的可见窗口");
        std::process::exit(1);
    };
    println!("窗口句柄: {:?}", hwnd.0);

    let mut rc = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut rc).is_err() {
            eprintln!("GetWindowRect 失败");
            std::process::exit(1);
        }
    }
    let w = (rc.right - rc.left).max(1);
    let h = (rc.bottom - rc.top).max(1);
    println!("窗口外框: ({}, {}) {}x{}", rc.left, rc.top, w, h);

    let mut img = RgbaImage::new(w as u32, h as u32);
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize =
            std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w;
        bmi.bmiHeader.biHeight = -h; // 负数 = 自上而下
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0;
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("CreateDIBSection 失败");
        let old = SelectObject(mem, HGDIOBJ(bmp.0));

        // 先用 PrintWindow 抓窗口自身内容
        let ok = PrintWindow(hwnd, mem, PW_RENDERFULLCONTENT).as_bool();
        println!("PrintWindow: {}", if ok { "ok" } else { "失败" });

        // 打印失败时退化：从屏幕 DC 上按外框抠
        if !ok {
            let _ = windows::Win32::Graphics::Gdi::BitBlt(
                mem,
                0,
                0,
                w,
                h,
                Some(screen),
                rc.left,
                rc.top,
                SRCCOPY,
            );
        }

        for y in 0..h as usize {
            for x in 0..w as usize {
                let p = (bits as *const u8).add((y * w as usize + x) * 4);
                img.set_pixel(
                    x as u32,
                    y as u32,
                    screenloom_core::Rgba {
                        r: *p.add(2),
                        g: *p.add(1),
                        b: *p,
                        a: 255,
                    },
                );
            }
        }

        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
    }

    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let png = img.encode_png().expect("PNG 编码失败");
    std::fs::write(&out, &png).expect("写 PNG 失败");
    println!("已保存: {} ({} 字节)", out.display(), png.len());
}
