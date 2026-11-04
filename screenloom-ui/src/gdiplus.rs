//! GDI+ 封装：进程级启动、位图/画布 RAII、绘图基元。
//!
//! 全部走 GDI+ 的 flat API（`Win32_Graphics_GdiPlus`），不引入 COM 依赖。

use std::cell::RefCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Once;

use windows::core::PCWSTR;
// 注意：不能用 `use ...::GdiPlus::*` —— GDI+ 里有个 `Ok` 状态常量，会把 `Result::Ok` 顶掉。
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus::{
    BitmapData, CombineModeReplace, DashCapRound, FillModeAlternate, FontStyleBold,
    FontStyleRegular, GdipBitmapLockBits, GdipBitmapUnlockBits, GdipCreateBitmapFromScan0,
    GdipCreateFont, GdipCreateFontFamilyFromName, GdipCreateFromHDC, GdipCreatePen1,
    GdipCreateSolidFill, GdipCreateStringFormat, GdipDeleteBrush, GdipDeleteFont,
    GdipDeleteGraphics, GdipDeletePen, GdipDeleteStringFormat, GdipDisposeImage, GdipDrawArc,
    GdipDrawEllipse, GdipDrawLine, GdipDrawRectangle, GdipDrawString, GdipFillEllipse,
    GdipFillPolygon, GdipFillRectangle, GdipGetGenericFontFamilySansSerif,
    GdipGetImageGraphicsContext, GdipMeasureString, GdipResetClip, GdipSetClipRect,
    GdipSetPageUnit, GdipSetPenLineCap197819, GdipSetPixelOffsetMode, GdipSetSmoothingMode,
    GdipSetStringFormatHotkeyPrefix, GdipSetTextRenderingHint, GdiplusStartup, GdiplusStartupInput,
    GdiplusStartupOutput, GpBitmap, GpBrush, GpFont, GpFontFamily, GpGraphics, GpImage, GpPen,
    GpSolidFill, GpStringFormat, ImageLockModeRead, LineCapRound, PixelOffsetModeHalf, PointF,
    RectF, SmoothingModeAntiAlias, Status, TextRenderingHintAntiAliasGridFit, UnitPixel,
};

use screenloom_core::{Error, Result};

/// `PixelFormat32bppPARGB`（**flat API** 取值）。
///
/// 注意：GDI+ flat API 用的 PixelFormat 与 C++ 版 `Gdiplus::PixelFormat` **不一样**，
/// 必须带 `Alpha | PAlpha | GDI` 标志位：
/// `11 | (32 << 8) | 0x40000 | 0x80000 | 0x20000 = 0x000E200B`。
/// 用错会得到 `Status=2 (InvalidParameter)`。
///
/// 内存布局（小端）= B, G, R, A，且 **已预乘**，正好是 `UpdateLayeredWindow` 需要的格式。
pub const PIXEL_FORMAT_32BPP_PARGB: i32 = 0x000E_200B;

/// 中文字体族名。Windows 中文系统字体多为 TTC，用族名取，不要自己找 TTF 文件。
const FONT_FAMILY_NAME: &str = "Microsoft YaHei";

static STARTUP: Once = Once::new();
static STARTUP_OK: AtomicBool = AtomicBool::new(false);
static TOKEN: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// 字体族缓存（进程内创建一次）。
    static FONT_FAMILY: RefCell<Option<*mut GpFontFamily>> = const { RefCell::new(None) };
}

/// GDI+ 进程级初始化。**必须**先调用一次，否则 `GdipGetImageGraphicsContext` 会失败。
pub fn startup() -> Result<()> {
    STARTUP.call_once(|| unsafe {
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            DebugEventCallback: 0,
            SuppressBackgroundThread: windows::core::BOOL(0),
            SuppressExternalCodecs: windows::core::BOOL(0),
        };
        let mut token: usize = 0;
        let mut output = GdiplusStartupOutput::default();
        let st = GdiplusStartup(&mut token, &input, &mut output);
        if st.0 == 0 {
            TOKEN.store(token, Ordering::Relaxed);
            STARTUP_OK.store(true, Ordering::Relaxed);
        }
    });
    if STARTUP_OK.load(Ordering::Relaxed) {
        Ok(())
    } else {
        Err(Error::Window("GdiplusStartup 失败".into()))
    }
}

