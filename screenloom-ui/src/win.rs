//! Win32 底层封装：DPI 感知、窗口类注册、分层窗口像素提交。
//!
//! 这里只放"薄封装"，不含业务逻辑。

use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, POINT, SIZE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassInfoExW, RegisterClassExW, UpdateLayeredWindow, ULW_ALPHA, WNDCLASSEXW, WNDCLASS_STYLES,
};

use screenloom_core::{Error, Result};

/// UTF-16（带结尾 NUL）。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `GET_X_LPARAM`：取 **有符号** 的鼠标 x。
///
/// 跨显示器时 lParam 的坐标可能为负，直接截断成 u16 会变成巨大的正数。
#[inline]
pub fn get_x_lparam(lp: LPARAM) -> i32 {
    (lp.0 & 0xffff) as u16 as i16 as i32
}

/// `GET_Y_LPARAM`：取 **有符号** 的鼠标 y。
#[inline]
pub fn get_y_lparam(lp: LPARAM) -> i32 {
    ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32
}

/// 进程级 DPI 感知（Per-Monitor V2）。
///
/// 设为 DPI 感知后，浮窗的窗口坐标 == 物理像素，背景图可以 1:1 无缩放地贴上去。
/// 重复设置会失败，忽略即可。
pub fn ensure_dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 当前模块句柄。
pub fn module_handle() -> HINSTANCE {
    unsafe { HINSTANCE(GetModuleHandleW(None).unwrap_or_default().0) }
}

/// 注册窗口类；同名类只注册一次（重复 `RegisterClassW` 会失败）。
pub fn register_class(
    class_name: &str,
    proc: windows::Win32::UI::WindowsAndMessaging::WNDPROC,
) -> Result<()> {
    let name = wide(class_name);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }

    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        // 需要双击消息（双击选区中心确认）
        style: WNDCLASS_STYLES(windows::Win32::UI::WindowsAndMessaging::CS_DBLCLKS.0),
        lpfnWndProc: proc,
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        ..WNDCLASSEXW::default()
    };

    let atom = unsafe { RegisterClassExW(&wc) };
    if atom == 0 {
        // 并发注册时可能被别人抢先：类已存在就算成功
        let err = unsafe { GetLastError() };
        if err != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::Window(format!(
                "RegisterClassExW({class_name}) 失败: {err:?}"
            )));
        }
    }
    Ok(())
}

/// 分层窗口的像素源：内存 DC + 32bpp 自上而下 DIB section。
///
/// `UpdateLayeredWindow(ULW_ALPHA)` 要求源是 **预乘 alpha 的 BGRA**。
pub struct LayeredTarget {
    dc: HDC,
    dib: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: i32,
    h: i32,
}

impl LayeredTarget {
    pub fn new(w: i32, h: i32) -> Result<Self> {
        let (dc, dib, old, bits) = create_dib(w, h)?;
        Ok(LayeredTarget {
            dc,
            dib,
            old,
            bits,
            w,
            h,
        })
    }

    /// 改变尺寸（重建 DIB）。
    pub fn resize(&mut self, w: i32, h: i32) -> Result<()> {
        if w == self.w && h == self.h {
            return Ok(());
        }
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = DeleteDC(self.dc);
        }
        let (dc, dib, old, bits) = create_dib(w, h)?;
        self.dc = dc;
        self.dib = dib;
        self.old = old;
        self.bits = bits;
        self.w = w;
        self.h = h;
        Ok(())
    }

    /// 上传 BGRA 像素（长度必须是 w*h*4）。
    pub fn upload(&mut self, data: &[u8]) {
        debug_assert_eq!(data.len(), (self.w * self.h * 4) as usize);
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), self.bits, data.len());
        }
    }

    /// 把当前像素提交到分层窗口。`pos` 是窗口在屏幕上的位置（物理像素）。
    pub fn commit(&self, hwnd: HWND, pos: POINT) -> Result<()> {
        let size = SIZE {
            cx: self.w,
            cy: self.h,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = windows::Win32::Graphics::Gdi::BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&pos),
                Some(&size),
                Some(self.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
            .map_err(|e| Error::Window(format!("UpdateLayeredWindow 失败: {e}")))?;
        }
        Ok(())
    }
}

impl Drop for LayeredTarget {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

fn create_dib(w: i32, h: i32) -> Result<(HDC, HBITMAP, HGDIOBJ, *mut u8)> {
    let dc = unsafe { CreateCompatibleDC(None) };
    if dc.0.is_null() {
        return Err(Error::Window("CreateCompatibleDC 失败".into()));
    }
    let mut bmi = BITMAPINFO::default();
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = w;
    // 负高度 = 自上而下，正好和 GDI+ 的 PARGB 内存布局一致
    bmi.bmiHeader.biHeight = -h;
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB.0;

    let mut bits: *mut core::ffi::c_void = ptr::null_mut();
    let dib = unsafe { CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }
        .map_err(|e| Error::Window(format!("CreateDIBSection 失败: {e}")))?;
    if bits.is_null() {
        unsafe {
            let _ = DeleteDC(dc);
        }
        return Err(Error::Window("CreateDIBSection 失败".into()));
    }
    let old = unsafe { SelectObject(dc, HGDIOBJ(dib.0)) };
    Ok((dc, dib, old, bits as *mut u8))
}
