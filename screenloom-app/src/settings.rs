//! 设置窗口：**完全自绘**的深色界面。
//!
//! 自绘的部分：窗口底色、分组面板、分组标题、输入框外框、复选框、
//! 下拉框本体、下拉菜单、底部按钮外观。
//!
//! 仍然用原生控件的部分（为了 IME / 剪贴板 / 撤销 / 无障碍）：
//!
//! * **文本输入**用原生 `EDIT`，但底色与文字色改成深色，圆角外框由宿主自绘。
//! * **按钮**用原生 `BUTTON` + `BS_OWNERDRAW`，外观自绘，
//!   但 `BM_CLICK`、键盘焦点、屏幕阅读器照旧可用（端到端脚本依赖这一点）。
//!
//! 复选框与下拉框是**纯自绘**控件，自己实现命中测试、焦点环与键盘操作。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
    CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, InvalidateRect, SelectObject, SetBkColor,
    SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    DEFAULT_PITCH, FF_DONTCARE, HBITMAP, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
    PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN,
    VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHBrowseForFolderW, SHGetPathFromIDListW, SetWindowSubclass, BIF_EDITBOX,
    BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use screenloom_core::{Config, CopyFormat, Error, Result};
use screenloom_ui::gdiplus::Graphics;

use crate::theme::{self, Rect};

const CLASS_NAME: &str = "ScreenloomSettingsWindow";
const POPUP_CLASS_NAME: &str = "ScreenloomComboPopup";
/// 自绘按钮的窗口类名。
const FLAT_BTN_CLASS: &str = "ScreenloomFlatButton";

/// 「保存」按钮 id。
const ID_SAVE: usize = 1001;
/// 「取消」按钮 id。
const ID_CANCEL: usize = 1002;
/// 「浏览…」按钮 id。
const ID_BROWSE: usize = 1003;

/// 主题热重载定时器 id（每 400ms 查一次覆盖文件的修改时间）。
const TIMER_THEME: usize = 1;

// ---------------------------------------------------------------- 自定义消息

/// 子控件 → 宿主：请求移动焦点（wp = +1 / -1）。
const WM_APP_FOCUS_NEXT: u32 = WM_APP + 21;
/// 子控件 → 宿主：请求保存。
const WM_APP_SAVE: u32 = WM_APP + 22;
/// 子控件 → 宿主：请求取消。
const WM_APP_CANCEL: u32 = WM_APP + 23;
/// 子控件 → 宿主：状态变了，重画。
const WM_APP_REDRAW: u32 = WM_APP + 24;
/// 下拉菜单 → 宿主：选中了第 n 项。
const WM_APP_COMBO_SELECT: u32 = WM_APP + 25;
/// 下拉菜单 → 宿主：菜单关闭了。
const WM_APP_COMBO_CLOSED: u32 = WM_APP + 26;
/// 子控件 → 宿主：鼠标不在按钮上了。
const WM_APP_HOVER_CLEAR: u32 = WM_APP + 28;

// ---------------------------------------------------------------- 尺寸常量（逻辑像素）

/// 客户区宽度。
const WIN_W: i32 = 640;
/// 客户区四周留白。
const PAD: i32 = 20;
/// 面板内边距（左右）。
const PANEL_PAD_X: i32 = 18;
/// 面板内边距（上）。
const PANEL_PAD_TOP: i32 = 10;
/// 面板内边距（下）。
const PANEL_PAD_BOTTOM: i32 = 14;
/// 分组标题高度。
const HEADER_H: i32 = 30;
/// 行高。
const ROW_H: i32 = 30;
/// 行间距。
const ROW_GAP: i32 = 6;
/// 分组间距。
const SECTION_GAP: i32 = 12;
/// 标签列宽。
const LABEL_W: i32 = 196;
/// 标签列与控件列间距。
const CTRL_GAP: i32 = 16;
/// 底部按钮区与最后一个分组的间距。
const FOOTER_GAP: i32 = 16;
/// 输入框高度。
const FIELD_H: i32 = 30;
/// 按钮高度。
const BTN_H: i32 = 32;
/// 按钮宽度。
const BTN_W: i32 = 96;
/// 「浏览…」按钮宽度。
const BROWSE_W: i32 = 76;
/// 下拉菜单单项高度。
const MENU_ITEM_H: i32 = 30;

// ---------------------------------------------------------------- 字段定义

/// 一个配置字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Field {
    Hotkey,
    HideSelf,
    ToolbarCollapsed,
    CopyFormat,
    DefaultDir,
    LongMax,
    LongSearch,
    AiProvider,
    AiModel,
    AiEndpoint,
    AiApiKey,
    AiTimeout,
}

/// 控件类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Edit,
    /// 密码框（`ES_PASSWORD`，系统负责掩码）。
    Password,
    Check,
    Combo,
}

/// 控件定义。
#[derive(Clone, Copy)]
struct Widget {
    field: Field,
    kind: Kind,
    /// 复选框方块右侧的文字（其它类型为空）。
    text: &'static str,
}

/// 行尾附加按钮（如「浏览…」）。
#[derive(Clone, Copy)]
struct Trailing {
    text: &'static str,
    id: usize,
}

/// 一行。
#[derive(Clone, Copy)]
struct Row {
    /// 左侧标签（空 = 控件从标签列开始）。
    label: &'static str,
    widgets: &'static [Widget],
    trailing: Option<Trailing>,
}

/// 一个分组。
#[derive(Clone, Copy)]
struct Section {
    title: &'static str,
    note: &'static str,
    rows: &'static [Row],
}

/// 复制格式下拉项。**顺序即映射**（0 = Both，1 = Png，2 = Dib）。
const COPY_FORMAT_ITEMS: &[&str] = &["PNG + DIB", "仅 PNG", "仅 DIB"];
/// AI 服务方下拉项（0 = minimax，1 = deepseek）。
const AI_PROVIDER_ITEMS: &[&str] = &["MiniMax", "DeepSeek"];

const W_HOTKEY: Widget = Widget {
    field: Field::Hotkey,
    kind: Kind::Edit,
    text: "",
};
const W_HIDE_SELF: Widget = Widget {
    field: Field::HideSelf,
    kind: Kind::Check,
    text: "截图时隐藏本工具窗口",
};
const W_TOOLBAR_COLLAPSED: Widget = Widget {
    field: Field::ToolbarCollapsed,
    kind: Kind::Check,
    text: "工具栏默认折叠",
};
const W_COPY_FORMAT: Widget = Widget {
    field: Field::CopyFormat,
    kind: Kind::Combo,
    text: "",
};
const W_DEFAULT_DIR: Widget = Widget {
    field: Field::DefaultDir,
    kind: Kind::Edit,
    text: "",
};
const W_LONG_MAX: Widget = Widget {
    field: Field::LongMax,
    kind: Kind::Edit,
    text: "",
};
const W_LONG_SEARCH: Widget = Widget {
    field: Field::LongSearch,
    kind: Kind::Edit,
    text: "",
};
const W_AI_PROVIDER: Widget = Widget {
    field: Field::AiProvider,
    kind: Kind::Combo,
    text: "",
};
const W_AI_MODEL: Widget = Widget {
    field: Field::AiModel,
    kind: Kind::Edit,
    text: "",
};
const W_AI_ENDPOINT: Widget = Widget {
    field: Field::AiEndpoint,
    kind: Kind::Edit,
    text: "",
};
const W_AI_KEY: Widget = Widget {
    field: Field::AiApiKey,
    kind: Kind::Password,
    text: "",
};
const W_AI_TIMEOUT: Widget = Widget {
    field: Field::AiTimeout,
    kind: Kind::Edit,
    text: "",
};

const TRAILING_BROWSE: Trailing = Trailing {
    text: "浏览…",
    id: ID_BROWSE,
};

