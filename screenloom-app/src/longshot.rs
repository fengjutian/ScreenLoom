//! 长截图（滚动截图）流程。
//!
//! 对齐微信的 `wx::LongScreenShoter` / `wx::OpenCVWorker` / `wx::SpliceWorker`：
//! 用户滚动页面 → 每按一次 Enter 抓一帧 → 模板匹配求出纵向偏移 → 拼接。
//!
//! 与微信的差异：微信用 OpenCV 做模板匹配，这里用 `screenloom_core::match_offset`
//! （纯 Rust SAD 匹配），省掉 OpenCV 原生依赖。
//!
//! 交互提示用一个置顶小窗承载（自己实现，不依赖 screenloom-ui）。

use std::sync::{Mutex, OnceLock};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, InvalidateRect,
    SetBkMode, SetTextColor, DT_CENTER, DT_VCENTER, DT_WORDBREAK, HGDIOBJ, PAINTSTRUCT,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
    SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, HWND_TOPMOST, MSG, SWP_NOMOVE,
    SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WM_DESTROY, WM_KEYDOWN, WM_PAINT, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use screenloom_capture::{capture_virtual_screen, CaptureBackend};
use screenloom_core::{append_scroll, match_offset, Config, Error, Rect, Result, RgbaImage};

/// 提示窗上显示的文本（特殊值表示按键事件）。
static HINT_TEXT: OnceLock<Mutex<String>> = OnceLock::new();

const ENTER_FLAG: &str = "__SHOT_ENTER__";
const ESC_FLAG: &str = "__SHOT_ESC__";

fn hint_text() -> &'static Mutex<String> {
    HINT_TEXT.get_or_init(|| Mutex::new(String::new()))
}

fn set_hint(s: &str) {
    if let Ok(mut g) = hint_text().lock() {
        *g = s.to_string();
    }
}

/// 窗口过程：只处理绘制与按键，其余交给默认过程。
unsafe extern "system" fn hint_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 420,
                bottom: 92,
            };
            let brush = CreateSolidBrush(COLORREF(0x00201C1C));
            FillRect(hdc, &rc, brush);
            let _ = DeleteObject(HGDIOBJ(brush.0));
            SetTextColor(hdc, COLORREF(0x00FFFFFF));
            SetBkMode(hdc, TRANSPARENT);
            let text = hint_text()
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            let mut buf: Vec<u16> = text.encode_utf16().collect();
            rc.left = 12;
            rc.top = 8;
            rc.right = 408;
            rc.bottom = 84;
            DrawTextW(
                hdc,
                &mut buf,
                &mut rc,
                DT_CENTER | DT_WORDBREAK | DT_VCENTER,
            );
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = wparam.0 as u32;
            if vk == VK_RETURN.0 as u32 {
                set_hint(ENTER_FLAG);
            } else if vk == VK_ESCAPE.0 as u32 {
                set_hint(ESC_FLAG);
            }
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 置顶提示窗。
struct HintWindow {
    hwnd: HWND,
}