fn check(st: Status, what: &str) -> Result<()> {
    if st.0 == 0 {
        Ok(())
    } else {
        Err(Error::Window(format!("{what} 失败 (Status={})", st.0)))
    }
}

/// 颜色 → GDI+ ARGB（0xAARRGGBB）。
pub fn argb(c: screenloom_core::Rgba) -> u32 {
    ((c.a as u32) << 24) | ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// ARGB → 颜色。
pub fn unargb(v: u32) -> screenloom_core::Rgba {
    screenloom_core::Rgba {
        r: ((v >> 16) & 0xFF) as u8,
        g: ((v >> 8) & 0xFF) as u8,
        b: (v & 0xFF) as u8,
        a: ((v >> 24) & 0xFF) as u8,
    }
}

/// 32bpp PARGB 位图。
///
/// 像素内存由我们自己分配并传给 `GdipCreateBitmapFromScan0`
/// （传 NULL 会让 GDI+ 返回 `Status=2 / OutOfMemory`）。
pub struct Bitmap {
    ptr: *mut GpBitmap,
    /// scan0 的后备内存：GDI+ 不接管所有权，必须活到位图销毁为止。
    _buf: Vec<u8>,
}

impl Bitmap {
    /// 新建一张 32bpp PARGB 位图（内容为零）。
    pub fn new(w: i32, h: i32) -> Result<Self> {
        startup()?;
        if w <= 0 || h <= 0 {
            return Err(Error::Window(format!("位图尺寸非法: {w}x{h}")));
        }
        let stride = w as usize * 4;
        let buf = vec![0u8; stride * h as usize];
        let mut p: *mut GpBitmap = ptr::null_mut();
        let st = unsafe {
            GdipCreateBitmapFromScan0(
                w,
                h,
                stride as i32,
                PIXEL_FORMAT_32BPP_PARGB,
                Some(buf.as_ptr()),
                &mut p,
            )
        };
        check(st, "GdipCreateBitmapFromScan0")?;
        if p.is_null() {
            return Err(Error::Window("GdipCreateBitmapFromScan0 返回空指针".into()));
        }
        Ok(Bitmap { ptr: p, _buf: buf })
    }

    /// 锁定像素区（PARGB，小端内存序 = B,G,R,A）。
    pub fn lock(&self) -> Result<BitmapData> {
        let mut d = BitmapData::default();
        let st = unsafe {
            GdipBitmapLockBits(
                self.ptr,
                ptr::null(),
                ImageLockModeRead.0 as u32,
                PIXEL_FORMAT_32BPP_PARGB,
                &mut d,
            )
        };
        check(st, "GdipBitmapLockBits")?;
        Ok(d)
    }

    /// 解锁像素区。
    pub fn unlock(&self, d: &mut BitmapData) {
        unsafe {
            GdipBitmapUnlockBits(self.ptr, d);
        }
    }

    /// 取绘图上下文。
    pub fn graphics(&self) -> Result<Graphics> {
        let mut g: *mut GpGraphics = ptr::null_mut();
        let st = unsafe { GdipGetImageGraphicsContext(self.ptr as *mut GpImage, &mut g) };
        check(st, "GdipGetImageGraphicsContext")?;
        if g.is_null() {
            return Err(Error::Window(
                "GdipGetImageGraphicsContext 返回空指针".into(),
            ));
        }
        Ok(Graphics { ptr: g })
    }
}

impl Graphics {
    /// 直接在**设备上下文**上绘图（窗口 DC / 内存 DC）。
    ///
    /// 自绘窗口（如设置窗口）用它把矢量图形直接画到窗口上，
    /// 不用先渲染到位图再搬运。
    pub fn from_hdc(hdc: HDC) -> Result<Self> {
        startup()?;
        let mut g: *mut GpGraphics = ptr::null_mut();
        let st = unsafe { GdipCreateFromHDC(hdc, &mut g) };
        check(st, "GdipCreateFromHDC")?;
        if g.is_null() {
            return Err(Error::Window("GdipCreateFromHDC 返回空指针".into()));
        }
        Ok(Graphics { ptr: g })
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.ptr as *mut GpImage);
        }
    }
}

