//! 全局热键注册。
//!
//! 对齐微信的 `gScreenshotShortcutKey` 配置项：支持 `Alt+A` / `Ctrl+Shift+S` 形式。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
    VIRTUAL_KEY,
};

use screenloom_core::{Error, Result};

/// 已注册的热键。
pub struct Hotkey {
    hwnd: HWND,
    id: i32,
}

impl Hotkey {
    /// 注册热键。`spec` 形如 `Alt+A`、`Ctrl+Shift+S`。
    ///
    /// 失败返回错误（常见于热键被其它程序占用）。
    pub fn register(hwnd: HWND, id: i32, spec: &str) -> Result<Hotkey> {
        let (mods, vk) = parse_spec(spec)?;
        // SAFETY: hwnd/id 有效，modifiers/vk 来自解析结果。
        unsafe { RegisterHotKey(Some(hwnd), id, mods, vk) }
            .map_err(|e| Error::Window(format!("注册热键 {spec} 失败: {e}")))?;
        Ok(Hotkey { hwnd, id })
    }

    /// 注销热键。
    pub fn unregister(&self) {
        // SAFETY: 句柄与 id 来自本结构，注册时已成功。
        unsafe {
            let _ = UnregisterHotKey(Some(self.hwnd), self.id);
        }
    }
}

impl Drop for Hotkey {
    fn drop(&mut self) {
        self.unregister();
    }
}

/// 解析 `Alt+Ctrl+Shift+Win+A` 形式的快捷键描述。
pub fn parse_spec(spec: &str) -> Result<(HOT_KEY_MODIFIERS, u32)> {
    let parts: Vec<&str> = spec
        .split(['+', '-'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(Error::Config("热键为空".into()));
    }
    let key = parts[parts.len() - 1];
    let mut mods = 0u32;
    for m in &parts[..parts.len() - 1] {
        match m.to_ascii_lowercase().as_str() {
            "alt" => mods |= MOD_ALT.0,
            "ctrl" | "control" => mods |= MOD_CONTROL.0,
            "shift" => mods |= MOD_SHIFT.0,
            "win" | "windows" | "super" => mods |= MOD_WIN.0,
            other => return Err(Error::Config(format!("未知修饰键: {other}"))),
        }
    }
    let vk = parse_vk(key)?;
    Ok((HOT_KEY_MODIFIERS(mods), vk.0 as u32))
}

/// 键名 -> 虚拟键码。支持单字符与 F1–F24。
fn parse_vk(key: &str) -> Result<VIRTUAL_KEY> {
    let k = key.trim();
    if k.chars().count() == 1 {
        let c = k.chars().next().unwrap();
        let upper = c.to_ascii_uppercase();
        if upper.is_ascii_alphanumeric() {
            return Ok(VIRTUAL_KEY(upper as u16));
        }
    }
    if let Some(num) = k.strip_prefix('F').or_else(|| k.strip_prefix('f')) {
        if let Ok(n) = num.parse::<u16>() {
            if (1..=24).contains(&n) {
                return Ok(VIRTUAL_KEY(0x70 + n - 1)); // VK_F1 = 0x70
            }
        }
    }
    // 常见特殊键
    let special = match k.to_ascii_uppercase().as_str() {
        "ESC" => 0x1B,
        "SPACE" => 0x20,
        "ENTER" | "RETURN" => 0x0D,
        "TAB" => 0x09,
        "BACKSPACE" => 0x08,
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        _ => 0,
    };
    if special != 0 {
        return Ok(VIRTUAL_KEY(special));
    }
    Err(Error::Config(format!("无法识别的按键: {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alt_a() {
        let (mods, vk) = parse_spec("Alt+A").unwrap();
        assert_eq!(mods.0, MOD_ALT.0);
        assert_eq!(vk, b'A' as u32);
    }

    #[test]
    fn parses_combo() {
        let (mods, vk) = parse_spec("Ctrl+Shift+S").unwrap();
        assert_eq!(mods.0, MOD_CONTROL.0 | MOD_SHIFT.0);
        assert_eq!(vk, b'S' as u32);
    }

    #[test]
    fn parses_function_key() {
        let (_, vk) = parse_spec("Alt+F5").unwrap();
        assert_eq!(vk, 0x70 + 4);
    }

    #[test]
    fn rejects_unknown_modifier() {
        assert!(parse_spec("Hyper+A").is_err());
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(parse_spec("Alt+@@").is_err());
    }
}
