//! 贴图窗：置顶常驻、可拖动、可缩放、可关闭、不抢焦点。

use std::ptr;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::GdiPlus::{
    GdipCreateBitmapFromScan0, GdipDisposeImage, GdipDrawImageRectI, GpBitmap,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use screenloom_core::{Point, Result, Rgba, RgbaImage};

use crate::gdiplus::{self, Graphics};
use crate::win::{self, LayeredTarget};

const CLASS_NAME: &str = "ScreenloomPinWindow";

/// 贴图窗。
pub struct PinWindow {
    image: RgbaImage,
}

impl PinWindow {
    /// 创建贴图窗（还不显示）。
    pub fn new(image: RgbaImage) -> Result<PinWindow> {
        gdiplus::startup()?;
        win::ensure_dpi_aware();
        Ok(PinWindow { image })
    }

    /// 显示并进入消息循环（直到窗口被关闭）。
    pub fn show(self) {
        let mut state = Box::new(PinState {
            image: self.image,
            hwnd: HWND::default(),
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            drag: None,
            dirty: false,
        });

        win::register_class(CLASS_NAME, Some(pin_wnd_proc)).ok();
        let class = win::wide(CLASS_NAME);
        let title = win::wide("screenloom pin");
        let state_ptr = &mut *state as *mut PinState;

        let (sw, sh) = (unsafe { GetSystemMetrics(SM_CXSCREEN) }, unsafe {
            GetSystemMetrics(SM_CYSCREEN)
        });
        // 默认缩到屏幕的 1/2 以内
        let mut w = state.image.w as i32;
        let mut h = state.image.h as i32;
        let max_w = (sw as f32 * 0.6) as i32;
        let max_h = (sh as f32 * 0.6) as i32;
        if w > max_w || h > max_h {
            let f = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
            w = (w as f32 * f) as i32;
            h = (h as f32 * f) as i32;
        }
        state.w = w;
        state.h = h;
        state.x = (sw - w) / 2;
        state.y = (sh - h) / 2;

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                state.x,
                state.y,
                w,
                h,
                None,
                None,
                Some(win::module_handle()),
                Some(state_ptr as *const core::ffi::c_void),
            )
        };
        let Ok(hwnd) = hwnd else {
            return;
        };
        state.hwnd = hwnd;
        // 贴图窗不应被录进录屏/截图。
        let _ = unsafe {
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)
        };

        let mut target = match LayeredTarget::new(w, h) {
            Ok(t) => t,
            Err(_) => return,
        };

        // 渲染一帧
        if let Ok(bytes) = state.render_pargb() {
            target.upload(&bytes);
            let _ = target.commit(
                hwnd,
                POINT {
                    x: state.x,
                    y: state.y,
                },
            );
        }

        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                state.x,
                state.y,
                w,
                h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
            .ok();
        }

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
            if state.drag.is_some() || state.dirty {
                state.dirty = false;
                if let Ok(bytes) = state.render_pargb() {
                    target.upload(&bytes);
                    let _ = target.commit(
                        hwnd,
                        POINT {
                            x: state.x,
                            y: state.y,
                        },
                    );
                }
            }
        }
    }
}

struct PinState {
    image: RgbaImage,
    hwnd: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    drag: Option<PinDrag>,
    dirty: bool,
}

#[derive(Clone, Copy)]
enum PinDrag {
    Move { dx: i32, dy: i32 },
    Resize,
}

impl PinState {
    /// 关闭按钮 / 缩放手柄（客户区坐标）。
    fn close_rect(&self) -> screenloom_core::Rect {
        screenloom_core::Rect::new(self.w - 20, 0, 20, 20)
    }

    fn grip_rect(&self) -> screenloom_core::Rect {
        screenloom_core::Rect::new(self.w - 16, self.h - 16, 16, 16)
    }

