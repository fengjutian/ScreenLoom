//! 运行时表单模型：字段、控件、访问器，以及两个表单的构造函数。
//!
//! 设计见 `doc/设置表单引擎与滚动方案.md`（ADR-001）。
//! 加一个字段 = 一个 [`Field`] 变体 + 一条 [`Row`] + 两个小函数（get / set）。
//!
//! 字段的「显示值 / 写回配置 / 校验」三件事跟着字段走，而不是散在
//! `fill` / `collect` / `validate` 三个 `match` 里 —— 这是本模块存在的理由。

use std::path::PathBuf;

use screenloom_capture::MonitorInfo;
use screenloom_core::{Config, CopyFormat};

use super::*;

// ---------------------------------------------------------------- 校验边界

/// 长截图搜索窗口（物理像素）下界。
pub(crate) const LONG_SEARCH_MIN: i32 = 8;
/// 长截图搜索窗口（物理像素）上界。
pub(crate) const LONG_SEARCH_MAX: i32 = 4000;
/// 长截图单次最大长度上限（物理像素）。超过会让拼接卡死。
pub(crate) const LONG_MAX_LIMIT: u32 = 200_000;
/// AI 请求超时上限（秒）。
pub(crate) const AI_TIMEOUT_MAX: u32 = 3600;

// ---------------------------------------------------------------- 字段与控件

/// 全部可编辑字段：截图 13 + 录屏 9。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum Field {
    // ---- 截图 ----
    Hotkey,
    HideSelf,
    PrivacyMode,
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
    // ---- 录屏 ----
    Fps,
    Bitrate,
    Countdown,
    RecordDir,
    Monitor,
    Cursor,
    Hud,
    OpenFolder,
}

impl Field {
    /// 字段全集（单测用：保证每个字段都有界面）。
    #[cfg(test)]
    pub(crate) const ALL: &'static [Field] = &[
        Field::Hotkey,
        Field::HideSelf,
        Field::PrivacyMode,
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
        Field::Fps,
        Field::Bitrate,
        Field::Countdown,
        Field::RecordDir,
        Field::Monitor,
        Field::Cursor,
        Field::Hud,
        Field::OpenFolder,
    ];
}

/// 控件类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// 单行文本（数字字段也是它，范围校验在 `TextSet` 里）。
    Edit,
    /// 密码框（`ES_PASSWORD`，系统负责掩码）。
    Password,
    /// 复选框（自绘）。
    Check,
    /// 下拉框（自绘）。
    Combo,
}

/// 表单种类：决定窗口类名、标题与字段集合。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FormKind {
    Capture,
    Record,
}

impl FormKind {
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            FormKind::Capture => CLASS_NAME,
            FormKind::Record => RECORD_CLASS_NAME,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            FormKind::Capture => "screenloom 设置",
            FormKind::Record => "screenloom 录屏设置",
        }
    }
}

// ---------------------------------------------------------------- 访问器

pub(crate) type TextGet = fn(&Config) -> String;
/// 写回配置并校验。`Err` = 给用户看的提示（**不落盘、保留原值**）。
pub(crate) type TextSet = fn(&mut Config, &str) -> std::result::Result<(), String>;
pub(crate) type BoolGet = fn(&Config) -> bool;
pub(crate) type BoolSet = fn(&mut Config, bool);
/// 第二个参数是候选项个数（动态下拉需要它来夹下标）。
pub(crate) type ChoiceGet = fn(&Config, usize) -> usize;
pub(crate) type ChoiceSet = fn(&mut Config, usize);

/// 一个字段的读写方式。
pub(crate) enum Access {
    Text { get: TextGet, set: TextSet },
    Bool { get: BoolGet, set: BoolSet },
    Choice {
        items: Vec<String>,
        get: ChoiceGet,
        set: ChoiceSet,
    },
}

/// 一个控件。
pub(crate) struct Widget {
    pub(crate) field: Field,
    pub(crate) kind: Kind,
    /// 复选框方块右侧的文字（其它类型为空）。
    pub(crate) text: &'static str,
    pub(crate) access: Access,
}

/// 行尾附加按钮（如「浏览…」）。
#[derive(Clone, Copy)]
pub(crate) struct Trailing {
    pub(crate) text: &'static str,
    pub(crate) id: usize,
}

/// 一行。
pub(crate) struct Row {
    /// 左侧标签（空 = 控件从标签列开始，复选框行用）。
    pub(crate) label: &'static str,
    pub(crate) widgets: Vec<Widget>,
    pub(crate) trailing: Option<Trailing>,
}

/// 一个分组。
pub(crate) struct Section {
    pub(crate) title: &'static str,
    pub(crate) note: &'static str,
    pub(crate) rows: Vec<Row>,
}