const SECTIONS: &[Section] = &[
    Section {
        title: "常规",
        note: "截图行为",
        rows: &[
            Row {
                label: "截图热键",
                widgets: &[W_HOTKEY],
                trailing: None,
            },
            Row {
                label: "",
                widgets: &[W_HIDE_SELF, W_TOOLBAR_COLLAPSED],
                trailing: None,
            },
            Row {
                label: "复制到剪贴板的格式",
                widgets: &[W_COPY_FORMAT],
                trailing: None,
            },
            Row {
                label: "默认保存目录",
                widgets: &[W_DEFAULT_DIR],
                trailing: Some(TRAILING_BROWSE),
            },
        ],
    },
    Section {
        title: "长截图",
        note: "长度单位：物理像素",
        rows: &[
            Row {
                label: "最大长度（0 = 不限）",
                widgets: &[W_LONG_MAX],
                trailing: None,
            },
            Row {
                label: "搜索窗口（像素）",
                widgets: &[W_LONG_SEARCH],
                trailing: None,
            },
        ],
    },
    Section {
        title: "AI",
        note: "DeepSeek / MiniMax",
        rows: &[
            Row {
                label: "服务方",
                widgets: &[W_AI_PROVIDER],
                trailing: None,
            },
            Row {
                label: "模型",
                widgets: &[W_AI_MODEL],
                trailing: None,
            },
            Row {
                label: "端点",
                widgets: &[W_AI_ENDPOINT],
                trailing: None,
            },
            Row {
                label: "API Key",
                widgets: &[W_AI_KEY],
                trailing: None,
            },
            Row {
                label: "请求超时（秒）",
                widgets: &[W_AI_TIMEOUT],
                trailing: None,
            },
        ],
    },
];

/// 下拉框对应的候选项。
fn combo_items(field: Field) -> &'static [&'static str] {
    match field {
        Field::CopyFormat => COPY_FORMAT_ITEMS,
        Field::AiProvider => AI_PROVIDER_ITEMS,
        _ => &[],
    }
}

/// 焦点顺序（Tab / Shift+Tab 走这个）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FocusItem {
    Field(Field),
    Button(usize),
}

const FOCUS_ITEMS: &[FocusItem] = &[
    FocusItem::Field(Field::Hotkey),
    FocusItem::Field(Field::HideSelf),
    FocusItem::Field(Field::ToolbarCollapsed),
    FocusItem::Field(Field::CopyFormat),
    FocusItem::Field(Field::DefaultDir),
    FocusItem::Field(Field::LongMax),
    FocusItem::Field(Field::LongSearch),
    FocusItem::Field(Field::AiProvider),
    FocusItem::Field(Field::AiModel),
    FocusItem::Field(Field::AiEndpoint),
    FocusItem::Field(Field::AiApiKey),
    FocusItem::Field(Field::AiTimeout),
    FocusItem::Button(ID_BROWSE),
    FocusItem::Button(ID_SAVE),
    FocusItem::Button(ID_CANCEL),
];

// ---------------------------------------------------------------- 布局

/// 一个控件的最终几何。
#[derive(Clone, Copy)]
struct WidgetGeom {
    field: Field,
    kind: Kind,
    /// 控件本体矩形（输入框 / 下拉框 / 复选框方块）。
    rect: Rect,
    /// 复选框方块右侧的文字。
    text: &'static str,
    /// 命中测试矩形（比控件本体大，鼠标点文字也算命中）。
    hit: Rect,
}

/// 一行的最终几何。
#[derive(Clone)]
struct RowGeom {
    label: &'static str,
    label_rect: Rect,
    widgets: Vec<WidgetGeom>,
    trailing: Option<(usize, &'static str, Rect)>,
}

/// 一个分组的面板几何。
#[derive(Clone)]
struct PanelGeom {
    title: &'static str,
    note: &'static str,
    rect: Rect,
}

/// 整体布局。
#[derive(Clone)]
struct Layout {
    panels: Vec<PanelGeom>,
    rows: Vec<RowGeom>,
    /// 客户区高度。
    client_h: i32,
    /// 底部「取消」「保存」按钮矩形。
    cancel: Rect,
    save: Rect,
}

/// 估算文本宽度（逻辑像素）。
///
/// 布局阶段还没有 GDI+ 上下文，只能按字符宽度粗估：
/// 中文/全角按 1 个字号宽，ASCII 按 0.55 个字号宽。
fn est_text_w(text: &str, size: i32) -> i32 {
    let mut w = 0.0f32;
    for ch in text.chars() {
        w += if (ch as u32) > 0x2E80 {
            size as f32
        } else {
            size as f32 * 0.55
        };
    }
    w.round() as i32
}

/// 计算整个界面的布局。
fn compute_layout(k: f32) -> Layout {
    let panel_w = px(WIN_W, k) - px(PAD, k) * 2;
    let inner_x = px(PAD, k) + px(PANEL_PAD_X, k);
    let inner_w = panel_w - px(PANEL_PAD_X, k) * 2;
    let label_w = px(LABEL_W, k);
    let ctrl_x = inner_x + label_w + px(CTRL_GAP, k);
    let ctrl_w = inner_w - label_w - px(CTRL_GAP, k);
    let row_h = px(ROW_H, k);
    let field_h = px(FIELD_H, k);

    let mut panels = Vec::new();
    let mut rows = Vec::new();
    let mut y = px(PAD, k);

    for (si, section) in SECTIONS.iter().enumerate() {
        let panel_y = y;
        let rows_y = panel_y + px(PANEL_PAD_TOP, k) + px(HEADER_H, k);
        let n_rows = section.rows.len() as i32;

        for (n, row) in section.rows.iter().enumerate() {
            let ry = rows_y + n as i32 * (row_h + px(ROW_GAP, k));
            // 有标签 → 控件在控件列；没标签（复选框行）→ 控件从标签列开始
            let (area_x, area_w) = if row.label.is_empty() {
                (inner_x, inner_w)
            } else {
                (ctrl_x, ctrl_w)
            };
            let n_widgets = row.widgets.len().max(1) as i32;

            let mut widgets = Vec::new();
            // 一行只有一个控件 → 占满控件列；多个（复选框）→ 依次紧挨着排
            let mut cursor_x = area_x;
            for w in row.widgets.iter() {
                let (rect, cell, hit_w) = match w.kind {
                    Kind::Check => {
                        let s = px(16, k);
                        let tw = px(est_text_w(w.text, 13), k);
                        let cw = s + px(8, k) + tw;
                        (
                            Rect::new(cursor_x, ry + (row_h - s) / 2, s, s),
                            cw + px(20, k),
                            cw,
                        )
                    }
                    _ => {
                        let cw = if n_widgets == 1 {
                            area_w
                        } else {
                            area_w / n_widgets
                        };
                        (
                            Rect::new(cursor_x, ry + (row_h - field_h) / 2, cw, field_h),
                            cw + px(12, k),
                            cw,
                        )
                    }
                };
                cursor_x += cell;
                widgets.push(WidgetGeom {
                    field: w.field,
                    kind: w.kind,
                    rect,
                    text: w.text,
                    // 命中区比控件本体宽（复选框的文字部分也算命中）
                    hit: Rect::new(rect.x, ry, hit_w, row_h),
                });
            }

            // 行尾按钮（「浏览…」）：输入框要给它让出位置
            let trailing = row.trailing.map(|t| {
                let bw = px(BROWSE_W, k);
                let br = Rect::new(
                    area_x + area_w - bw,
                    ry + (row_h - px(BTN_H, k)) / 2,
                    bw,
                    px(BTN_H, k),
                );
                if let Some(w) = widgets.first_mut() {
                    if matches!(w.kind, Kind::Edit | Kind::Password) {
                        w.rect.w = br.x - px(8, k) - w.rect.x;
                        w.hit.w = w.rect.w;
                    }
                }
                (t.id, t.text, br)
            });

            rows.push(RowGeom {
                label: row.label,
                label_rect: Rect::new(inner_x, ry, label_w, row_h),
                widgets,
                trailing,
            });
        }

        let section_h = px(PANEL_PAD_TOP, k)
            + px(HEADER_H, k)
            + n_rows * row_h
            + (n_rows - 1).max(0) * px(ROW_GAP, k)
            + px(PANEL_PAD_BOTTOM, k);
        panels.push(PanelGeom {
            title: section.title,
            note: section.note,
            rect: Rect::new(px(PAD, k), panel_y, panel_w, section_h),
        });
        y = panel_y + section_h;
        if si + 1 < SECTIONS.len() {
            y += px(SECTION_GAP, k);
        }
    }

    // 底部按钮
    let footer_y = y + px(FOOTER_GAP, k);
    let bw = px(BTN_W, k);
    let bh = px(BTN_H, k);
    let client_w = px(WIN_W, k);
    let save = Rect::new(client_w - px(PAD, k) - bw, footer_y, bw, bh);
    let cancel = Rect::new(save.x - bw - px(8, k), footer_y, bw, bh);

    Layout {
        panels,
        rows,
        client_h: footer_y + bh + px(PAD, k),
        cancel,
        save,
    }
}

// ---------------------------------------------------------------- 窗口状态

/// 下拉菜单状态。
struct ComboState {
    field: Field,
    items: &'static [&'static str],
    selected: usize,
    hover: Option<usize>,
}

