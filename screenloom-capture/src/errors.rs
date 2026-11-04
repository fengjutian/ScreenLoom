//! 捕获错误分类。
//!
//! 区分「目标受保护无法捕获」（企业策略 / DRM / 权限不足）与「普通失败」，
//! 便于 UI 层给出不同的提示文案。

use screenloom_core::Error;

/// 目标受保护时使用的 **固定文案**，UI 层可直接展示。
pub const CAPTURE_PROTECTED_MESSAGE: &str = "设备环境不允许当前应用截图，可联系网管或IT人员确认。";

/// 判断错误是否属于「目标受保护无法捕获」。
pub fn is_capture_protected_err(e: &Error) -> bool {
    match e {
        Error::Capture(msg) => msg.contains(CAPTURE_PROTECTED_MESSAGE),
        _ => false,
    }
}

/// 构造「受保护」错误（文案固定，便于 UI 直接展示）。
pub(crate) fn protected_error(detail: &str) -> Error {
    Error::Capture(format!("{CAPTURE_PROTECTED_MESSAGE}（{detail}）"))
}

/// `E_ACCESSDENIED` / `DXGI_ERROR_ACCESS_DENIED`：目标受保护或权限不足。
pub(crate) fn is_access_denied(code: i32) -> bool {
    const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;
    const DXGI_ERROR_ACCESS_DENIED: i32 = 0x887A_002Bu32 as i32;
    code == E_ACCESSDENIED || code == DXGI_ERROR_ACCESS_DENIED
}

/// API 在当前系统上不存在（需要更新的 Windows 或缺少组件）。
pub(crate) fn is_api_unavailable(code: i32) -> bool {
    const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    const CLASS_E_CLASSNOTAVAILABLE: i32 = 0x8004_0111u32 as i32;
    const REGDB_E_CLASSNOTREG: i32 = 0x8004_0154u32 as i32;
    code == E_NOTIMPL
        || code == CLASS_E_CLASSNOTAVAILABLE
        || code == REGDB_E_CLASSNOTREG
        || code == E_INVALIDARG
}

/// 捕获失败（目标尺寸变化导致帧池失效），需要重建帧池后重试。
pub(crate) fn is_size_changed(code: i32) -> bool {
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    code == E_INVALIDARG
}

/// 把 `windows::core::Error` 归类成 `screenloom_core::Error`。
pub(crate) fn capture_error(context: &str, e: &windows::core::Error) -> Error {
    if is_access_denied(e.code().0) {
        protected_error(context)
    } else {
        Error::Capture(format!("{context}: {e}"))
    }
}