/// 一张完整的表单。
pub(crate) struct Form {
    pub(crate) kind: FormKind,
    pub(crate) sections: Vec<Section>,
}

impl Form {
    /// 按界面顺序遍历所有控件。
    pub(crate) fn widgets(&self) -> impl Iterator<Item = &Widget> {
        self.sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .flat_map(|r| r.widgets.iter())
    }

    pub(crate) fn widget(&self, field: Field) -> Option<&Widget> {
        self.widgets().find(|w| w.field == field)
    }

    /// 下拉框的候选项（非下拉字段返回空）。
    pub(crate) fn choice_items(&self, field: Field) -> &[String] {
        match self.widget(field).map(|w| &w.access) {
            Some(Access::Choice { items, .. }) => items,
            _ => &[],
        }
    }

    /// Tab 焦点顺序：控件按界面顺序 → 行尾按钮 → 保存 → 取消。
    pub(crate) fn focus_items(&self) -> Vec<FocusItem> {
        let mut v: Vec<FocusItem> = self
            .widgets()
            .map(|w| FocusItem::Field(w.field))
            .collect();
        for s in &self.sections {
            for r in &s.rows {
                if let Some(t) = &r.trailing {
                    v.push(FocusItem::Button(t.id));
                }
            }
        }
        v.push(FocusItem::Button(ID_SAVE));
        v.push(FocusItem::Button(ID_CANCEL));
        v
    }
}

/// 焦点项。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FocusItem {
    Field(Field),
    Button(usize),
}

// ---------------------------------------------------------------- 构造工具

fn text(field: Field, get: TextGet, set: TextSet) -> Widget {
    Widget {
        field,
        kind: Kind::Edit,
        text: "",
        access: Access::Text { get, set },
    }
}

fn password(field: Field, get: TextGet, set: TextSet) -> Widget {
    Widget {
        field,
        kind: Kind::Password,
        text: "",
        access: Access::Text { get, set },
    }
}

fn check(
    field: Field,
    label: &'static str,
    get: BoolGet,
    set: BoolSet,
) -> Widget {
    Widget {
        field,
        kind: Kind::Check,
        text: label,
        access: Access::Bool { get, set },
    }
}

fn choice(
    field: Field,
    items: Vec<String>,
    get: ChoiceGet,
    set: ChoiceSet,
) -> Widget {
    Widget {
        field,
        kind: Kind::Combo,
        text: "",
        access: Access::Choice { items, get, set },
    }
}

fn row(label: &'static str, widgets: Vec<Widget>) -> Row {
    Row {
        label,
        widgets,
        trailing: None,
    }
}

/// 复制格式下拉项。**顺序即映射**（0 = Both，1 = Png，2 = Dib）。
pub(crate) const COPY_FORMAT_ITEMS: &[&str] = &["PNG + DIB", "仅 PNG", "仅 DIB"];
/// AI 服务方下拉项（0 = minimax，1 = deepseek）。
pub(crate) const AI_PROVIDER_ITEMS: &[&str] = &["MiniMax", "DeepSeek"];

