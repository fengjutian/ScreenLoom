//! 保存前的字段校验（数值范围 + 必填项）。

use super::*;

/// 数字字段的合法范围。
pub(crate) const LONG_SEARCH_MIN: u32 = 8;
pub(crate) const LONG_SEARCH_MAX: u32 = 4000;
pub(crate) const LONG_MAX_LIMIT: u32 = 200_000;
pub(crate) const AI_TIMEOUT_MAX: u32 = 3600;

/// 校验一个数字字段；返回 `Some(提示)` 表示不合法。
///
/// 这些值会直接进算法 —— 不夹住就会出问题：
/// `long_screenshot_search` 是搜索距离（代价 `O(search * band * cols)`），
/// 填个超大值能让长截图**卡死**；`ai_timeout` 太大则请求永远挂着。
pub(crate) fn check_numeric(field: Field, text: &str) -> Option<String> {
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
pub(crate) fn validate(st: &State) -> Option<String> {
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
