//! 截图浮窗的“外部能力”接线：二维码、复制色值、窗口选择。
//!
//! `screenloom-ui` 不能依赖 app 层，所以浮窗只暴露 `set_*_handler` 回调；
//! 具体实现集中在这里，`flow.rs` 只负责安装，不掺业务细节。

use screenloom_core::{detect_qrs, Rect, RgbaImage};
use screenloom_ui::{post_ai_reply, post_window_picked, AiReply, Overlay};

use crate::windowpicker::{self, PickerMode};

/// 工具栏「识别二维码」→ 本地解码（后台线程，不阻塞 UI）。
pub fn install_qr_handler(overlay: &mut Overlay) {
    let hwnd_raw = overlay.hwnd().0 as isize;
    overlay.set_qr_handler(move |image: RgbaImage| {
        std::thread::spawn(move || {
            let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut core::ffi::c_void);
            let codes = detect_qrs(&image);
            let texts: Vec<String> = codes.iter().map(|c| c.text.clone()).collect();
            if !texts.is_empty() {
                // 识别结果直接进剪贴板，方便打开链接/粘贴。
                let _ = crate::clipboard::write_text(&texts.join("\n"));
            }
            post_ai_reply(hwnd, AiReply::QrCodes(texts));
        });
    });
}

/// 取色面板的 `Ctrl+C` → 写剪贴板。
pub fn install_clipboard_handler(overlay: &mut Overlay) {
    overlay.set_clipboard_handler(|text| {
        let _ = crate::clipboard::write_text(&text);
    });
}

/// 工具栏「窗口选择」→ 模态选择器，选中后把窗口矩形投回浮窗。
pub fn install_window_picker_handler(overlay: &mut Overlay, frame_bounds: Rect) {
    let hwnd_raw = overlay.hwnd().0 as isize;
    overlay.set_window_picker_handler(move || {
        let Some(picked) = windowpicker::run(PickerMode::PickTarget) else {
            return;
        };
        let Some(w) = picked.first() else {
            return;
        };
        let region = Rect::new(
            w.bounds.x - frame_bounds.x,
            w.bounds.y - frame_bounds.y,
            w.bounds.w,
            w.bounds.h,
        )
        .intersect(Rect::new(0, 0, frame_bounds.w, frame_bounds.h));
        let Some(region) = region else {
            return;
        };
        if region.is_empty() {
            return;
        }
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut core::ffi::c_void);
        post_window_picked(hwnd, region);
    });
}