fn static_items(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ---------------------------------------------------------------- 截图表单

pub(crate) fn capture_form(_cfg: &Config) -> Form {
    let sections = vec![
        Section {
            title: "常规",
            note: "截图行为",
            rows: vec![
                row("截图热键", vec![text(Field::Hotkey, get_hotkey, set_hotkey)]),
                row(
                    "",
                    vec![
                        check(
                            Field::HideSelf,
                            "截图时隐藏本窗口",
                            get_hide_self,
                            set_hide_self,
                        ),
                        check(Field::PrivacyMode, "隐私模式", get_privacy, set_privacy),
                        check(
                            Field::ToolbarCollapsed,
                            "工具栏折叠",
                            get_collapsed,
                            set_collapsed,
                        ),
                    ],
                ),
                row(
                    "复制到剪贴板的格式",
                    vec![choice(
                        Field::CopyFormat,
                        static_items(COPY_FORMAT_ITEMS),
                        get_copy_format,
                        set_copy_format,
                    )],
                ),
                Row {
                    label: "默认保存目录",
                    widgets: vec![text(Field::DefaultDir, get_default_dir, set_default_dir)],
                    trailing: Some(Trailing {
                        text: "浏览…",
                        id: ID_BROWSE,
                    }),
                },
            ],
        },
        Section {
            title: "长截图",
            note: "长度单位：物理像素",
            rows: vec![
                row(
                    "最大长度（0 = 不限）",
                    vec![text(Field::LongMax, get_long_max, set_long_max)],
                ),
                row(
                    "搜索窗口（像素）",
                    vec![text(Field::LongSearch, get_long_search, set_long_search)],
                ),
            ],
        },
        Section {
            title: "AI",
            note: "DeepSeek / MiniMax",
            rows: vec![
                row(
                    "服务方",
                    vec![choice(
                        Field::AiProvider,
                        static_items(AI_PROVIDER_ITEMS),
                        get_provider,
                        set_provider,
                    )],
                ),
                row("模型", vec![text(Field::AiModel, get_model, set_model)]),
                row(
                    "端点",
                    vec![text(Field::AiEndpoint, get_endpoint, set_endpoint)],
                ),
                row(
                    "API Key",
                    vec![password(Field::AiApiKey, get_api_key, set_api_key)],
                ),
                row(
                    "请求超时（秒）",
                    vec![text(Field::AiTimeout, get_ai_timeout, set_ai_timeout)],
                ),
            ],
        },
    ];
    Form {
        kind: FormKind::Capture,
        sections,
    }
}

// ---------------------------------------------------------------- 录屏表单

pub(crate) fn record_form(_cfg: &Config, monitors: &[MonitorInfo]) -> Form {
    let mut monitor_items = vec!["主显示器（自动）".to_string()];
    for (i, m) in monitors.iter().enumerate() {
        monitor_items.push(format!(
            "显示器 {} · {}x{} · {:.0}%",
            i,
            m.bounds.w,
            m.bounds.h,
            m.scale * 100.0
        ));
    }

    let sections = vec![
        Section {
            title: "录制",
            note: "H.264 / MP4",
            rows: vec![
                row("录制帧率（1–120）", vec![text(Field::Fps, get_fps, set_fps)]),
                row(
                    "码率（Mbps）",
                    vec![text(Field::Bitrate, get_bitrate, set_bitrate)],
                ),
                row(
                    "开始前倒计时（秒）",
                    vec![text(Field::Countdown, get_countdown, set_countdown)],
                ),
                row(
                    "录制显示器",
                    vec![choice(
                        Field::Monitor,
                        monitor_items,
                        get_monitor,
                        set_monitor,
                    )],
                ),
            ],
        },
        Section {
            title: "输出",
            note: "默认 Videos\\Screenloom",
            rows: vec![Row {
                label: "输出目录（空 = 视频文件夹）",
                widgets: vec![text(Field::RecordDir, get_record_dir, set_record_dir)],
                trailing: Some(Trailing {
                    text: "浏览…",
                    id: ID_BROWSE,
                }),
            }],
        },
        Section {
            title: "行为",
            note: "录制中",
            rows: vec![
                row(
                    "",
                    vec![
                        check(Field::Cursor, "录制鼠标指针", get_cursor, set_cursor),
                        check(
                            Field::Hud,
                            "录制时显示 HUD（计时/暂停/停止）",
                            get_hud,
                            set_hud,
                        ),
                    ],
                ),
                row(
                    "",
                    vec![check(
                        Field::OpenFolder,
                        "结束后打开所在文件夹",
                        get_open_folder,
                        set_open_folder,
                    )],
                ),
            ],
        },
    ];
    Form {
        kind: FormKind::Record,
        sections,
    }
}

// ---------------------------------------------------------------- 取值/写值

fn parse_u32(label: &str, s: &str) -> std::result::Result<u32, String> {
    s.trim()
        .parse::<u32>()
        .map_err(|_| format!("{label}必须是数字"))
}

fn get_hotkey(c: &Config) -> String {
    c.hotkey.clone()
}

fn set_hotkey(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("截图热键不能为空".to_string());
    }
    c.hotkey = t.to_string();
    Ok(())
}

fn get_hide_self(c: &Config) -> bool {
    c.hide_self
}

fn set_hide_self(c: &mut Config, v: bool) {
    c.hide_self = v;
}

fn get_privacy(c: &Config) -> bool {
    c.privacy_mode
}

fn set_privacy(c: &mut Config, v: bool) {
    c.privacy_mode = v;
}

fn get_collapsed(c: &Config) -> bool {
    c.toolbar_collapsed
}

fn set_collapsed(c: &mut Config, v: bool) {
    c.toolbar_collapsed = v;
}

fn get_copy_format(c: &Config, _len: usize) -> usize {
    match c.copy_format {
        CopyFormat::Both => 0,
        CopyFormat::Png => 1,
        CopyFormat::Dib => 2,
    }
}

fn set_copy_format(c: &mut Config, i: usize) {
    c.copy_format = match i {
        1 => CopyFormat::Png,
        2 => CopyFormat::Dib,
        _ => CopyFormat::Both,
    };
}

fn get_default_dir(c: &Config) -> String {
    c.default_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

fn set_default_dir(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let t = s.trim();
    c.default_dir = if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    };
    Ok(())
}