/// 绘图上下文。
pub struct Graphics {
    ptr: *mut GpGraphics,
}

impl Graphics {
    /// 原始指针（供需要直接调用 GDI+ flat API 的地方用）。
    pub(crate) fn raw(&self) -> *mut GpGraphics {
        self.ptr
    }

    /// 设置抗锯齿 + 文本渲染质量。
    pub fn setup_quality(&mut self) {
        unsafe {
            GdipSetPageUnit(self.ptr, UnitPixel);
            GdipSetSmoothingMode(self.ptr, SmoothingModeAntiAlias);
            GdipSetPixelOffsetMode(self.ptr, PixelOffsetModeHalf);
            GdipSetTextRenderingHint(self.ptr, TextRenderingHintAntiAliasGridFit);
        }
    }

    pub fn clear_clip(&mut self) {
        unsafe {
            GdipResetClip(self.ptr);
        }
    }

    pub fn clip_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        unsafe {
            GdipSetClipRect(self.ptr, x, y, w, h, CombineModeReplace);
        }
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let brush = solid_fill(color);
        unsafe {
            GdipFillRectangle(self.ptr, brush as *mut GpBrush, x, y, w, h);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
    }

    /// 圆角矩形填充。
    ///
    /// 用「两条矩形 + 四个椭圆」拼出来 —— GDI+ 的 `GpPath` 路径 API 在这套 flat
    /// 绑定下画不出东西（整块失效），而矩形/椭圆/直线都是好的。
    pub fn fill_round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let d = (r * 2.0).min(w).min(h);
        let rr = d / 2.0;
        self.fill_rect(x, y + rr, w, h - d, color);
        self.fill_rect(x + rr, y, w - d, h, color);
        self.fill_ellipse(x, y, d, d, color);
        self.fill_ellipse(x + w - d, y, d, d, color);
        self.fill_ellipse(x, y + h - d, d, d, color);
        self.fill_ellipse(x + w - d, y + h - d, d, d, color);
    }

    /// 圆角矩形描边（四条直边 + 四段圆弧）。
    // 绘图原语天然参数多（x/y/w/h/r/color/width），拆成结构体反而更难用
    #[allow(clippy::too_many_arguments)]
    pub fn draw_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        color: u32,
        width: f32,
    ) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let d = (r * 2.0).min(w).min(h);
        let rr = d / 2.0;
        self.draw_line(x + rr, y, x + w - rr, y, color, width);
        self.draw_line(x + rr, y + h, x + w - rr, y + h, color, width);
        self.draw_line(x, y + rr, x, y + h - rr, color, width);
        self.draw_line(x + w, y + rr, x + w, y + h - rr, color, width);
        self.draw_arc(x, y, d, d, 180.0, 90.0, color, width);
        self.draw_arc(x + w - d, y, d, d, 270.0, 90.0, color, width);
        self.draw_arc(x + w - d, y + h - d, d, d, 0.0, 90.0, color, width);
        self.draw_arc(x, y + h - d, d, d, 90.0, 90.0, color, width);
    }

    /// 椭圆弧（角度单位：度，顺时针，0° 在 3 点钟方向）。
    #[allow(clippy::too_many_arguments)]
    fn draw_arc(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        start: f32,
        sweep: f32,
        color: u32,
        width: f32,
    ) {
        let pen = make_pen(color, width);
        unsafe {
            GdipDrawArc(self.ptr, pen, x, y, w, h, start, sweep);
            GdipDeletePen(pen);
        }
    }

    pub fn draw_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32, width: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let pen = make_pen(color, width);
        unsafe {
            GdipDrawRectangle(self.ptr, pen, x, y, w, h);
            GdipDeletePen(pen);
        }
    }

    pub fn fill_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let brush = solid_fill(color);
        unsafe {
            GdipFillEllipse(self.ptr, brush as *mut GpBrush, x, y, w, h);
            GdipDeleteBrush(brush as *mut GpBrush);
        }
    }

    pub fn draw_ellipse(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32, width: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let pen = make_pen(color, width);
        unsafe {
            GdipDrawEllipse(self.ptr, pen, x, y, w, h);
            GdipDeletePen(pen);
        }
    }

    pub fn draw_line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, color: u32, width: f32) {
        let pen = make_pen(color, width);
        unsafe {
            GdipSetPenLineCap197819(pen, LineCapRound, LineCapRound, DashCapRound);
            GdipDrawLine(self.ptr, pen, x1, y1, x2, y2);
            GdipDeletePen(pen);
        }
    }

    pub fn fill_polygon(&mut self, pts: &[PointF], color: u32) {
        if pts.len() < 3 {
            return;
        }
        let brush = solid_fill(color);
        unsafe {
            GdipFillPolygon(
                self.ptr,
                brush as *mut GpBrush,
                pts.as_ptr(),
                pts.len() as i32,
                FillModeAlternate,
            );
            GdipDeleteBrush(brush as *mut GpBrush);
        }
    }

    /// 画文本（`x`,`y` 为左上角）。返回绘制尺寸 (w, h)。
    pub fn draw_text(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        size: f32,
        color: u32,
        bold: bool,
    ) -> (f32, f32) {
        let Some((font, fmt)) = make_font(size, bold) else {
            return (0.0, 0.0);
        };
        let s = wide(text);
        let layout = RectF {
            X: x,
            Y: y,
            Width: 8192.0,
            Height: size * 2.0 + 8.0,
        };
        let mut bounds = RectF::default();
        let mut fitted: i32 = 0;
        let mut lines: i32 = 0;
        unsafe {
            GdipMeasureString(
                self.ptr,
                PCWSTR(s.as_ptr()),
                -1,
                font,
                &layout,
                fmt,
                &mut bounds,
                &mut fitted,
                &mut lines,
            );
            let brush = solid_fill(color);
            GdipDrawString(
                self.ptr,
                PCWSTR(s.as_ptr()),
                -1,
                font,
                &layout,
                fmt,
                brush as *mut GpBrush,
            );
            GdipDeleteBrush(brush as *mut GpBrush);
            GdipDeleteStringFormat(fmt);
            GdipDeleteFont(font);
        }
        (bounds.Width, bounds.Height)
    }

    /// 量文本尺寸。
    pub fn measure_text(&mut self, text: &str, size: f32, bold: bool) -> (f32, f32) {
        let Some((font, fmt)) = make_font(size, bold) else {
            return (0.0, 0.0);
        };
        let s = wide(text);
        let layout = RectF {
            X: 0.0,
            Y: 0.0,
            Width: 8192.0,
            Height: size * 2.0 + 8.0,
        };
        let mut bounds = RectF::default();
        let mut fitted: i32 = 0;
        let mut lines: i32 = 0;
        unsafe {
            GdipMeasureString(
                self.ptr,
                PCWSTR(s.as_ptr()),
                -1,
                font,
                &layout,
                fmt,
                &mut bounds,
                &mut fitted,
                &mut lines,
            );
            GdipDeleteStringFormat(fmt);
            GdipDeleteFont(font);
        }
        (bounds.Width, bounds.Height)
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.ptr);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn solid_fill(color: u32) -> *mut GpSolidFill {
    let mut b: *mut GpSolidFill = ptr::null_mut();
    unsafe {
        GdipCreateSolidFill(color, &mut b);
    }
    b
}