impl HintWindow {
    fn new(text: &str) -> Result<HintWindow> {
        set_hint(text);
        // SAFETY: 注册窗口类与创建窗口均为一次性操作。
        unsafe {
            let hinst = GetModuleHandleW(None)
                .map_err(|e| Error::Window(format!("获取模块句柄失败: {e}")))?;
            let hinstance = windows::Win32::Foundation::HINSTANCE(hinst.0);
            let cls = w!("ScreenshotLongShotHint");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(hint_wnd_proc),
                hInstance: hinstance,
                lpszClassName: PCWSTR(cls.as_ptr()),
                ..Default::default()
            };
            // 重复注册会返回 0，但类已存在时可忽略
            let _ = RegisterClassW(&wc);

            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(cls.as_ptr()),
                w!("滚动截图"),
                WS_POPUP,
                40,
                40,
                420,
                92,
                None,
                None,
                Some(hinstance),
                None,
            )
            .map_err(|e| Error::Window(format!("创建提示窗失败: {e}")))?;
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            )
            .map_err(|e| Error::Window(format!("置顶提示窗失败: {e}")))?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // 提示窗必须**拿到键盘焦点**，否则用户的 Enter / Esc 根本送不到这里
            //（浮窗刚退出、提示窗是 WS_POPUP，Windows 不会自动把焦点给它）。
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            Ok(HintWindow { hwnd })
        }
    }

    /// 更新提示文字并重绘。
    fn set(&self, text: &str) {
        set_hint(text);
        // SAFETY: 仅触发重绘
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// 等待用户按键。返回 true = Enter（继续拼接），false = Esc（结束）。
    fn wait_key(&self) -> bool {
        let mut msg = MSG::default();
        loop {
            // SAFETY: 标准消息循环
            unsafe {
                let r = GetMessageW(&mut msg, None, 0, 0);
                if r.0 <= 0 {
                    return false;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
            let flag = hint_text()
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            if flag == ENTER_FLAG {
                set_hint("");
                return true;
            }
            if flag == ESC_FLAG {
                set_hint("");
                return false;
            }
        }
    }
}

impl Drop for HintWindow {
    fn drop(&mut self) {
        // SAFETY: 销毁自己创建的窗口
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// 长截图会话。
pub struct LongShot {
    /// 目标区域在虚拟屏幕中的物理像素矩形
    pub screen_rect: Rect,
    /// 已拼接的结果（第一帧）
    pub canvas: RgbaImage,
}

impl LongShot {
    /// 跑完整个长截图交互循环，返回拼接结果。
    pub fn run(self, cfg: &Config, backend: &dyn CaptureBackend) -> Result<RgbaImage> {
        let LongShot {
            screen_rect,
            canvas: first,
        } = self;
        let mut canvas = first;
        // 模板匹配必须拿「上一屏」比，不能拿已经拼接好的长图比：
        // 长图高度大于单屏后，match_offset 的坐标系会错，追加量也会算错。
        let mut prev_view = canvas.clone();
        let hint = HintWindow::new("滚动页面后按 Enter 拼接\n按 Esc 结束长截图")?;
        let mut pieces = 1usize;

        loop {
            if !hint.wait_key() {
                break;
            }
            let next = match grab_region(backend, screen_rect) {
                Ok(f) => f,
                Err(e) => {
                    hint.set(&format!("抓屏失败：{e}\n按 Enter 重试，按 Esc 结束"));
                    continue;
                }
            };
            let next_view = next.clone();
            match match_offset(&prev_view, &next, cfg.long_screenshot_search) {
                Some(dy) if dy != 0 => match append_scroll(&canvas, &next, dy) {
                    Some(merged) => {
                        canvas = merged;
                        pieces += 1;
                        hint.set(&format!(
                            "已拼接 {pieces} 段（偏移 {dy}px）\n继续滚动后按 Enter，Esc 结束"
                        ));
                    }
                    None => {
                        hint.set("拼接失败，按 Enter 重试，按 Esc 结束");
                    }
                },
                Some(_) => {
                    hint.set("画面没有滚动，按 Enter 重试，按 Esc 结束");
                }
                None => {
                    // 对齐微信文案
                    hint.set("截图中断，返回中断位置继续滚动\n按 Enter 重试，按 Esc 结束");
                }
            }
            // 下一轮以当前画面为基准，而不是以上一帧成功拼接的旧画面为基准。
            prev_view = next_view;

            if cfg.long_screenshot_max > 0 && canvas.h >= cfg.long_screenshot_max {
                hint.set("已达最大长度，无法继续截图\n按 Esc 结束");
                let _ = hint.wait_key();
                break;
            }
        }
        Ok(canvas)
    }
}

/// 抓取指定屏幕区域（虚拟屏物理坐标）。
///
/// 抓 **整块虚拟屏** 再裁剪：选区可能跨显示器，只抓单块显示器会把区域算错。
fn grab_region(backend: &dyn CaptureBackend, screen_rect: Rect) -> Result<RgbaImage> {
    let frame = capture_virtual_screen(backend)?;
    let local = Rect::new(
        screen_rect.x - frame.bounds.x,
        screen_rect.y - frame.bounds.y,
        screen_rect.w,
        screen_rect.h,
    );
    let cropped = frame.image.crop(local);
    if cropped.w == 0 || cropped.h == 0 {
        return Err(Error::Capture("长截图区域无效".into()));
    }
    Ok(cropped)
}