struct State {
    /// 设置窗口句柄。
    hwnd: HWND,
    /// 打开时的配置（作为编辑起点）。
    cfg: Config,
    /// 配置文件路径（保存时写回这里）。
    path: PathBuf,
    /// 字段 → 原生 EDIT 句柄。
    edits: HashMap<Field, HWND>,
    /// 字段 → 复选框状态（自绘）。
    checks: HashMap<Field, bool>,
    /// 字段 → 下拉选中项下标（自绘）。
    combos: HashMap<Field, usize>,
    /// 按钮 id → 原生 BUTTON 句柄。
    buttons: HashMap<usize, HWND>,
    /// 布局。
    layout: Layout,
    /// DPI 缩放系数。
    k: f32,
    /// 输入框底色刷子（`WM_CTLCOLOREDIT` 用）。
    field_brush: HBRUSH,
    /// 焦点所在的自绘控件（复选框 / 下拉框）。
    focus_field: Option<Field>,
    /// 焦点所在的原生按钮（焦点环由按钮自己画）。
    focus_button: Option<usize>,
    /// 鼠标悬停的自绘控件。
    hover_field: Option<Field>,
    /// 鼠标悬停的原生按钮 id。
    hover_button: Option<usize>,
    /// 正在按下的按钮 id。
    pressed_button: Option<usize>,
    /// 打开中的下拉菜单。
    combo: Option<ComboState>,
    /// 下拉菜单窗口句柄。
    combo_hwnd: HWND,
    /// 是否点了「保存」。
    saved: bool,
    /// 点「保存」时**当场**读到的配置（必须在销毁窗口前收集）。
    result: Option<Config>,
}

impl State {
    /// 把配置填进控件。
    fn fill(&mut self) {
        for (field, hwnd) in &self.edits {
            let text = match field {
                Field::Hotkey => self.cfg.hotkey.clone(),
                Field::DefaultDir => self
                    .cfg
                    .default_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                Field::LongMax => self.cfg.long_screenshot_max.to_string(),
                Field::LongSearch => self.cfg.long_screenshot_search.to_string(),
                Field::AiModel => self.cfg.ai_model.clone(),
                Field::AiEndpoint => self.cfg.ai_endpoint.clone(),
                Field::AiApiKey => self.cfg.ai_api_key.clone(),
                Field::AiTimeout => self.cfg.ai_timeout.to_string(),
                _ => String::new(),
            };
            set_text(*hwnd, &text);
        }
        self.checks.insert(Field::HideSelf, self.cfg.hide_self);
        self.checks
            .insert(Field::ToolbarCollapsed, self.cfg.toolbar_collapsed);
        self.combos.insert(
            Field::CopyFormat,
            match self.cfg.copy_format {
                CopyFormat::Both => 0,
                CopyFormat::Png => 1,
                CopyFormat::Dib => 2,
            },
        );
        self.combos.insert(
            Field::AiProvider,
            if self.cfg.ai_provider.eq_ignore_ascii_case("deepseek") {
                1
            } else {
                0
            },
        );
    }

    /// 从控件把配置读出来（解析失败的字段保持原值）。
    fn collect(&self) -> Config {
        let mut cfg = self.cfg.clone();
        let t = |f: Field| self.edit_text(f);
        if !t(Field::Hotkey).trim().is_empty() {
            cfg.hotkey = t(Field::Hotkey).trim().to_string();
        }
        cfg.hide_self = *self.checks.get(&Field::HideSelf).unwrap_or(&false);
        cfg.toolbar_collapsed = *self.checks.get(&Field::ToolbarCollapsed).unwrap_or(&false);
        cfg.copy_format = match self.combos.get(&Field::CopyFormat).copied().unwrap_or(0) {
            1 => CopyFormat::Png,
            2 => CopyFormat::Dib,
            _ => CopyFormat::Both,
        };
        let dir = t(Field::DefaultDir);
        cfg.default_dir = if dir.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(dir.trim()))
        };
        if let Ok(v) = t(Field::LongMax).trim().parse::<u32>() {
            cfg.long_screenshot_max = v;
        }
        if let Ok(v) = t(Field::LongSearch).trim().parse::<i32>() {
            cfg.long_screenshot_search = v;
        }
        cfg.ai_provider = match self.combos.get(&Field::AiProvider).copied().unwrap_or(0) {
            1 => "deepseek".to_string(),
            _ => "minimax".to_string(),
        };
        cfg.ai_model = t(Field::AiModel).trim().to_string();
        cfg.ai_endpoint = t(Field::AiEndpoint).trim().to_string();
        cfg.ai_api_key = t(Field::AiApiKey).trim().to_string();
        if let Ok(v) = t(Field::AiTimeout).trim().parse::<u32>() {
            cfg.ai_timeout = v;
        }
        cfg
    }

    /// 读某个输入框的文本。
    fn edit_text(&self, f: Field) -> String {
        self.edits.get(&f).map(|h| get_text(*h)).unwrap_or_default()
    }
}

// ---------------------------------------------------------------- 对外 API

/// 打开设置窗口。
///
/// 返回 `Some(配置)` = 用户点了「保存」（且已写回配置文件）；
/// `None` = 取消。
pub fn open(path: &Path) -> Result<Option<Config>> {
    let cfg = Config::load(path).unwrap_or_default();

    // 应用 `--theme` 覆盖（给了路径才生效；没给就是内置默认）。
    // 必须在建窗**之前**应用，首帧就是覆盖后的配色。
    theme::reload();

    // 必须是**创建窗口之前**设好 DPI 感知：
    // 否则进程是 DPI-unaware，系统会把窗口整体拉伸（文字发虚），
    // 且 GetDpiForWindow 只会返回 96，控件尺寸算出来就不对。
    ensure_dpi_awareness();
    let dpi = system_dpi();
    let k = dpi as f32 / 96.0;

    // Shell 的目录选择框需要 COM
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();

    let mut state = Box::new(State {
        hwnd: HWND::default(),
        cfg,
        path: path.to_path_buf(),
        edits: HashMap::new(),
        checks: HashMap::new(),
        combos: HashMap::new(),
        buttons: HashMap::new(),
        layout: compute_layout(k),
        k,
        field_brush: field_brush(),
        focus_field: None,
        focus_button: None,
        hover_field: None,
        hover_button: None,
        pressed_button: None,
        combo: None,
        combo_hwnd: HWND::default(),
        saved: false,
        result: None,
    });
    let state_ptr = &mut *state as *mut State;

    register_class()?;
    register_popup_class()?;
    register_flat_button_class()?;
    let class = wide(CLASS_NAME);
    let title = wide("screenloom 设置");

    // 客户区尺寸 → 整窗尺寸（必须补标题栏 + 边框，否则底部按钮会被裁掉）
    let (ww, wh) = frame_adjusted(px(WIN_W, k), state.layout.client_h, dpi);

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_DLGMODALFRAME,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ww,
            wh,
            None,
            None,
            Some(module_handle()),
            Some(state_ptr as *const core::ffi::c_void),
        )
    }
    .map_err(|e| Error::Window(format!("创建设置窗口失败: {e}")))?;

    state.hwnd = hwnd;
    enable_dark_title(hwnd);
    build_controls(hwnd, &mut state, k);
    state.fill();
    center(hwnd);

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // 主题热重载：定时查覆盖文件的修改时间
        let _ = SetTimer(Some(hwnd), TIMER_THEME, 400, None);
    }

    // 消息循环
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
        if state.saved || !window_alive(hwnd) {
            break;
        }
    }

    // 注意：`state.result` 是在点「保存」时**当场**从控件读出来的
    // （见窗口过程 ID_SAVE 分支）—— 那时控件还活着。
    // 这里不要再调 collect()，窗口已经销毁，读不到任何值。
    let saved = state.saved;
    let cfg = state.result.take().unwrap_or_else(|| state.cfg.clone());
    if window_alive(hwnd) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    unsafe {
        let _ = DeleteObject(HGDIOBJ(state.field_brush.0));
    }
    if saved {
        if let Err(e) = cfg.save(&state.path) {
            if com {
                unsafe { CoUninitialize() };
            }
            return Err(e);
        }
    }
    if com {
        unsafe { CoUninitialize() };
    }
    Ok(if saved { Some(cfg) } else { None })
}

