//! 设置的**静态模型**与**布局计算**：字段定义、分组/行/控件常量、几何与 `compute_layout`。
//!
//! 纯数据 + 纯算术，不碰 Win32（只用到 `Rect` 与 [`px`]）。

use super::*;

// ---------------------------------------------------------------- 尺寸常量（逻辑像素）

/// 客户区宽度。
pub(crate) const WIN_W: i32 = 640;
/// 客户区四周留白。
pub(crate) const PAD: i32 = 20;
/// 面板内边距（左右）。
pub(crate) const PANEL_PAD_X: i32 = 18;
/// 面板内边距（上）。
pub(crate) const PANEL_PAD_TOP: i32 = 10;
/// 面板内边距（下）。
pub(crate) const PANEL_PAD_BOTTOM: i32 = 14;
/// 分组标题高度。
pub(crate) const HEADER_H: i32 = 30;
/// 行高。
pub(crate) const ROW_H: i32 = 30;
/// 行间距。
pub(crate) const ROW_GAP: i32 = 6;
/// 分组间距。
pub(crate) const SECTION_GAP: i32 = 12;
/// 标签列宽。
pub(crate) const LABEL_W: i32 = 196;
/// 标签列与控件列间距。
pub(crate) const CTRL_GAP: i32 = 16;
/// 底部按钮区与最后一个分组的间距。
pub(crate) const FOOTER_GAP: i32 = 16;
/// 输入框高度。
pub(crate) const FIELD_H: i32 = 30;
/// 按钮高度。
pub(crate) const BTN_H: i32 = 32;
/// 按钮宽度。
pub(crate) const BTN_W: i32 = 96;
/// 「浏览…」按钮宽度。
pub(crate) const BROWSE_W: i32 = 76;
/// 下拉菜单单项高度。
pub(crate) const MENU_ITEM_H: i32 = 30;

// ---------------------------------------------------------------- 字段定义

/// 一个配置字段。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum Field {
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
}

/// 控件类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Edit,
    /// 密码框（`ES_PASSWORD`，系统负责掩码）。
    Password,
    Check,
    Combo,
}

/// 控件定义。
#[derive(Clone, Copy)]
pub(crate) struct Widget {
    pub(crate) field: Field,
    pub(crate) kind: Kind,
    /// 复选框方块右侧的文字（其它类型为空）。
    pub(crate) text: &'static str,
}

/// 行尾附加按钮（如「浏览…」）。
#[derive(Clone, Copy)]
pub(crate) struct Trailing {
    pub(crate) text: &'static str,
    pub(crate) id: usize,
}

/// 一行。
#[derive(Clone, Copy)]
pub(crate) struct Row {
    /// 左侧标签（空 = 控件从标签列开始）。
    pub(crate) label: &'static str,
    pub(crate) widgets: &'static [Widget],
    pub(crate) trailing: Option<Trailing>,
}

/// 一个分组。
#[derive(Clone, Copy)]
pub(crate) struct Section {
    pub(crate) title: &'static str,
    pub(crate) note: &'static str,
    pub(crate) rows: &'static [Row],
}

/// 复制格式下拉项。**顺序即映射**（0 = Both，1 = Png，2 = Dib）。
pub(crate) const COPY_FORMAT_ITEMS: &[&str] = &["PNG + DIB", "仅 PNG", "仅 DIB"];
/// AI 服务方下拉项（0 = minimax，1 = deepseek）。
pub(crate) const AI_PROVIDER_ITEMS: &[&str] = &["MiniMax", "DeepSeek"];

