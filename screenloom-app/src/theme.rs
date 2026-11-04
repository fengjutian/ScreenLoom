//! 深色主题：配色与绘制基元。
//!
//! 设置窗口是**完全自绘**的，所有外观都从这里出：
//! 面板、分组标题、输入框外框、复选框、下拉框、按钮、下拉菜单。
//!
//! 颜色一律 `0xAARRGGBB`（和 `screenloom_ui::gdiplus::argb` 一致）。
//!
//! # 运行时热重载
//!
//! 配色与圆角**不是编译期常量**，而是存在一个全局 `RwLock<Theme>` 里。
//! 用 `--theme PATH` 指定一个 JSON 覆盖文件后：
//!
//! ```json
//! { "accent": "#07C160", "bg": "#131518", "radius_panel": 9 }
//! ```
//!
//! 设置窗口的定时器会盯这个文件的修改时间，**变了就热重载并重绘** ——
//! 调配色 / 圆角不用重新编译，保存文件即时生效。
//! 没给 `--theme` 时用内置默认值，行为与之前完全一致。

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use screenloom_ui::gdiplus::Graphics;

// ---------------------------------------------------------------- 主题定义

/// 主题：全部颜色 + 圆角半径（圆角是**逻辑**像素）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    // ---- 颜色（0xAARRGGBB）----
    /// 窗口底色。
    pub bg: u32,
    /// 面板底色。
    pub panel: u32,
    /// 面板描边。
    pub panel_border: u32,
    /// 输入框底色（必须和原生 EDIT 的底色**完全一致**，否则会露出方角）。
    pub field_bg: u32,
    /// 输入框底色 - 悬停。
    pub field_bg_hover: u32,
    /// 输入框描边。
    pub field_border: u32,
    /// 输入框描边 - 悬停。
    pub field_border_hover: u32,
    /// 输入框聚焦描边。
    pub field_focus: u32,
    /// 主文字。
    pub text: u32,
    /// 次级文字（字段标签）。
    pub text_dim: u32,
    /// 三级文字（占位符、计数）。
    pub text_faint: u32,
    /// 强调色（微信绿）。
    pub accent: u32,
    /// 强调色 - 悬停。
    pub accent_hover: u32,
    /// 强调色 - 按下。
    pub accent_down: u32,
    /// 次要按钮底色。
    pub btn_bg: u32,
    /// 次要按钮底色 - 悬停。
    pub btn_bg_hover: u32,
    /// 次要按钮底色 - 按下。
    pub btn_bg_down: u32,
    /// 次要按钮描边。
    pub btn_border: u32,
    /// 下拉菜单底色。
    pub menu_bg: u32,
    /// 下拉菜单项 - 悬停。
    pub menu_hover: u32,
    /// 下拉菜单描边。
    pub menu_border: u32,
    /// 焦点环。
    pub focus_ring: u32,
    /// 纯白（对勾）。
    pub white: u32,

    // ---- 圆角（逻辑像素）----
    /// 面板圆角。
    pub radius_panel: f32,
    /// 输入框圆角。
    pub radius_field: f32,
    /// 按钮圆角。
    pub radius_btn: f32,
    /// 下拉菜单圆角。
    pub radius_menu: f32,
    /// 复选框圆角。
    pub radius_check: f32,
}

impl Theme {
    /// 内置默认主题（深色专业风 + 微信绿强调色）。
    pub const DEFAULT: Theme = Theme {
        bg: 0xFF13_1518,
        panel: 0xFF1C_1F24,
        panel_border: 0xFF2B_3037,
        field_bg: 0xFF16_181C,
        field_bg_hover: 0xFF1B_1E23,
        field_border: 0xFF33_3942,
        field_border_hover: 0xFF45_4D57,
        field_focus: 0xFF07_C160,
        text: 0xFF_E7EAEE,
        text_dim: 0xFF98_A1AC,
        text_faint: 0xFF69_727D,
        accent: 0xFF07_C160,
        accent_hover: 0xFF2B_D37F,
        accent_down: 0xFF05_A854,
        btn_bg: 0xFF25_2A31,
        btn_bg_hover: 0xFF31_3842,
        btn_bg_down: 0xFF1F_2329,
        btn_border: 0xFF3A_424C,
        menu_bg: 0xFF23_2830,
        menu_hover: 0xFF30_3843,
        menu_border: 0xFF3C_444F,
        focus_ring: 0xFF07_C160,
        white: 0xFF_FF_FF_FF,

        radius_panel: 9.0,
        radius_field: 6.0,
        radius_btn: 6.0,
        radius_menu: 8.0,
        radius_check: 4.0,
    };
}