// ---------------------------------------------------------------- 基础工具

/// 逻辑像素 → 物理像素。
fn px(v: i32, k: f32) -> i32 {
    theme::px(v, k)
}

/// UTF-16（带结尾 NUL）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 进程 DPI 感知（Per-Monitor V2）。
///
/// 必须在**创建任何窗口之前**调用。重复调用会失败，忽略即可。
fn ensure_dpi_awareness() {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }
}

/// 当前系统 DPI（最低按 96 处理）。
fn system_dpi() -> u32 {
    unsafe { GetDpiForSystem() }.max(96)
}

/// 把**客户区**尺寸换算成**窗口**尺寸（补上标题栏 + 边框）。
fn frame_adjusted(cw: i32, ch: i32, dpi: u32) -> (i32, i32) {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: cw,
        bottom: ch,
    };
    unsafe {
        let _ =
            AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WS_EX_DLGMODALFRAME, dpi);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// 深色标题栏（Windows 10 20H1+ / 11）。
fn enable_dark_title(hwnd: HWND) {
    let v: i32 = 1;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &v as *const i32 as *const core::ffi::c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

// ---------------------------------------------------------------- 构建控件

/// 输入框底色刷子（`WM_CTLCOLOREDIT` 用）—— 颜色来自**当前主题**。
fn field_brush() -> HBRUSH {
    let (r, g, b) = theme::rgb(theme::current().field_bg);
    unsafe { CreateSolidBrush(COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)) }
}

/// 主题热重载后重建输入框底色刷子。
fn refresh_field_brush(st: &mut State) {
    unsafe {
        let _ = DeleteObject(HGDIOBJ(st.field_brush.0));
    }
    st.field_brush = field_brush();
}

/// 主题变了：重建画刷 + 让**所有**子窗口重绘。
///
/// 父窗口的 `WM_PAINT` 不覆盖子控件区域 —— 不逐个 invalidate，
/// 子控件（按钮 / 输入框）会保留旧配色。
fn repaint_all(st: &mut State) {
    refresh_field_brush(st);
    invalidate(st);
    for h in st.buttons.values() {
        unsafe {
            let _ = InvalidateRect(Some(*h), None, false);
        }
    }
    for h in st.edits.values() {
        unsafe {
            let _ = InvalidateRect(Some(*h), None, false);
        }
    }
    if !st.combo_hwnd.0.is_null() {
        unsafe {
            let _ = InvalidateRect(Some(st.combo_hwnd), None, false);
        }
    }
}

fn build_controls(parent: HWND, st: &mut State, k: f32) {
    let font = make_font(k);
    let state_ptr = &mut *st as *mut State;

    // 输入框（保持**创建顺序** = 字段顺序，端到端脚本按下标取控件）
    for row in &st.layout.rows {
        for wg in &row.widgets {
            if matches!(wg.kind, Kind::Edit | Kind::Password) {
                let h = create_edit(parent, wg.rect, wg.kind == Kind::Password, font, k);
                st.edits.insert(wg.field, h);
                unsafe {
                    let _ = SetWindowSubclass(h, Some(child_subclass), 1, 0);
                }
            }
        }
    }

    // 行尾按钮（「浏览…」）
    for row in &st.layout.rows {
        if let Some((id, text, rect)) = &row.trailing {
            let h = create_button(parent, text, *rect, *id, font, state_ptr);
            st.buttons.insert(*id, h);
        }
    }

    // 底部按钮
    let save = create_button(parent, "保存", st.layout.save, ID_SAVE, font, state_ptr);
    st.buttons.insert(ID_SAVE, save);
    let cancel = create_button(parent, "取消", st.layout.cancel, ID_CANCEL, font, state_ptr);
    st.buttons.insert(ID_CANCEL, cancel);
}

/// 文本输入框。
///
/// 不加 `WS_BORDER` / `WS_EX_CLIENTEDGE` —— 外框由宿主自绘圆角，
/// 框内底色由 `WM_CTLCOLOREDIT` 设成和自绘底色**完全一致**的深色，
/// 这样矩形输入框和圆角外框看不出接缝。
fn create_edit(parent: HWND, r: Rect, password: bool, font: HFONT, k: f32) -> HWND {
    let class = wide("EDIT");
    let style = WS_CHILD.0
        | WS_VISIBLE.0
        | WS_TABSTOP.0
        | ES_AUTOHSCROLL as u32
        | if password { ES_PASSWORD as u32 } else { 0 };
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(style),
            r.x + px(11, k),
            r.y + px(4, k),
            (r.w - px(22, k)).max(1),
            (r.h - px(8, k)).max(1),
            Some(parent),
            None,
            Some(module_handle()),
            None,
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        );
    }
    h
}

/// 自绘按钮：外观、悬停、按下、焦点环全部自己画。
///
/// * `WS_EX_NOACTIVATE` —— 按钮**永不**持有键盘焦点，
///   否则系统会画一个丑陋的虚线焦点框，而且父窗口重绘不覆盖子控件区域，
///   那个框会一直留在屏幕上。
/// * `BM_CLICK` 照样支持（端到端脚本用它点「保存」）。
fn create_button(
    parent: HWND,
    text: &str,
    r: Rect,
    id: usize,
    font: HFONT,
    state: *mut State,
) -> HWND {
    let class = wide(FLAT_BTN_CLASS);
    let t = wide(text);
    let h = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            PCWSTR(class.as_ptr()),
            PCWSTR(t.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            r.x,
            r.y,
            r.w,
            r.h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            Some(module_handle()),
            Some(state as *const core::ffi::c_void),
        )
    }
    .unwrap_or_default();
    unsafe {
        let _ = SendMessageW(
            h,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(0)),
        );
    }
    h
}

/// 触发一个按钮（等价于原生按钮的 `BN_CLICKED`）。
fn activate_button(parent: HWND, id: usize) {
    unsafe {
        let _ = SendMessageW(
            parent,
            WM_COMMAND,
            Some(WPARAM(id)),
            Some(LPARAM(parent.0 as isize)),
        );
    }
}

/// 界面字体。
fn make_font(k: f32) -> HFONT {
    let h = -px(13, k);
    let face = wide("Microsoft YaHei");
    unsafe {
        CreateFontW(
            h,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32 | FF_DONTCARE.0 as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}

// ---------------------------------------------------------------- 控件读写

fn set_text(hwnd: HWND, text: &str) {
    if hwnd.0.is_null() {
        return;
    }
    let w: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
    }
}

fn get_text(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 1024];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) }.clamp(0, buf.len() as i32 - 1) as usize;
    String::from_utf16_lossy(&buf[..n])
}

// ---------------------------------------------------------------- 杂项

fn register_class() -> Result<()> {
    let name = wide(CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(settings_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        // 不给背景刷：整块客户区都由我们自己画
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册设置窗口类失败".into()));
    }
    Ok(())
}

fn register_popup_class() -> Result<()> {
    let name = wide(POPUP_CLASS_NAME);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(popup_wnd_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册下拉菜单窗口类失败".into()));
    }
    Ok(())
}

fn register_flat_button_class() -> Result<()> {
    let name = wide(FLAT_BTN_CLASS);
    let mut existing = WNDCLASSEXW::default();
    if unsafe { GetClassInfoExW(Some(module_handle()), PCWSTR(name.as_ptr()), &mut existing) }
        .is_ok()
    {
        return Ok(());
    }
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(flat_button_proc),
        hInstance: module_handle(),
        lpszClassName: PCWSTR(name.as_ptr()),
        hbrBackground: HBRUSH(std::ptr::null_mut()),
        ..WNDCLASSEXW::default()
    };
    if unsafe { RegisterClassExW(&wc) } == 0 {
        return Err(Error::Window("注册自绘按钮窗口类失败".into()));
    }
    Ok(())
}

