//! GDI 屏幕 DC 坐标原点的判别实验。
//!
//! 现象：GDI 兜底抓屏得到的是**全零图像**（PNG 里 alpha=0，r/g/b 全 0）。
//! 怀疑是「屏幕 DC 的原点」理解错了：
//!   - 假设 A：屏幕 DC 原点 = **虚拟屏幕包围盒**左上角 → 源坐标 = 虚拟坐标 - 包围盒左上
//!   - 假设 B：屏幕 DC 原点 = **主显示器**左上角 → 源坐标 = 虚拟坐标（主屏在 0,0 时即原样）
//!
//! 这个实验把几个候选源坐标各 BitBlt 一小块，统计非黑像素个数，直接判定哪个假设成立。

use std::ptr;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// 在屏幕 DC 上用给定源坐标抠一块 200x200，返回非黑像素数与首像素颜色。
unsafe fn probe(screen: HDC, sx: i32, sy: i32) -> (usize, [u8; 4]) {
    let mem = CreateCompatibleDC(Some(screen));
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize =
        std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = 200;
    bmi.bmiHeader.biHeight = -200;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0;
    let mut bits: *mut std::ffi::c_void = ptr::null_mut();
    let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
    let old = SelectObject(mem, HGDIOBJ(bmp.0));
    let ok = BitBlt(mem, 0, 0, 200, 200, Some(screen), sx, sy, SRCCOPY).is_ok();
    let mut nonzero = 0usize;
    let mut first = [0u8; 4];
    for i in 0..200usize * 200usize {
        let p = (bits as *const u8).add(i * 4);
        let b = *p;
        let g = *p.add(1);
        let r = *p.add(2);
        if r != 0 || g != 0 || b != 0 {
            nonzero += 1;
        }
        if i == 0 {
            first = [b, g, r, *p.add(3)];
        }
    }
    SelectObject(mem, old);
    let _ = DeleteObject(HGDIOBJ(bmp.0));
    let _ = DeleteDC(mem);
    if !ok {
        println!("    BitBlt 失败");
    }
    (nonzero, first)
}

fn main() {
    unsafe {
        // 与 capture crate 一致：进程级 DPI 感知 V2
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );

        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        println!("虚拟屏幕(感知后): origin=({vx},{vy}) size={vw}x{vh}");

        let screen = GetDC(None);
        println!("屏幕 DC: {:?}", screen.0);

        let candidates: Vec<(&str, i32, i32)> = vec![
            ("DC(0,0)      主屏左上", 0, 0),
            ("DC(892,252)  主屏内部", 892, 252),
            ("DC(-1038,-1080) #2左上", -1038, -1080),
            ("DC(-500,-500) #2内部", -500, -500),
            ("DC(1000,-500) #1内部", 1000, -500),
            ("DC(892,-1440) #1左上", 892, -1440),
            ("DC(2000,500) 主屏右侧空隙", 2000, 500),
            ("DC(1500,-700) #1内部2", 1500, -700),
        ];

        for (name, sx, sy) in candidates {
            let (nz, first) = probe(screen, sx, sy);
            println!("  {name:42} 非黑 {nz:>6}/40000  首像素 BGRA={:?}", first);
        }

        let _ = ReleaseDC(None, screen);
        let _ = RECT::default();
    }
}