impl Default for Theme {
    fn default() -> Self {
        Theme::DEFAULT
    }
}

/// 当前生效的主题。
static THEME: RwLock<Theme> = RwLock::new(Theme::DEFAULT);

/// 覆盖文件路径（`--theme PATH`）。
static OVERRIDE_PATH: RwLock<Option<PathBuf>> = RwLock::new(None);

/// 覆盖文件上次读到时的 (修改时间, 长度)，用来判断"文件变了没有"。
static OVERRIDE_STAMP: RwLock<Option<(u64, u64)>> = RwLock::new(None);

/// 取当前主题（只读锁）。
pub fn current() -> std::sync::RwLockReadGuard<'static, Theme> {
    THEME.read().unwrap_or_else(|e| e.into_inner())
}

/// 设置覆盖文件路径（`None` = 回到内置默认）。
pub fn set_override_path(p: Option<PathBuf>) {
    let mut guard = OVERRIDE_PATH.write().unwrap_or_else(|e| e.into_inner());
    *guard = p;
    let mut stamp = OVERRIDE_STAMP.write().unwrap_or_else(|e| e.into_inner());
    *stamp = None;
}

/// 覆盖文件路径。
pub fn override_path() -> Option<PathBuf> {
    OVERRIDE_PATH
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 覆盖文件是否被改动过（比读一遍文件便宜得多）。
///
/// 只在设了 `--theme` 时有意义；没设永远返回 `false`。
pub fn source_changed() -> bool {
    let Some(path) = override_path() else {
        return false;
    };
    let now = file_stamp(&path);
    let last = OVERRIDE_STAMP.read().unwrap_or_else(|e| e.into_inner());
    now != *last
}

/// 重新读覆盖文件并应用。**返回主题是否真的变了**。
pub fn reload() -> bool {
    let path = override_path();

    // 记录本次读到的文件版本（供 `source_changed` 比较）
    {
        let mut stamp = OVERRIDE_STAMP.write().unwrap_or_else(|e| e.into_inner());
        *stamp = path.as_deref().and_then(file_stamp);
    }

    let Some(path) = path else {
        // 没有覆盖文件 → 回到默认
        let mut guard = THEME.write().unwrap_or_else(|e| e.into_inner());
        if *guard == Theme::DEFAULT {
            return false;
        }
        *guard = Theme::DEFAULT;
        return true;
    };

    let mut next = Theme::DEFAULT;
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            apply_json(&mut next, &v);
        }
    }

    let mut guard = THEME.write().unwrap_or_else(|e| e.into_inner());
    if *guard == next {
        return false;
    }
    *guard = next;
    true
}

/// 文件的 (修改时间, 长度)。
fn file_stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some((mtime, meta.len()))
}

/// 把 JSON 覆盖应用到主题上。返回生效的项数。
fn apply_json(t: &mut Theme, v: &serde_json::Value) -> usize {
    let Some(obj) = v.as_object() else {
        return 0;
    };
    let mut n = 0;
    for (key, val) in obj {
        if apply_one(t, key, val) {
            n += 1;
        }
    }
    n
}

/// 应用单个键。
///
/// **按键名**判断是颜色还是圆角 —— 不能按值的类型判断：
/// 圆角也是数字，若先按值判成颜色，圆角键就永远不生效。
fn apply_one(t: &mut Theme, key: &str, val: &serde_json::Value) -> bool {
    match key {
        // ---- 圆角（逻辑像素）----
        "radius_panel" | "radius_field" | "radius_btn" | "radius_menu" | "radius_check" => {
            let Some(r) = val.as_f64() else { return false };
            match key {
                "radius_panel" => t.radius_panel = r as f32,
                "radius_field" => t.radius_field = r as f32,
                "radius_btn" => t.radius_btn = r as f32,
                "radius_menu" => t.radius_menu = r as f32,
                _ => t.radius_check = r as f32,
            }
            true
        }
        // ---- 颜色 ----
        _ => {
            let Some(c) = color_value(val) else {
                return false;
            };
            match key {
                "bg" => t.bg = c,
                "panel" => t.panel = c,
                "panel_border" => t.panel_border = c,
                "field_bg" => t.field_bg = c,
                "field_bg_hover" => t.field_bg_hover = c,
                "field_border" => t.field_border = c,
                "field_border_hover" => t.field_border_hover = c,
                "field_focus" => t.field_focus = c,
                "text" => t.text = c,
                "text_dim" => t.text_dim = c,
                "text_faint" => t.text_faint = c,
                "accent" => t.accent = c,
                "accent_hover" => t.accent_hover = c,
                "accent_down" => t.accent_down = c,
                "btn_bg" => t.btn_bg = c,
                "btn_bg_hover" => t.btn_bg_hover = c,
                "btn_bg_down" => t.btn_bg_down = c,
                "btn_border" => t.btn_border = c,
                "menu_bg" => t.menu_bg = c,
                "menu_hover" => t.menu_hover = c,
                "menu_border" => t.menu_border = c,
                "focus_ring" => t.focus_ring = c,
                "white" => t.white = c,
                _ => return false,
            }
            true
        }
    }
}

