//! 简单的运行时语言判定与文案切换（中文 / 英文）。

use std::sync::OnceLock;

static IS_ZH: OnceLock<bool> = OnceLock::new();

/// 当前系统语言是否为中文（以 `zh` 开头视为中文）。
pub fn is_zh() -> bool {
    *IS_ZH.get_or_init(|| {
        sys_locale::get_locale().unwrap_or_else(|| "en-US".to_string()).to_lowercase().starts_with("zh")
    })
}

/// 根据当前语言返回中文或英文文案。
pub fn lang_text<'a>(zh: &'a str, en: &'a str) -> &'a str {
    if is_zh() { zh } else { en }
}