/// 窗口是否还活着。
fn window_alive(hwnd: HWND) -> bool {
    unsafe { IsWindow(Some(hwnd)) }.as_bool()
}

fn module_handle() -> windows::Win32::Foundation::HINSTANCE {
    windows::Win32::Foundation::HINSTANCE(
        unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
            .unwrap_or_default()
            .0,
    )
}

/// 把窗口摆到主显示器中央。
fn center(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let x = (sw - w) / 2;
    let y = (sh - h) / 2;
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// 目录选择框（SHBrowseForFolder）。
fn pick_folder(hwnd: HWND) -> Option<PathBuf> {
    let title = wide("选择截图保存目录");
    let bi = BROWSEINFOW {
        hwndOwner: hwnd,
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_NEWDIALOGSTYLE | BIF_RETURNONLYFSDIRS | BIF_EDITBOX,
        ..BROWSEINFOW::default()
    };
    unsafe {
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let ok = SHGetPathFromIDListW(pidl, &mut buf).as_bool();
        CoTaskMemFree(Some(pidl as *const core::ffi::c_void));
        if !ok {
            return None;
        }
        let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }
}

// ---------------------------------------------------------------- 绘制

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    (r.right - r.left, r.bottom - r.top)
}

/// 画整个界面（双缓冲）。
fn paint(hwnd: HWND, st: &State) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
    let (w, h) = client_size(hwnd);
    if w <= 0 || h <= 0 {
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        return;
    }

    let mem = unsafe { CreateCompatibleDC(Some(hdc)) };
    let bmp = unsafe { CreateCompatibleBitmap(hdc, w, h) };
    if !mem.0.is_null() && !bmp.0.is_null() {
        unsafe {
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            if let Ok(mut g) = Graphics::from_hdc(mem) {
                g.setup_quality();
                draw_scene(&mut g, st, w, h);
            }
            let _ = BitBlt(hdc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
            let _ = SelectObject(mem, old);
        }
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
        }
    }

    unsafe {
        let _ = EndPaint(hwnd, &ps);
    }
}

/// 画界面内容（不含原生按钮 —— 那些走 owner-draw）。
fn draw_scene(g: &mut Graphics, st: &State, w: i32, h: i32) {
    let k = st.k;
    g.fill_rect(0.0, 0.0, w as f32, h as f32, theme::current().bg);

    // 分组面板 + 标题
    for p in &st.layout.panels {
        theme::panel(g, p.rect, k);
        theme::section_header(
            g,
            p.rect.x + px(PANEL_PAD_X, k),
            p.rect.y + px(PANEL_PAD_TOP, k),
            p.title,
            p.note,
            k,
        );
    }

    // 行
    let focused_edit = focused_edit(st);
    for row in &st.layout.rows {
        if !row.label.is_empty() {
            let f = px(13, k) as f32;
            let (_, th) = g.measure_text(row.label, f, false);
            let ty =
                row.label_rect.y as f32 + (row.label_rect.h as f32 - th) / 2.0 - px(1, k) as f32;
            g.draw_text(
                row.label_rect.x as f32,
                ty,
                row.label,
                f,
                theme::current().text_dim,
                false,
            );
        }
        for wg in &row.widgets {
            match wg.kind {
                Kind::Edit | Kind::Password => {
                    let focused = focused_edit == Some(wg.field);
                    let hovered = st.hover_field == Some(wg.field);
                    theme::input_frame(g, wg.rect, focused, hovered, k);
                }
                Kind::Check => {
                    let on = *st.checks.get(&wg.field).unwrap_or(&false);
                    let hov = st.hover_field == Some(wg.field);
                    theme::checkbox(g, wg.rect.x, wg.rect.y, wg.rect.w, on, hov, k);
                    // 文字
                    let f = px(13, k) as f32;
                    let tx = wg.rect.right() + px(8, k);
                    let (_, th) = g.measure_text(wg.text, f, false);
                    let ty = wg.rect.y as f32 + (wg.rect.h as f32 - th) / 2.0 - px(1, k) as f32;
                    g.draw_text(tx as f32, ty, wg.text, f, theme::current().text, false);
                    if st.focus_field == Some(wg.field) {
                        theme::focus_ring(g, wg.hit, k);
                    }
                }
                Kind::Combo => {
                    let sel = st.combos.get(&wg.field).copied().unwrap_or(0);
                    let text = combo_items(wg.field).get(sel).copied().unwrap_or("");
                    let open = st
                        .combo
                        .as_ref()
                        .map(|c| c.field == wg.field)
                        .unwrap_or(false);
                    theme::combo_box(
                        g,
                        wg.rect,
                        text,
                        open,
                        st.hover_field == Some(wg.field),
                        st.focus_field == Some(wg.field),
                        k,
                    );
                }
            }
        }
    }
}

/// 当前获得键盘焦点的输入框字段。
fn focused_edit(st: &State) -> Option<Field> {
    let h = unsafe { GetFocus() };
    if h.0.is_null() {
        return None;
    }
    st.edits.iter().find(|(_, v)| **v == h).map(|(k, _)| *k)
}

// ---------------------------------------------------------------- 命中测试

/// 命中一个自绘控件。
fn hit_widget(st: &State, x: i32, y: i32) -> Option<Field> {
    for row in &st.layout.rows {
        for wg in &row.widgets {
            if matches!(wg.kind, Kind::Check | Kind::Combo) && wg.hit.contains(x, y) {
                return Some(wg.field);
            }
        }
    }
    None
}

// ---------------------------------------------------------------- 焦点

/// 当前焦点项在 FOCUS_ITEMS 里的下标。
fn focus_index(st: &State) -> Option<usize> {
    if let Some(f) = st.focus_field {
        return FOCUS_ITEMS.iter().position(|i| *i == FocusItem::Field(f));
    }
    if let Some(id) = st.focus_button {
        return FOCUS_ITEMS.iter().position(|i| *i == FocusItem::Button(id));
    }
    let h = unsafe { GetFocus() };
    if h.0.is_null() {
        return None;
    }
    FOCUS_ITEMS.iter().position(|item| match item {
        FocusItem::Field(f) => st.edits.get(f) == Some(&h),
        FocusItem::Button(id) => st.buttons.get(id) == Some(&h),
    })
}

/// 把焦点移到第 idx 项。
fn focus_to(st: &mut State, idx: usize) {
    let item = match FOCUS_ITEMS.get(idx) {
        Some(i) => *i,
        None => return,
    };
    match item {
        FocusItem::Field(f) => {
            st.focus_button = None;
            if let Some(h) = st.edits.get(&f).copied() {
                st.focus_field = None;
                unsafe {
                    let _ = SetFocus(Some(h));
                }
            } else {
                st.focus_field = Some(f);
                unsafe {
                    let _ = SetFocus(Some(st.hwnd));
                }
            }
        }
        FocusItem::Button(id) => {
            // 按钮**不真正持有焦点**（焦点环自己画），
            // 否则系统会画一个丑陋的虚线焦点框。
            st.focus_field = None;
            st.focus_button = Some(id);
            unsafe {
                let _ = SetFocus(Some(st.hwnd));
            }
        }
    }
    invalidate(st);
}

fn invalidate(st: &State) {
    unsafe {
        let _ = InvalidateRect(Some(st.hwnd), None, false);
    }
}

// ---------------------------------------------------------------- 下拉菜单