/// 取颜色值：字符串（`#RRGGBB` / `#AARRGGBB`）或数字（`0xAARRGGBB`）。
fn color_value(val: &serde_json::Value) -> Option<u32> {
    if let Some(s) = val.as_str() {
        return parse_color(s);
    }
    val.as_u64().filter(|v| *v <= 0xFFFF_FFFF).map(|v| v as u32)
}

/// 解析颜色：`#RRGGBB` / `#AARRGGBB` / `RRGGBB` / `AARRGGBB`（不带 `#`）。
fn parse_color(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches('#').trim_start_matches("0x");
    match t.len() {
        6 => u32::from_str_radix(t, 16).ok().map(|v| 0xFF00_0000 | v),
        8 => u32::from_str_radix(t, 16).ok(),
        _ => None,
    }
}

/// 拆成 (r, g, b) —— 方便构造 `COLORREF`。
pub fn rgb(c: u32) -> (u8, u8, u8) {
    (
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
    )
}

// ---------------------------------------------------------------- 几何

/// 整数矩形（物理像素）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// 点是否落在矩形内（左闭右开）。
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// 逻辑像素 → 物理像素。
pub fn px(v: i32, k: f32) -> i32 {
    (v as f32 * k).round() as i32
}

/// 逻辑浮点 → 物理浮点（线宽、圆角半径这类亚像素值用）。
pub fn pfx(v: f32, k: f32) -> f32 {
    (v * k).round()
}

/// 主题圆角（逻辑像素，可为小数）→ 物理像素。
fn r_pt(v: f32, k: f32) -> f32 {
    (v * k).round()
}

// ---------------------------------------------------------------- 基元

/// 圆角矩形（填充 + 可选描边）。
pub fn round_box(g: &mut Graphics, r: Rect, radius: f32, fill: u32, border: Option<(u32, f32)>) {
    let (x, y, w, h) = (r.x as f32, r.y as f32, r.w as f32, r.h as f32);
    g.fill_round_rect(x, y, w, h, radius, fill);
    if let Some((c, lw)) = border {
        g.draw_round_rect(x, y, w, h, radius, c, lw);
    }
}

/// 分组面板（圆角来自主题）。
pub fn panel(g: &mut Graphics, r: Rect, k: f32) {
    let t = current();
    round_box(
        g,
        r,
        r_pt(t.radius_panel, k),
        t.panel,
        Some((t.panel_border, 1.0)),
    );
}

/// 分组标题：左侧强调色竖条 + 标题（+ 淡色补充说明）。
pub fn section_header(g: &mut Graphics, x: i32, y: i32, title: &str, note: &str, k: f32) {
    let t = current();
    let bar_w = px(3, k);
    let bar_h = px(15, k);
    let bar_y = y + (px(20, k) - bar_h) / 2;
    g.fill_round_rect(
        x as f32,
        bar_y as f32,
        bar_w as f32,
        bar_h as f32,
        (bar_w as f32) / 2.0,
        t.accent,
    );

    let f = px(15, k) as f32;
    let (tw, th) = g.draw_text((x + px(10, k)) as f32, y as f32, title, f, t.text, true);
    if !note.is_empty() {
        // 备注画在标题**右边**（不是下一行 —— 画在下一行会压住标题）
        let nf = px(11, k) as f32;
        let (_, nth) = g.measure_text(note, nf, false);
        g.draw_text(
            (x + px(10, k)) as f32 + tw + px(10, k) as f32,
            y as f32 + (th - nth) / 2.0,
            note,
            nf,
            t.text_faint,
            false,
        );
    }
}

