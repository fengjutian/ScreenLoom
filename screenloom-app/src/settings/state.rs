//! 窗口状态与配置绑定：`State` / `ComboState`，以及配置 ↔ 控件的双向搬运。

use super::*;

// ---------------------------------------------------------------- 窗口状态

/// 下拉菜单状态。
pub(crate) struct ComboState {
    pub(crate) field: Field,
    pub(crate) items: &'static [&'static str],
    pub(crate) selected: usize,
    pub(crate) hover: Option<usize>,
}

pub(crate) struct State {
    /// 设置窗口句柄。
    pub(crate) hwnd: HWND,
    /// 打开时的配置（作为编辑起点）。
    pub(crate) cfg: Config,
    /// 配置文件路径（保存时写回这里）。
    pub(crate) path: PathBuf,
    /// 字段 → 原生 EDIT 句柄。
    pub(crate) edits: HashMap<Field, HWND>,
    /// 字段 → 复选框状态（自绘）。
    pub(crate) checks: HashMap<Field, bool>,
    /// 字段 → 下拉选中项下标（自绘）。
    pub(crate) combos: HashMap<Field, usize>,
    /// 按钮 id → 原生 BUTTON 句柄。
    pub(crate) buttons: HashMap<usize, HWND>,
    /// 布局。
    pub(crate) layout: Layout,
    /// DPI 缩放系数。
    pub(crate) k: f32,
    /// 输入框底色刷子（`WM_CTLCOLOREDIT` 用）。
    pub(crate) field_brush: HBRUSH,
    /// 焦点所在的自绘控件（复选框 / 下拉框）。
    pub(crate) focus_field: Option<Field>,
    /// 焦点所在的原生按钮（焦点环由按钮自己画）。
    pub(crate) focus_button: Option<usize>,
    /// 鼠标悬停的自绘控件。
    pub(crate) hover_field: Option<Field>,
    /// 鼠标悬停的原生按钮 id。
    pub(crate) hover_button: Option<usize>,
    /// 正在按下的按钮 id。
    pub(crate) pressed_button: Option<usize>,
    /// 打开中的下拉菜单。
    pub(crate) combo: Option<ComboState>,
    /// 下拉菜单窗口句柄。
    pub(crate) combo_hwnd: HWND,
    /// 是否点了「保存」。
    pub(crate) saved: bool,
    /// 点「保存」时**当场**读到的配置（必须在销毁窗口前收集）。
    pub(crate) result: Option<Config>,
}

impl State {
    /// 把配置填进控件。
    pub(crate) fn fill(&mut self) {
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
        self.checks.insert(Field::PrivacyMode, self.cfg.privacy_mode);
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
    pub(crate) fn collect(&self) -> Config {
        let mut cfg = self.cfg.clone();
        let t = |f: Field| self.edit_text(f);
        if !t(Field::Hotkey).trim().is_empty() {
            cfg.hotkey = t(Field::Hotkey).trim().to_string();
        }
        cfg.hide_self = *self.checks.get(&Field::HideSelf).unwrap_or(&false);
        cfg.privacy_mode = *self.checks.get(&Field::PrivacyMode).unwrap_or(&false);
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
    pub(crate) fn edit_text(&self, f: Field) -> String {
        self.edits.get(&f).map(|h| get_text(*h)).unwrap_or_default()
    }
}