/// 打开某个下拉框。
fn open_combo(st: &mut State, field: Field) {
    let items = combo_items(field);
    if items.is_empty() {
        return;
    }
    // 找到控件矩形
    let mut ctrl = None;
    for row in &st.layout.rows {
        for wg in &row.widgets {
            if wg.field == field && wg.kind == Kind::Combo {
                ctrl = Some(wg.rect);
            }
        }
    }
    let Some(ctrl) = ctrl else { return };

    let k = st.k;
    let item_h = px(MENU_ITEM_H, k);
    let menu_h = items.len() as i32 * item_h + px(8, k);
    let menu_w = ctrl.w;

    // 客户区 → 屏幕坐标（下方放不下就往上开）
    let mut below = POINT {
        x: ctrl.x,
        y: ctrl.bottom(),
    };
    let mut above = POINT {
        x: ctrl.x,
        y: ctrl.y,
    };
    unsafe {
        let _ = ClientToScreen(st.hwnd, &mut below);
        let _ = ClientToScreen(st.hwnd, &mut above);
    }
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let (menu_x, menu_y) = if below.y + menu_h > screen_h {
        (above.x, above.y - menu_h - px(4, k))
    } else {
        (below.x, below.y + px(4, k))
    };

    st.combo = Some(ComboState {
        field,
        items,
        selected: st.combos.get(&field).copied().unwrap_or(0),
        hover: None,
    });

    // 创建 / 复用弹出窗口
    let class = wide(POPUP_CLASS_NAME);
    let hwnd = if st.combo_hwnd.0.is_null() {
        let h = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                menu_x,
                menu_y,
                menu_w,
                menu_h,
                Some(st.hwnd),
                None,
                Some(module_handle()),
                Some(&mut *st as *mut State as *const core::ffi::c_void),
            )
        }
        .unwrap_or_default();
        st.combo_hwnd = h;
        h
    } else {
        st.combo_hwnd
    };

    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            menu_x,
            menu_y,
            menu_w,
            menu_h,
            SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
    }
    invalidate(st);
}

/// 关掉下拉菜单。
fn close_combo(st: &mut State) {
    st.combo = None;
    if !st.combo_hwnd.0.is_null() && window_alive(st.combo_hwnd) {
        unsafe {
            let _ = DestroyWindow(st.combo_hwnd);
        }
    }
    st.combo_hwnd = HWND::default();
    unsafe {
        let _ = SetFocus(Some(st.hwnd));
    }
    invalidate(st);
}

// ---------------------------------------------------------------- 子控件子类化

