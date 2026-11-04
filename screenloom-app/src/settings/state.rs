//! 窗口状态与配置绑定。
//!
//! 唯一真值是 `draft: Config`：复选框 / 下拉框立即写回，文本在保存与失焦时提交。
//! 不再维护 `checks` / `combos` 两张表，也不需要「销毁窗口前收集」那套技巧。

use super::*;

/// 打开中的下拉菜单。
pub(crate) struct ComboState {
    pub(crate) field: Field,
    pub(crate) items: Vec<String>,
    pub(crate) selected: usize,
    pub(crate) hover: Option<usize>,
}

pub(crate) struct State {
    /// 设置窗口句柄。
    pub(crate) hwnd: HWND,
    /// 这张表单的种类（截图 / 录屏）。
    pub(crate) kind: FormKind,
    /// 表单模型。
    pub(crate) form: Form,
    /// 编辑中的配置（唯一真值）。
    pub(crate) draft: Config,
    /// 配置文件路径（保存时写回这里）。
    pub(crate) path: PathBuf,
    /// 布局（内容坐标）。
    pub(crate) layout: Layout,
    /// 滚动状态。
    pub(crate) scroll: Scroll,
    /// DPI 缩放系数。
    pub(crate) k: f32,
    /// 字段 → 原生 `EDIT` 句柄。
    pub(crate) edits: HashMap<Field, HWND>,
    /// 按钮 id → 自绘按钮句柄。
    pub(crate) buttons: HashMap<usize, HWND>,
    /// 输入框底色刷子（`WM_CTLCOLOREDIT` 用）。
    pub(crate) field_brush: HBRUSH,
    /// 焦点所在的自绘控件（复选框 / 下拉框）。
    pub(crate) focus_field: Option<Field>,
    /// 焦点所在的原生按钮。
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
    /// 保存时得到的配置（提交后从 `draft` 取出）。
    pub(crate) result: Option<Config>,
}

impl State {
    pub(crate) fn widget(&self, f: Field) -> Option<&Widget> {
        self.form.widget(f)
    }

    pub(crate) fn kind_of(&self, f: Field) -> Kind {
        self.widget(f).map(|w| w.kind).unwrap_or(Kind::Edit)
    }

    pub(crate) fn choice_items(&self, f: Field) -> &[String] {
        self.form.choice_items(f)
    }

    /// 读取某个文本控件的当前内容。
    pub(crate) fn text_of(&self, f: Field) -> String {
        self.edits.get(&f).map(|h| get_text(*h)).unwrap_or_default()
    }

    pub(crate) fn bool_of(&self, f: Field) -> bool {
        match self.widget(f).map(|w| &w.access) {
            Some(Access::Bool { get, .. }) => get(&self.draft),
            _ => false,
        }
    }

    pub(crate) fn choice_of(&self, f: Field) -> usize {
        let len = self.choice_items(f).len();
        match self.widget(f).map(|w| &w.access) {
            Some(Access::Choice { get, .. }) => get(&self.draft, len),
            _ => 0,
        }
    }

    pub(crate) fn set_bool(&mut self, f: Field, v: bool) {
        if let Some(Access::Bool { set, .. }) = self.widget(f).map(|w| &w.access) {
            set(&mut self.draft, v);
        }
    }

    pub(crate) fn set_choice(&mut self, f: Field, i: usize) {
        if let Some(Access::Choice { set, .. }) = self.widget(f).map(|w| &w.access) {
            set(&mut self.draft, i);
        }
    }

    /// 所有文本字段（顺序即界面顺序）。
    fn text_fields(&self) -> Vec<Field> {
        self.form
            .widgets()
            .filter(|w| matches!(&w.access, Access::Text { .. }))
            .map(|w| w.field)
            .collect()
    }

    /// 用 `draft` 初始化所有文本控件（建窗后调用一次）。
    pub(crate) fn fill_text(&mut self) {
        for f in self.text_fields() {
            if let Some(Access::Text { get, .. }) = self.widget(f).map(|w| &w.access) {
                let text = get(&self.draft);
                if let Some(h) = self.edits.get(&f).copied() {
                    set_text(h, &text);
                }
            }
        }
    }

    /// 保存前校验：对每个文本字段跑一次「解析干跑」（写在 clone 上）。
    /// 返回第一条给用户看的提示。
    pub(crate) fn validate(&self) -> Option<String> {
        let mut probe = self.draft.clone();
        for f in self.text_fields() {
            if let Some(Access::Text { set, .. }) = self.widget(f).map(|w| &w.access) {
                if let Err(e) = set(&mut probe, &self.text_of(f)) {
                    return Some(e);
                }
            }
        }
        None
    }

    /// 把文本控件的值提交进 `draft`（非法值保留原值，不打断其它字段）。
    pub(crate) fn commit_text(&mut self) {
        for f in self.text_fields() {
            let text = self.text_of(f);
            if let Some(Access::Text { set, .. }) = self.widget(f).map(|w| &w.access) {
                let _ = set(&mut self.draft, &text);
            }
        }
    }

    /// 提交并取出最终配置。
    pub(crate) fn take_result(&mut self) -> Config {
        self.commit_text();
        self.draft.clone()
    }
}