/// 输入框外框（原生 EDIT 画在上面，这里只画底 + 描边）。
pub fn input_frame(g: &mut Graphics, r: Rect, focused: bool, hovered: bool, k: f32) {
    let t = current();
    let radius = r_pt(t.radius_field, k);
    let fill = if hovered {
        t.field_bg_hover
    } else {
        t.field_bg
    };
    g.fill_round_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, radius, fill);
    let (c, w) = if focused {
        (t.field_focus, pfx(1.6, k))
    } else if hovered {
        (t.field_border_hover, 1.0)
    } else {
        (t.field_border, 1.0)
    };
    g.draw_round_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, radius, c, w);
}

/// 复选框方块。
pub fn checkbox(g: &mut Graphics, x: i32, y: i32, size: i32, checked: bool, hover: bool, k: f32) {
    let t = current();
    let s = size as f32;
    let radius = r_pt(t.radius_check, k);
    if checked {
        g.fill_round_rect(x as f32, y as f32, s, s, radius, t.accent);
        // 对勾：两段线
        let lw = pfx(1.9, k);
        let (cx, cy) = (x as f32, y as f32);
        g.draw_line(
            cx + s * 0.24,
            cy + s * 0.52,
            cx + s * 0.42,
            cy + s * 0.70,
            t.white,
            lw,
        );
        g.draw_line(
            cx + s * 0.42,
            cy + s * 0.70,
            cx + s * 0.76,
            cy + s * 0.30,
            t.white,
            lw,
        );
    } else {
        let fill = if hover { t.field_bg_hover } else { t.field_bg };
        g.fill_round_rect(x as f32, y as f32, s, s, radius, fill);
        let border = if hover {
            t.field_border_hover
        } else {
            t.field_border
        };
        g.draw_round_rect(x as f32, y as f32, s, s, radius, border, 1.0);
    }
}

/// 下拉框本体。
pub fn combo_box(
    g: &mut Graphics,
    r: Rect,
    text: &str,
    open: bool,
    hover: bool,
    focused: bool,
    k: f32,
) {
    input_frame(g, r, open || focused, hover, k);

    let t = current();
    let f = px(13, k) as f32;
    let (_, th) = g.measure_text(text, f, false);
    let ty = r.y as f32 + (r.h as f32 - th) / 2.0 - px(1, k) as f32;
    g.draw_text((r.x + px(11, k)) as f32, ty, text, f, t.text, false);

    // 下拉箭头（两段线拼一个 v）
    let ax = r.right() - px(15, k);
    let ay = r.y as f32 + r.h as f32 / 2.0;
    let s = px(4, k) as f32;
    let lw = pfx(1.5, k);
    let col = if open { t.accent } else { t.text_dim };
    g.draw_line(
        ax as f32 - s,
        ay - s * 0.6,
        ax as f32,
        ay + s * 0.4,
        col,
        lw,
    );
    g.draw_line(
        ax as f32,
        ay + s * 0.4,
        ax as f32 + s,
        ay - s * 0.6,
        col,
        lw,
    );
}

/// 按钮样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnStyle {
    /// 主按钮（强调色实心）。
    Primary,
    /// 次按钮（描边）。
    Secondary,
}

/// 按钮状态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnState {
    Normal,
    Hover,
    Pressed,
}

/// 按钮。
///
/// `behind` = 按钮**背后**的颜色（面板色 / 窗口底色）：
/// 圆角之外的那 4 个角要填成它，否则会露出下层内容。
pub fn button(
    g: &mut Graphics,
    r: Rect,
    label: &str,
    style: BtnStyle,
    state: BtnState,
    k: f32,
    behind: u32,
) {
    let t = current();
    let radius = r_pt(t.radius_btn, k);
    let (fill, text, border) = match (style, state) {
        (BtnStyle::Primary, BtnState::Hover) => (t.accent_hover, t.white, None),
        (BtnStyle::Primary, BtnState::Pressed) => (t.accent_down, t.white, None),
        (BtnStyle::Primary, _) => (t.accent, t.white, None),
        (BtnStyle::Secondary, BtnState::Hover) => {
            (t.btn_bg_hover, t.text, Some((t.btn_border, 1.0)))
        }
        (BtnStyle::Secondary, BtnState::Pressed) => {
            (t.btn_bg_down, t.text, Some((t.btn_border, 1.0)))
        }
        (BtnStyle::Secondary, _) => (t.btn_bg, t.text, Some((t.btn_border, 1.0))),
    };
    // 先把整块客户区填成**按钮背后的底色**（不留透明角），
    // 否则系统焦点框会从圆角缝隙里露出来。
    g.fill_rect(r.x as f32, r.y as f32, r.w as f32, r.h as f32, behind);
    round_box(g, r, radius, fill, border);

    let f = px(13, k) as f32;
    let (tw, th) = g.measure_text(label, f, true);
    let tx = r.x as f32 + (r.w as f32 - tw) / 2.0;
    let ty = r.y as f32 + (r.h as f32 - th) / 2.0 - px(1, k) as f32;
    g.draw_text(tx, ty, label, f, text, true);
}