fn make_pen(color: u32, width: f32) -> *mut GpPen {
    let mut p: *mut GpPen = ptr::null_mut();
    unsafe {
        GdipCreatePen1(color, width.max(0.1), UnitPixel, &mut p);
    }
    p
}

/// 取字体族（找不到就用系统无衬线体）。
fn font_family() -> *mut GpFontFamily {
    FONT_FAMILY.with(|c| {
        let mut slot = c.borrow_mut();
        if slot.is_none() {
            let mut ff: *mut GpFontFamily = ptr::null_mut();
            let name = wide(FONT_FAMILY_NAME);
            let st = unsafe {
                GdipCreateFontFamilyFromName(PCWSTR(name.as_ptr()), ptr::null_mut(), &mut ff)
            };
            if st.0 != 0 || ff.is_null() {
                unsafe {
                    GdipGetGenericFontFamilySansSerif(&mut ff);
                }
            }
            *slot = Some(ff);
        }
        slot.unwrap_or(ptr::null_mut())
    })
}

/// 创建字体 + 字符串格式。
fn make_font(size: f32, bold: bool) -> Option<(*mut GpFont, *mut GpStringFormat)> {
    startup().ok()?;
    let fam = font_family();
    if fam.is_null() {
        return None;
    }
    let style = if bold {
        FontStyleBold.0
    } else {
        FontStyleRegular.0
    };
    let mut font: *mut GpFont = ptr::null_mut();
    unsafe {
        if GdipCreateFont(fam, size, style, UnitPixel, &mut font).0 != 0 {
            return None;
        }
    }
    let mut fmt: *mut GpStringFormat = ptr::null_mut();
    unsafe {
        if GdipCreateStringFormat(0, 0, &mut fmt).0 != 0 {
            GdipDeleteFont(font);
            return None;
        }
        GdipSetStringFormatHotkeyPrefix(fmt, 0);
    }
    Some((font, fmt))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: i32 = 64;
    const H: i32 = 64;

    /// 画一帧并返回原始像素。
    fn draw(f: impl FnOnce(&mut Graphics)) -> Vec<u8> {
        let bmp = Bitmap::new(W, H).expect("位图创建失败");
        {
            let mut d = bmp.lock().unwrap();
            unsafe {
                ptr::write_bytes(d.Scan0 as *mut u8, 0, (d.Stride as usize) * H as usize);
            }
            bmp.unlock(&mut d);
        }
        {
            let mut g = bmp.graphics().unwrap();
            g.setup_quality();
            f(&mut g);
        }
        let mut d = bmp.lock().unwrap();
        let stride = d.Stride as usize;
        let mut out = vec![0u8; (W * H * 4) as usize];
        for y in 0..H as usize {
            unsafe {
                ptr::copy_nonoverlapping(
                    (d.Scan0 as *const u8).add(y * stride),
                    out.as_mut_ptr().add(y * W as usize * 4),
                    W as usize * 4,
                );
            }
        }
        bmp.unlock(&mut d);
        out
    }

    #[inline]
    fn px(buf: &[u8], x: i32, y: i32) -> [u8; 4] {
        let i = ((y as usize) * W as usize + x as usize) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn plain_fill_rect_writes_pixels() {
        let buf = draw(|g| g.fill_rect(10.0, 10.0, 20.0, 20.0, 0xFF_00_00_FF));
        // PARGB 内存序 = B,G,R,A
        assert_eq!(px(&buf, 20, 20), [0xFF, 0x00, 0x00, 0xFF]);
        assert_eq!(px(&buf, 2, 2), [0, 0, 0, 0]);
    }

    /// 圆角矩形填充必须真的画出来（路径版曾整块失效）。
    #[test]
    fn round_rect_fill_writes_pixels() {
        let buf = draw(|g| g.fill_round_rect(10.0, 10.0, 40.0, 40.0, 8.0, 0xFF_20_20_20));
        // 中心应被填充
        assert_eq!(
            px(&buf, 30, 30),
            [0x20, 0x20, 0x20, 0xFF],
            "圆角矩形中心没画出来"
        );
        // 圆角外的角落应为空
        assert_eq!(px(&buf, 10, 10), [0, 0, 0, 0], "圆角矩形没有切角");
    }

    /// 圆角矩形描边必须真的画出来。
    #[test]
    fn round_rect_outline_writes_pixels() {
        let buf = draw(|g| g.draw_round_rect(10.0, 10.0, 40.0, 40.0, 8.0, 0xFF_FF_00_00, 2.0));
        // 上边中点（上边是直的）
        assert_eq!(
            px(&buf, 30, 10),
            [0x00, 0x00, 0xFF, 0xFF],
            "圆角矩形上边没画出来"
        );
    }

    /// 文本必须真的画出来。
    #[test]
    fn text_is_drawn() {
        let buf = draw(|g| {
            g.draw_text(4.0, 4.0, "Ab", 16.0, 0xFF_FF_FF_FF, false);
        });
        let lit = buf.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 10, "文本没有被绘制（亮像素 {lit}）");
    }
}