/// 原生输入框的子类过程：Tab / 回车 / Esc 统一交给宿主，焦点变化通知重画。
unsafe extern "system" fn child_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _uid: usize,
    _refdata: usize,
) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            if vk == VK_TAB.0 as u32 {
                let dir = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                    -1i32
                } else {
                    1i32
                };
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(
                    Some(parent),
                    WM_APP_FOCUS_NEXT,
                    WPARAM(dir as usize),
                    LPARAM(0),
                );
                return LRESULT(0);
            }
            if vk == VK_ESCAPE.0 as u32 {
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(Some(parent), WM_APP_CANCEL, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
            // 回车 = 保存
            if vk == VK_RETURN.0 as u32 {
                let parent = GetParent(hwnd).unwrap_or_default();
                let _ = PostMessageW(Some(parent), WM_APP_SAVE, WPARAM(0), LPARAM(0));
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            // Tab 不要被插进文本框
            if wp.0 == 9 {
                return LRESULT(0);
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let parent = GetParent(hwnd).unwrap_or_default();
            let _ = PostMessageW(Some(parent), WM_APP_REDRAW, WPARAM(0), LPARAM(0));
            if msg == WM_SETFOCUS {
                set_light_caret(hwnd);
            }
        }
        WM_MOUSEMOVE => {
            // 鼠标不在按钮上了
            let parent = GetParent(hwnd).unwrap_or_default();
            let _ = PostMessageW(Some(parent), WM_APP_HOVER_CLEAR, WPARAM(0), LPARAM(0));
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// 深色输入框需要浅色插入符，否则黑底上根本看不见。
fn set_light_caret(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
    }
    let h = (r.bottom - r.top).max(4);
    let brush = unsafe { CreateSolidBrush(COLORREF(0x00_FF_FF_FF)) };
    unsafe {
        let _ = CreateCaret(hwnd, Some(HBITMAP(brush.0)), 1, h);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

// ---------------------------------------------------------------- 自绘按钮窗口过程

/// 自绘按钮的窗口过程。
///
/// 按钮自己画外观、自己处理悬停 / 按下 / 释放，
/// `BM_CLICK` 也照样响应（端到端脚本靠它点「保存」）。
unsafe extern "system" fn flat_button_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut State;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;
    let id = GetDlgCtrlID(hwnd) as usize;

    match msg {
        WM_ERASEBKGND => return LRESULT(1),

        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            if let Ok(mut g) = Graphics::from_hdc(hdc) {
                g.setup_quality();
                let rect = Rect::new(0, 0, r.right - r.left, r.bottom - r.top);
                let style = if id == ID_SAVE {
                    theme::BtnStyle::Primary
                } else {
                    theme::BtnStyle::Secondary
                };
                let state = if st.pressed_button == Some(id) {
                    theme::BtnState::Pressed
                } else if st.hover_button == Some(id) {
                    theme::BtnState::Hover
                } else {
                    theme::BtnState::Normal
                };
                // 按钮背后：行尾按钮在面板上，底部按钮在窗口底色上
                let behind = {
                    let t = theme::current();
                    if id == ID_BROWSE {
                        t.panel
                    } else {
                        t.bg
                    }
                };
                let text = get_text(hwnd);
                theme::button(&mut g, rect, &text, style, state, st.k, behind);
                if st.focus_button == Some(id) {
                    theme::focus_ring(&mut g, rect, st.k);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }

        WM_MOUSEMOVE => {
            if st.hover_button != Some(id) {
                st.hover_button = Some(id);
                invalidate(st);
            }
            return LRESULT(0);
        }

        WM_LBUTTONDOWN => {
            st.pressed_button = Some(id);
            unsafe {
                let _ = SetCapture(hwnd);
            }
            invalidate(st);
            return LRESULT(0);
        }

        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let pressed = st.pressed_button.take();
            st.pressed_button = None;
            invalidate(st);
            let mut r = RECT::default();
            let _ = GetClientRect(hwnd, &mut r);
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            if pressed == Some(id) && x >= 0 && y >= 0 && x < r.right && y < r.bottom {
                activate_button(st.hwnd, id);
            }
            return LRESULT(0);
        }

        // `BM_CLICK`：和原生按钮一样，外部脚本可以拿它来点按钮
        BM_CLICK => {
            activate_button(st.hwnd, id);
            return LRESULT(0);
        }

        WM_SETFOCUS | WM_KILLFOCUS => {
            // 按钮不持有焦点（WS_EX_NOACTIVATE），焦点环由宿主自绘
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

// ---------------------------------------------------------------- 窗口过程

unsafe extern "system" fn settings_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut State;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;

    match msg {
        // 自己画背景：擦除阶段什么都不做（避免闪烁）
        WM_ERASEBKGND => return LRESULT(1),

        WM_PAINT => {
            paint(hwnd, st);
            return LRESULT(0);
        }

        // 输入框：深色底 + 浅色字（颜色来自当前主题，热重载后会跟着变）
        WM_CTLCOLOREDIT => {
            let hdc = HDC(wp.0 as *mut core::ffi::c_void);
            let (tr, tg, tb) = theme::rgb(theme::current().text);
            let (br, bg, bb) = theme::rgb(theme::current().field_bg);
            unsafe {
                SetTextColor(
                    hdc,
                    COLORREF(tr as u32 | (tg as u32) << 8 | (tb as u32) << 16),
                );
                SetBkColor(
                    hdc,
                    COLORREF(br as u32 | (bg as u32) << 8 | (bb as u32) << 16),
                );
            }
            return LRESULT(st.field_brush.0 as isize);
        }

        // 主题热重载：覆盖文件改了 → 重新应用 + 全部重绘
        WM_TIMER => {
            if wp.0 == TIMER_THEME && theme::source_changed() && theme::reload() {
                repaint_all(st);
            }
            return LRESULT(0);
        }

        WM_COMMAND => {
            let id = (wp.0 as u32 & 0xFFFF) as usize;
            match id {
                ID_SAVE => {
                    // **先读控件值再销毁窗口**：DestroyWindow 之后子控件就没了，
                    // GetWindowTextW 只能拿到空串，用户刚改的值会全丢。
                    if let Some(msg) = validate(st) {
                        let t = wide(&msg);
                        let title = wide("screenloom 设置");
                        unsafe {
                            let _ = MessageBoxW(
                                Some(hwnd),
                                PCWSTR(t.as_ptr()),
                                PCWSTR(title.as_ptr()),
                                MB_OK | MB_ICONWARNING,
                            );
                        }
                        return LRESULT(0);
                    }
                    st.result = Some(st.collect());
                    st.saved = true;
                }
                ID_CANCEL => {
                    st.saved = false;
                }
                ID_BROWSE => {
                    if let Some(dir) = pick_folder(hwnd) {
                        if let Some(edit) = st.edits.get(&Field::DefaultDir).copied() {
                            set_text(edit, &dir.display().to_string());
                        }
                    }
                    return LRESULT(0);
                }
                _ => return LRESULT(0),
            }
            // 「保存」和「取消」都是关窗
            if st.saved || id == ID_CANCEL {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }

        // ---- 自绘控件的鼠标 ----
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let hit = hit_widget(st, x, y);
            if hit != st.hover_field {
                st.hover_field = hit;
                invalidate(st);
            }
            if st.hover_button.is_some() {
                st.hover_button = None;
                invalidate(st);
            }
            return LRESULT(0);
        }

        WM_LBUTTONDOWN => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            // 点到自绘控件外面 → 收起下拉 + 去掉自绘焦点
            if let Some(hit) = hit_widget(st, x, y) {
                match kind_of(st, hit) {
                    Kind::Check => {
                        let cur = *st.checks.get(&hit).unwrap_or(&false);
                        st.checks.insert(hit, !cur);
                        st.focus_field = Some(hit);
                        unsafe {
                            let _ = SetFocus(Some(hwnd));
                        }
                        invalidate(st);
                    }
                    Kind::Combo => {
                        st.focus_field = Some(hit);
                        if st.combo.as_ref().map(|c| c.field == hit).unwrap_or(false) {
                            close_combo(st);
                        } else {
                            open_combo(st, hit);
                        }
                    }
                    _ => {}
                }
            } else {
                if st.combo.is_some() {
                    close_combo(st);
                }
                st.focus_field = None;
                st.focus_button = None;
                unsafe {
                    let _ = SetFocus(Some(hwnd));
                }
                invalidate(st);
            }
            return LRESULT(0);
        }

        // ---- 键盘（焦点在自绘控件上时）----
        WM_KEYDOWN => {
            let vk = wp.0 as u32;
            if vk == VK_TAB.0 as u32 {
                let dir = if GetKeyState(VK_SHIFT.0 as i32) < 0 {
                    -1i32
                } else {
                    1i32
                };
                move_focus(st, dir);
                return LRESULT(0);
            }
            if vk == VK_ESCAPE.0 as u32 {
                st.saved = false;
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            if vk == VK_RETURN.0 as u32 {
                if let Some(id) = st.focus_button {
                    activate_button(hwnd, id);
                    return LRESULT(0);
                }
                if st.combo.is_some() {
                    close_combo(st);
                } else {
                    st.result = Some(st.collect());
                    st.saved = true;
                    let _ = DestroyWindow(hwnd);
                }
                return LRESULT(0);
            }
            if vk == VK_SPACE.0 as u32 {
                if let Some(id) = st.focus_button {
                    activate_button(hwnd, id);
                    return LRESULT(0);
                }
                if let Some(f) = st.focus_field {
                    if kind_of(st, f) == Kind::Check {
                        let cur = *st.checks.get(&f).unwrap_or(&false);
                        st.checks.insert(f, !cur);
                        invalidate(st);
                    }
                }
                return LRESULT(0);
            }
            if vk == VK_UP.0 as u32 || vk == VK_DOWN.0 as u32 {
                if let Some(f) = st.focus_field {
                    if kind_of(st, f) == Kind::Combo {
                        let items = combo_items(f);
                        if !items.is_empty() {
                            let cur = st.combos.get(&f).copied().unwrap_or(0);
                            let next = if vk == VK_UP.0 as u32 {
                                cur.saturating_sub(1)
                            } else {
                                (cur + 1).min(items.len() - 1)
                            };
                            st.combos.insert(f, next);
                            invalidate(st);
                        }
                    }
                }
                return LRESULT(0);
            }
        }

        // ---- 自定义消息 ----
        WM_APP_FOCUS_NEXT => {
            move_focus(st, wp.0 as i32);
            return LRESULT(0);
        }
        WM_APP_SAVE => {
            if st.combo.is_some() {
                close_combo(st);
            } else {
                st.result = Some(st.collect());
                st.saved = true;
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_APP_CANCEL => {
            st.saved = false;
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        WM_APP_REDRAW => {
            invalidate(st);
            return LRESULT(0);
        }
        WM_APP_HOVER_CLEAR => {
            if st.hover_button.is_some() {
                st.hover_button = None;
                invalidate(st);
            }
            return LRESULT(0);
        }
        WM_APP_COMBO_SELECT => {
            let idx = wp.0;
            if let Some(c) = &st.combo {
                let field = c.field;
                let sel = idx.min(c.items.len().saturating_sub(1));
                st.combos.insert(field, sel);
            }
            close_combo(st);
            return LRESULT(0);
        }
        WM_APP_COMBO_CLOSED => {
            if st.combo.is_some() {
                close_combo(st);
            }
            return LRESULT(0);
        }

        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
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

/// 下拉菜单窗口过程。
unsafe extern "system" fn popup_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = lp.0 as *const CREATESTRUCTW;
        if !cs.is_null() {
            let param = (*cs).lpCreateParams as *mut State;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, param as isize);
        }
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let st = &mut *ptr;

    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let (w, h) = client_size(hwnd);
            if let Ok(mut g) = Graphics::from_hdc(hdc) {
                g.setup_quality();
                if let Some(c) = &st.combo {
                    let r = Rect::new(0, 0, w, h);
                    let item_h = px(MENU_ITEM_H, st.k);
                    theme::menu(&mut g, r, c.items, c.selected, c.hover, item_h, st.k);
                }
            }
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let idx = item_index(st, x, y);
            if let Some(c) = &mut st.combo {
                c.hover = idx;
            }
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            let x = (lp.0 as i16) as i32;
            let y = ((lp.0 >> 16) as i16) as i32;
            let idx = item_index(st, x, y);
            if let Some(i) = idx {
                unsafe {
                    let _ = PostMessageW(Some(st.hwnd), WM_APP_COMBO_SELECT, WPARAM(i), LPARAM(0));
                }
            }
            return LRESULT(0);
        }
        WM_KILLFOCUS => {
            unsafe {
                let _ = PostMessageW(Some(st.hwnd), WM_APP_COMBO_CLOSED, WPARAM(0), LPARAM(0));
            }
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// 下拉菜单里第几项（坐标 → 下标）。
fn item_index(st: &State, _x: i32, y: i32) -> Option<usize> {
    let c = st.combo.as_ref()?;
    let item_h = px(MENU_ITEM_H, st.k);
    let top = px(4, st.k);
    if y < top {
        return None;
    }
    let i = ((y - top) / item_h) as usize;
    if i < c.items.len() {
        Some(i)
    } else {
        None
    }
}

/// 字段对应的控件类型。
fn kind_of(st: &State, field: Field) -> Kind {
    for row in &st.layout.rows {
        for wg in &row.widgets {
            if wg.field == field {
                return wg.kind;
            }
        }
    }
    Kind::Edit
}

/// 移动焦点（dir = +1 / -1）。
fn move_focus(st: &mut State, dir: i32) {
    let n = FOCUS_ITEMS.len() as i32;
    let cur = focus_index(st).map(|v| v as i32).unwrap_or(-1);
    let mut next = cur + dir;
    if next < 0 {
        next = n - 1;
    }
    if next >= n {
        next = 0;
    }
    focus_to(st, next as usize);
}

/// 数字字段的合法范围。
const LONG_SEARCH_MIN: u32 = 8;
const LONG_SEARCH_MAX: u32 = 4000;
const LONG_MAX_LIMIT: u32 = 200_000;
const AI_TIMEOUT_MAX: u32 = 3600;

/// 校验一个数字字段；返回 `Some(提示)` 表示不合法。
///
/// 这些值会直接进算法 —— 不夹住就会出问题：
/// `long_screenshot_search` 是搜索距离（代价 `O(search * band * cols)`），
/// 填个超大值能让长截图**卡死**；`ai_timeout` 太大则请求永远挂着。
fn check_numeric(field: Field, text: &str) -> Option<String> {
    let label = match field {
        Field::LongMax => "「长截图最大长度」",
        Field::LongSearch => "「长截图搜索窗口」",
        Field::AiTimeout => "「AI 请求超时」",
        _ => return None,
    };
    let t = text.trim();
    let Ok(v) = t.parse::<u32>() else {
        return Some(format!("{label}必须是数字"));
    };
    match field {
        Field::LongMax if v > LONG_MAX_LIMIT => {
            Some(format!("{label}不能超过 {LONG_MAX_LIMIT} 像素"))
        }
        Field::LongSearch if !(LONG_SEARCH_MIN..=LONG_SEARCH_MAX).contains(&v) => Some(format!(
            "{label}必须在 {LONG_SEARCH_MIN} ~ {LONG_SEARCH_MAX} 之间"
        )),
        Field::AiTimeout if v == 0 || v > AI_TIMEOUT_MAX => {
            Some(format!("{label}必须在 1 ~ {AI_TIMEOUT_MAX} 秒之间"))
        }
        _ => None,
    }
}

/// 保存前的校验；返回 Some(提示) 表示不合法。
fn validate(st: &State) -> Option<String> {
    if st.edit_text(Field::Hotkey).trim().is_empty() {
        return Some("截图热键不能为空".to_string());
    }
    for f in [Field::LongMax, Field::LongSearch, Field::AiTimeout] {
        if let Some(msg) = check_numeric(f, &st.edit_text(f)) {
            return Some(msg);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_has_a_widget() {
        let mut seen = std::collections::HashSet::new();
        for s in SECTIONS {
            for r in s.rows {
                for w in r.widgets {
                    assert!(seen.insert(w.field), "字段重复: {:?}", w.field);
                }
            }
        }
        // 每个字段都必须出现在焦点顺序里
        for f in [
            Field::Hotkey,
            Field::HideSelf,
            Field::ToolbarCollapsed,
            Field::CopyFormat,
            Field::DefaultDir,
            Field::LongMax,
            Field::LongSearch,
            Field::AiProvider,
            Field::AiModel,
            Field::AiEndpoint,
            Field::AiApiKey,
            Field::AiTimeout,
        ] {
            assert!(seen.contains(&f), "字段没有界面: {:?}", f);
            assert!(
                FOCUS_ITEMS.contains(&FocusItem::Field(f)),
                "字段不在焦点顺序里: {:?}",
                f
            );
        }
        assert_eq!(seen.len(), 12, "字段数量变了");
    }

    #[test]
    fn numeric_fields_are_range_checked() {
        // 合法值
        assert!(check_numeric(Field::LongSearch, "240").is_none());
        assert!(check_numeric(Field::LongMax, "0").is_none());
        assert!(check_numeric(Field::LongMax, "8888").is_none());
        assert!(check_numeric(Field::AiTimeout, "120").is_none());

        // 非数字 / 负数
        assert!(check_numeric(Field::LongSearch, "abc").is_some());
        assert!(check_numeric(Field::LongSearch, "-5").is_some());
        assert!(check_numeric(Field::AiTimeout, "0").is_some());

        // 超大值必须被挡住（否则长截图会卡死）
        assert!(check_numeric(Field::LongSearch, "999999").is_some());
        assert!(check_numeric(Field::LongMax, "999999999").is_some());
        assert!(check_numeric(Field::AiTimeout, "999999").is_some());

        // 边界内 / 边界外
        assert!(check_numeric(Field::LongSearch, "8").is_none());
        assert!(check_numeric(Field::LongSearch, "4000").is_none());
        assert!(check_numeric(Field::LongSearch, "7").is_some());
        assert!(check_numeric(Field::LongSearch, "4001").is_some());
    }

    #[test]
    fn combo_items_match_expected_order() {
        assert_eq!(COPY_FORMAT_ITEMS, &["PNG + DIB", "仅 PNG", "仅 DIB"]);
        assert_eq!(AI_PROVIDER_ITEMS, &["MiniMax", "DeepSeek"]);
    }

    #[test]
    fn checkbox_widgets_carry_their_own_text() {
        // 复选框的文字画在方块右边，所以必须自带文字。
        for s in SECTIONS {
            for r in s.rows {
                for w in r.widgets {
                    if w.kind == Kind::Check {
                        assert!(!w.text.is_empty(), "复选框必须自带文字: {:?}", w.field);
                    }
                }
            }
        }
    }

    #[test]
    fn layout_is_self_consistent() {
        // 布局的纵向推进必须和实际画出来的一致：
        // 面板底边不能越过下一个面板顶边，按钮不能越出客户区。
        let k = 1.25f32;
        let l = compute_layout(k);
        for pair in l.panels.windows(2) {
            assert!(pair[0].rect.bottom() <= pair[1].rect.y, "面板之间重叠了");
        }
        assert!(l.save.bottom() <= l.client_h, "保存按钮超出客户区");
        assert!(l.cancel.bottom() <= l.client_h, "取消按钮超出客户区");
        assert!(l.client_h > 400 && l.client_h < 1200, "客户区高度不合理");
    }

    #[test]
    fn layout_scales_with_dpi() {
        let a = compute_layout(1.0);
        let b = compute_layout(1.25);
        assert!(b.client_h > a.client_h, "高 DPI 下高度应变大");
        // 1.25 倍时高度约等于 1.25 倍（允许取整误差）
        let ratio = b.client_h as f32 / a.client_h as f32;
        assert!((ratio - 1.25).abs() < 0.03, "缩放比例不对: {ratio}");
    }

    #[test]
    fn window_size_includes_title_bar_and_border() {
        // 回归测试：`CreateWindowExW` 的宽高是**整窗**尺寸，
        // 而布局算的是**客户区**高度。
        let l = compute_layout(1.0);
        let (win_w, win_h) = frame_adjusted(px(WIN_W, 1.0), l.client_h, 96);
        assert!(win_w >= px(WIN_W, 1.0));
        assert!(
            win_h > l.client_h,
            "窗口高度必须大于客户区高度（标题栏+边框没算进去）"
        );
    }

    #[test]
    fn browse_button_reserves_space_for_the_edit() {
        // 「默认保存目录」这一行：输入框必须给「浏览…」按钮让出位置，
        // 否则两者会叠在一起。
        let l = compute_layout(1.0);
        let row = l
            .rows
            .iter()
            .find(|r| r.trailing.is_some())
            .expect("没有带行尾按钮的行");
        let (_, _, btn) = row.trailing.unwrap();
        let edit = row.widgets.first().expect("没有输入框");
        assert!(
            edit.rect.right() <= btn.x,
            "输入框和行尾按钮重叠：{} >= {}",
            edit.rect.right(),
            btn.x
        );
    }

    #[test]
    fn edits_are_created_in_field_order() {
        // 端到端脚本按下标取 EDIT（0 = 热键，2 = 长截图上限），
        // 创建顺序必须和字段顺序一致，否则脚本会改错字段。
        let l = compute_layout(1.0);
        let mut order: Vec<Field> = Vec::new();
        for row in &l.rows {
            for w in &row.widgets {
                if matches!(w.kind, Kind::Edit | Kind::Password) {
                    order.push(w.field);
                }
            }
        }
        assert_eq!(
            order,
            vec![
                Field::Hotkey,
                Field::DefaultDir,
                Field::LongMax,
                Field::LongSearch,
                Field::AiModel,
                Field::AiEndpoint,
                Field::AiApiKey,
                Field::AiTimeout,
            ]
        );
    }
}