pub(crate) const W_HOTKEY: Widget = Widget {
    field: Field::Hotkey,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_HIDE_SELF: Widget = Widget {
    field: Field::HideSelf,
    kind: Kind::Check,
    text: "截图时隐藏本工具窗口",
};
pub(crate) const W_PRIVACY_MODE: Widget = Widget {
    field: Field::PrivacyMode,
    kind: Kind::Check,
    text: "隐私模式：本工具窗口不出现在别人的截图里",
};
pub(crate) const W_TOOLBAR_COLLAPSED: Widget = Widget {
    field: Field::ToolbarCollapsed,
    kind: Kind::Check,
    text: "工具栏默认折叠",
};
pub(crate) const W_COPY_FORMAT: Widget = Widget {
    field: Field::CopyFormat,
    kind: Kind::Combo,
    text: "",
};
pub(crate) const W_DEFAULT_DIR: Widget = Widget {
    field: Field::DefaultDir,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_LONG_MAX: Widget = Widget {
    field: Field::LongMax,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_LONG_SEARCH: Widget = Widget {
    field: Field::LongSearch,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_AI_PROVIDER: Widget = Widget {
    field: Field::AiProvider,
    kind: Kind::Combo,
    text: "",
};
pub(crate) const W_AI_MODEL: Widget = Widget {
    field: Field::AiModel,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_AI_ENDPOINT: Widget = Widget {
    field: Field::AiEndpoint,
    kind: Kind::Edit,
    text: "",
};
pub(crate) const W_AI_KEY: Widget = Widget {
    field: Field::AiApiKey,
    kind: Kind::Password,
    text: "",
};
pub(crate) const W_AI_TIMEOUT: Widget = Widget {
    field: Field::AiTimeout,
    kind: Kind::Edit,
    text: "",
};

pub(crate) const TRAILING_BROWSE: Trailing = Trailing {
    text: "浏览…",
    id: ID_BROWSE,
};

pub(crate) const SECTIONS: &[Section] = &[
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
pub(crate) fn combo_items(field: Field) -> &'static [&'static str] {
    match field {
        Field::CopyFormat => COPY_FORMAT_ITEMS,
        Field::AiProvider => AI_PROVIDER_ITEMS,
        _ => &[],
    }
}

/// 焦点顺序（Tab / Shift+Tab 走这个）。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusItem {
    Field(Field),
    Button(usize),
}

pub(crate) const FOCUS_ITEMS: &[FocusItem] = &[
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
pub(crate) struct WidgetGeom {
    pub(crate) field: Field,
    pub(crate) kind: Kind,
    /// 控件本体矩形（输入框 / 下拉框 / 复选框方块）。
    pub(crate) rect: Rect,
    /// 复选框方块右侧的文字。
    pub(crate) text: &'static str,
    /// 命中测试矩形（比控件本体大，鼠标点文字也算命中）。
    pub(crate) hit: Rect,
}

/// 一行的最终几何。
#[derive(Clone)]
pub(crate) struct RowGeom {
    pub(crate) label: &'static str,
    pub(crate) label_rect: Rect,
    pub(crate) widgets: Vec<WidgetGeom>,
    pub(crate) trailing: Option<(usize, &'static str, Rect)>,
}

/// 一个分组的面板几何。
#[derive(Clone)]
pub(crate) struct PanelGeom {
    pub(crate) title: &'static str,
    pub(crate) note: &'static str,
    pub(crate) rect: Rect,
}

/// 整体布局。
#[derive(Clone)]
pub(crate) struct Layout {
    pub(crate) panels: Vec<PanelGeom>,
    pub(crate) rows: Vec<RowGeom>,
    /// 客户区高度。
    pub(crate) client_h: i32,
    /// 底部「取消」「保存」按钮矩形。
    pub(crate) cancel: Rect,
    pub(crate) save: Rect,
}

/// 估算文本宽度（逻辑像素）。
///
/// 布局阶段还没有 GDI+ 上下文，只能按字符宽度粗估：
/// 中文/全角按 1 个字号宽，ASCII 按 0.55 个字号宽。
pub(crate) fn est_text_w(text: &str, size: i32) -> i32 {
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
pub(crate) fn compute_layout(k: f32) -> Layout {
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