/// 焦点环（画在控件外侧一圈）。
pub fn focus_ring(g: &mut Graphics, r: Rect, k: f32) {
    let t = current();
    let radius = r_pt(t.radius_field + 1.0, k);
    let pad = pfx(2.0, k);
    g.draw_round_rect(
        r.x as f32 - pad,
        r.y as f32 - pad,
        r.w as f32 + pad * 2.0,
        r.h as f32 + pad * 2.0,
        radius,
        t.focus_ring,
        pfx(1.4, k),
    );
}

/// 下拉菜单（自绘，独立的弹出窗口用）。
///
/// `item_h` 是每项高度，`hover` 是当前悬停下标。
pub fn menu(
    g: &mut Graphics,
    r: Rect,
    items: &[&str],
    selected: usize,
    hover: Option<usize>,
    item_h: i32,
    k: f32,
) {
    let t = current();
    round_box(
        g,
        r,
        r_pt(t.radius_menu, k),
        t.menu_bg,
        Some((t.menu_border, 1.0)),
    );

    let f = px(13, k) as f32;
    for (i, it) in items.iter().enumerate() {
        let iy = r.y + px(4, k) + i as i32 * item_h;
        let ir = Rect::new(r.x + px(4, k), iy, r.w - px(8, k), item_h);
        if hover == Some(i) {
            g.fill_round_rect(
                ir.x as f32,
                ir.y as f32,
                ir.w as f32,
                ir.h as f32,
                px(5, k) as f32,
                t.menu_hover,
            );
        }
        let (_, th) = g.measure_text(it, f, false);
        let ty = ir.y as f32 + (ir.h as f32 - th) / 2.0 - px(1, k) as f32;
        g.draw_text((ir.x + px(10, k)) as f32, ty, it, f, t.text, false);
        if selected == i {
            // 选中项右侧打一个对勾
            let cx = ir.right() - px(18, k);
            let cy = ir.y as f32 + ir.h as f32 / 2.0;
            let s = px(4, k) as f32;
            let lw = pfx(1.8, k);
            g.draw_line(cx as f32 - s, cy, cx as f32, cy + s * 0.6, t.accent, lw);
            g.draw_line(
                cx as f32,
                cy + s * 0.6,
                cx as f32 + s,
                cy - s * 0.6,
                t.accent,
                lw,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_hit_test_is_half_open() {
        let r = Rect::new(10, 20, 100, 40);
        assert!(r.contains(10, 20), "左上角应命中");
        assert!(r.contains(109, 59), "右下角前一像素应命中");
        assert!(!r.contains(110, 59), "右边界应排除");
        assert!(!r.contains(9, 20), "左边界应排除");
        assert_eq!(r.right(), 110);
        assert_eq!(r.bottom(), 60);
    }

    #[test]
    fn px_scales_and_rounds() {
        assert_eq!(px(10, 1.0), 10);
        assert_eq!(px(10, 1.25), 13); // 12.5 → 13
        assert_eq!(px(0, 1.25), 0);
    }

    #[test]
    fn parse_color_accepts_shorthand_and_full() {
        assert_eq!(parse_color("#131518"), Some(0xFF13_1518));
        assert_eq!(parse_color("131518"), Some(0xFF13_1518));
        assert_eq!(parse_color("#FF131518"), Some(0xFF13_1518));
        assert_eq!(parse_color("zzz"), None);
    }

    #[test]
    fn json_override_updates_only_named_fields() {
        let mut t = Theme::DEFAULT;
        let v: serde_json::Value =
            serde_json::from_str(r##"{"accent":"#FF0000","radius_panel":12,"unknown_key":1}"##)
                .unwrap();
        let n = apply_json(&mut t, &v);
        assert_eq!(n, 2, "只应生效 accent 与 radius_panel");
        assert_eq!(t.accent, 0xFFFF_0000);
        assert_eq!(t.radius_panel, 12.0);
        // 其它字段不动
        assert_eq!(t.bg, Theme::DEFAULT.bg);
        assert_eq!(t.radius_field, Theme::DEFAULT.radius_field);
    }

    #[test]
    fn rgb_splits_argb() {
        assert_eq!(rgb(0xFF07_C160), (0x07, 0xC1, 0x60));
    }
}