    fn render_pargb(&self) -> Result<Vec<u8>> {
        let (w, h) = (self.w, self.h);
        if w <= 0 || h <= 0 {
            return Ok(Vec::new());
        }
        let bmp = gdiplus::Bitmap::new(w, h)?;
        {
            let mut d = bmp.lock()?;
            let n = ((d.Stride as usize) * h as usize).min((w * h * 4) as usize);
            unsafe {
                ptr::write_bytes(d.Scan0 as *mut u8, 0, n);
            }
            bmp.unlock(&mut d);
        }
        {
            let mut g = bmp.graphics()?;
            g.setup_quality();
            self.paint(&mut g)?;
        }
        let mut d = bmp.lock()?;
        let stride = d.Stride as usize;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let s = y * stride;
            let dst = y * w as usize * 4;
            let n = w as usize * 4;
            unsafe {
                ptr::copy_nonoverlapping(
                    (d.Scan0 as *const u8).add(s),
                    out.as_mut_ptr().add(dst),
                    n,
                );
            }
        }
        bmp.unlock(&mut d);
        Ok(out)
    }

    fn paint(&self, g: &mut Graphics) -> Result<()> {
        // 画图像（缩放到窗口大小）
        let (iw, ih) = (self.image.w as i32, self.image.h as i32);
        if iw > 0 && ih > 0 {
            let data = self.image.to_bgra_premultiplied();
            let mut src: *mut GpBitmap = ptr::null_mut();
            let st = unsafe {
                GdipCreateBitmapFromScan0(
                    iw,
                    ih,
                    iw * 4,
                    gdiplus::PIXEL_FORMAT_32BPP_PARGB,
                    Some(data.as_ptr()),
                    &mut src,
                )
            };
            if st.0 == 0 && !src.is_null() {
                unsafe {
                    GdipDrawImageRectI(
                        g.raw(),
                        src as *mut windows::Win32::Graphics::GdiPlus::GpImage,
                        0,
                        0,
                        self.w,
                        self.h,
                    );
                    GdipDisposeImage(src as *mut windows::Win32::Graphics::GdiPlus::GpImage);
                }
            }
        }

        // 关闭按钮
        let cr = self.close_rect();
        let c = gdiplus::argb(Rgba::rgb(0x00, 0x00, 0x00).with_alpha(0x99));
        g.fill_rect(cr.x as f32, cr.y as f32, cr.w as f32, cr.h as f32, c);
        let w = gdiplus::argb(Rgba::WHITE);
        g.draw_line(
            (cr.x + 6) as f32,
            (cr.y + 6) as f32,
            (cr.right() - 6) as f32,
            (cr.bottom() - 6) as f32,
            w,
            1.4,
        );
        g.draw_line(
            (cr.right() - 6) as f32,
            (cr.y + 6) as f32,
            (cr.x + 6) as f32,
            (cr.bottom() - 6) as f32,
            w,
            1.4,
        );

        // 缩放手柄
        let gr = self.grip_rect();
        let gc = gdiplus::argb(Rgba::rgb(0xFF, 0xFF, 0xFF).with_alpha(0xCC));
        for i in 0..3 {
            g.draw_line(
                (gr.right() - 4 - i * 4) as f32,
                (gr.bottom() - 3) as f32,
                (gr.right() - 3) as f32,
                (gr.bottom() - 4 - i * 4) as f32,
                gc,
                1.2,
            );
        }
        Ok(())
    }

    fn on_mouse_down(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        if self.close_rect().contains(c) {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
            return;
        }
        if self.grip_rect().contains(c) {
            self.drag = Some(PinDrag::Resize);
            return;
        }
        self.drag = Some(PinDrag::Move {
            dx: c.x - self.x,
            dy: c.y - self.y,
        });
    }

    fn on_mouse_move(&mut self, lp: LPARAM) {
        let c = Point::new(win::get_x_lparam(lp), win::get_y_lparam(lp));
        match self.drag {
            Some(PinDrag::Move { dx, dy }) => {
                self.x = c.x - dx;
                self.y = c.y - dy;
                unsafe {
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        self.x,
                        self.y,
                        self.w,
                        self.h,
                        SWP_NOACTIVATE,
                    );
                }
                self.dirty = true;
            }
            Some(PinDrag::Resize) => {
                let nw = (c.x - self.x).max(32);
                let nh = (c.y - self.y).max(32);
                if nw != self.w || nh != self.h {
                    self.w = nw;
                    self.h = nh;
                    unsafe {
                        let _ = SetWindowPos(
                            self.hwnd,
                            Some(HWND_TOPMOST),
                            self.x,
                            self.y,
                            self.w,
                            self.h,
                            SWP_NOACTIVATE,
                        );
                    }
                    self.dirty = true;
                }
            }
            None => {}
        }
    }

    fn on_mouse_up(&mut self) {
        self.drag = None;
    }
}

unsafe extern "system" fn pin_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut PinState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PinState;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;

    match msg {
        WM_LBUTTONDOWN => {
            st.on_mouse_down(lp);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            st.on_mouse_up();
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            st.on_mouse_move(lp);
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
    use std::time::Duration;

    /// 贴图窗能创建、能渲染、能被关闭。
    #[test]
    fn pin_window_creates_and_closes() {
        let mut img = RgbaImage::new(120, 90);
        for y in 0..90 {
            for x in 0..120 {
                img.set_pixel(x, y, Rgba::rgb(x as u8, y as u8, 0x20));
            }
        }
        let handle = std::thread::spawn(move || {
            PinWindow::new(img).expect("PinWindow::new 失败").show();
        });

        let cls: Vec<u16> = CLASS_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut hwnd = HWND::default();
        for _ in 0..100 {
            if let Ok(w) = unsafe { FindWindowW(PCWSTR(cls.as_ptr()), PCWSTR::null()) } {
                hwnd = w;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!hwnd.0.is_null(), "贴图窗没有创建出来");
        // 不抢焦点
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            assert_ne!(ex & WS_EX_NOACTIVATE.0, 0, "贴图窗应当不抢焦点");
            assert_ne!(ex & WS_EX_TOPMOST.0, 0, "贴图窗应当置顶");
        }
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        handle.join().expect("贴图窗线程异常退出");
    }
}