fn get_long_max(c: &Config) -> String {
    c.long_screenshot_max.to_string()
}

fn set_long_max(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = parse_u32("「长截图最大长度」", s)?;
    if v > LONG_MAX_LIMIT {
        return Err(format!("「长截图最大长度」不能超过 {LONG_MAX_LIMIT} 像素"));
    }
    c.long_screenshot_max = v;
    Ok(())
}

fn get_long_search(c: &Config) -> String {
    c.long_screenshot_search.to_string()
}

fn set_long_search(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = s
        .trim()
        .parse::<i32>()
        .map_err(|_| "「长截图搜索窗口」必须是数字".to_string())?;
    if !(LONG_SEARCH_MIN..=LONG_SEARCH_MAX).contains(&v) {
        return Err(format!(
            "「长截图搜索窗口」必须在 {LONG_SEARCH_MIN} ~ {LONG_SEARCH_MAX} 之间"
        ));
    }
    c.long_screenshot_search = v;
    Ok(())
}

fn get_provider(c: &Config, _len: usize) -> usize {
    usize::from(c.ai_provider.eq_ignore_ascii_case("deepseek"))
}

fn set_provider(c: &mut Config, i: usize) {
    c.ai_provider = if i == 1 { "deepseek" } else { "minimax" }.to_string();
}

fn get_model(c: &Config) -> String {
    c.ai_model.clone()
}

fn set_model(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    c.ai_model = s.trim().to_string();
    Ok(())
}

fn get_endpoint(c: &Config) -> String {
    c.ai_endpoint.clone()
}

fn set_endpoint(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    c.ai_endpoint = s.trim().to_string();
    Ok(())
}

fn get_api_key(c: &Config) -> String {
    c.ai_api_key.clone()
}

fn set_api_key(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    c.ai_api_key = s.trim().to_string();
    Ok(())
}

fn get_ai_timeout(c: &Config) -> String {
    c.ai_timeout.to_string()
}

fn set_ai_timeout(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = parse_u32("「AI 请求超时」", s)?;
    if v == 0 || v > AI_TIMEOUT_MAX {
        return Err(format!(
            "「AI 请求超时」必须在 1 ~ {AI_TIMEOUT_MAX} 秒之间"
        ));
    }
    c.ai_timeout = v;
    Ok(())
}

// ---- 录屏 ----

fn get_fps(c: &Config) -> String {
    c.record_fps.to_string()
}

fn set_fps(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = parse_u32("「录制帧率」", s)?;
    if !(1..=120).contains(&v) {
        return Err("「录制帧率」必须在 1 ~ 120 之间".to_string());
    }
    c.record_fps = v;
    Ok(())
}

fn get_bitrate(c: &Config) -> String {
    format!("{:.1}", c.record_bitrate as f64 / 1_000_000.0)
}

fn set_bitrate(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = s
        .trim()
        .parse::<f64>()
        .map_err(|_| "「码率」必须是数字".to_string())?;
    if !(0.1..=100.0).contains(&v) {
        return Err("「码率」必须在 0.1 ~ 100 Mbps 之间".to_string());
    }
    c.record_bitrate = (v * 1_000_000.0).round() as u32;
    Ok(())
}

fn get_countdown(c: &Config) -> String {
    c.record_countdown.to_string()
}

fn set_countdown(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let v = parse_u32("「倒计时」", s)?;
    if v > 10 {
        return Err("「倒计时」不能超过 10 秒".to_string());
    }
    c.record_countdown = v;
    Ok(())
}

fn get_record_dir(c: &Config) -> String {
    c.record_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

fn set_record_dir(c: &mut Config, s: &str) -> std::result::Result<(), String> {
    let t = s.trim();
    c.record_dir = if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    };
    Ok(())
}

fn get_monitor(c: &Config, len: usize) -> usize {
    if c.record_monitor >= 0 {
        (c.record_monitor as usize + 1).min(len.saturating_sub(1))
    } else {
        0
    }
}

fn set_monitor(c: &mut Config, i: usize) {
    c.record_monitor = if i == 0 { -1 } else { i as i32 - 1 };
}

fn get_cursor(c: &Config) -> bool {
    c.record_cursor
}

fn set_cursor(c: &mut Config, v: bool) {
    c.record_cursor = v;
}

fn get_hud(c: &Config) -> bool {
    c.record_show_hud
}

fn set_hud(c: &mut Config, v: bool) {
    c.record_show_hud = v;
}

fn get_open_folder(c: &Config) -> bool {
    c.record_open_folder
}

fn set_open_folder(c: &mut Config, v: bool) {
    c.record_open_folder = v;
}
