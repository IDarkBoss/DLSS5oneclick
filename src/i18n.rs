//! 语言支持（rust-i18n）。
//!
//! 翻译文件在仓库根 `locales/*.yml`（`_version: 2`：每个扁平点分 key 下
//! 按 `en:` / `zh-CN:` 给值），由 `main.rs` 的 `i18n!("locales", fallback = "en")`
//! 编译期嵌入 exe——发布单文件不附带翻译。运行时切换用 [`set`]，对全部
//! 线程立即生效（rust-i18n 的 locale 是全局原子量，线程缓存按版本失效）。
//!
//! 选择顺序：`--lang=` 旗标 > settings.json 的 `language` > Windows 界面语言
//! （中文系统给 `zh-CN`，其余 `en`）。缺省与缺失 key 的回退都是 `en`。
//!
//! 测试约定：单测里**不要** `set_locale`（全局量会污染并行测试）；断言
//! 文案时用 `t!("key", locale = "en")` 或直接断言英文——`en` 既是默认值
//! 也是回退值，两重保证下测试稳定。

/// 支持的语言（也是语言切换控件的出现顺序）。
pub const LOCALES: &[&str] = &["en", "zh-CN"];
/// 语言在切换控件里的自称，永远以各自语言书写，不参与翻译。
pub const NATIVE_NAMES: &[&str] = &["English", "简体中文"];

const LANG_CHINESE: u32 = 0x04;

/// locale 代码是否被支持。
pub fn is_valid(locale: &str) -> bool {
    LOCALES.contains(&locale)
}

/// 设置全局语言；未知代码返回 false，由调用方报错。
pub fn set(locale: &str) -> bool {
    if !is_valid(locale) {
        return false;
    }
    rust_i18n::set_locale(locale);
    true
}

/// 当前全局 locale。
pub fn current() -> String {
    rust_i18n::locale().to_string()
}

/// Windows 界面语言是中文（简体、繁体均算）时用 `zh-CN`，其余 `en`。
pub fn detect() -> &'static str {
    #[cfg(windows)]
    {
        // GetUserDefaultUILanguage 返回 LANGID，低 10 位是主语言；0x04 = LANG_CHINESE。
        let lang = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
        if u32::from(lang) & 0x3ff == LANG_CHINESE {
            return "zh-CN";
        }
    }
    "en"
}

/// 启动时调用一次：`--lang=` 优先，其次 settings.json，最后系统语言。
/// `--lang` 给了不认识的代码时在双语提示后退出（此时全局 locale 还没定，
/// 只能两种语言都打出来）。
pub fn init(args: &[String]) {
    if let Some(l) = args.iter().find_map(|a| a.strip_prefix("--lang=")) {
        if set(l) {
            return;
        }
        eprintln!(
            "错误：--lang 只接受 en 或 zh-CN（收到 {l:?}）。\n\
             Error: --lang accepts only en or zh-CN (got {l:?})."
        );
        std::process::exit(2);
    }
    let saved = crate::settings::Settings::load().language;
    match saved.as_deref() {
        Some(l) if is_valid(l) => rust_i18n::set_locale(l),
        _ => rust_i18n::set_locale(detect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// rust-i18n 只嵌入了我们声明的两种语言；en 必须在场（它是回退）。
    #[test]
    fn embedded_locales_are_exactly_en_and_zh_cn() {
        let mut got = available_locales!();
        got.sort_unstable();
        assert_eq!(got, vec!["en", "zh-CN"]);
    }

    /// 语言切换控件的两个关键文案在两种语言下都有值。用 `locale =` 参数
    /// 查询，不动全局 locale（见模块文档的测试约定）。
    #[test]
    fn language_picker_keys_exist_in_both_locales() {
        for key in ["core.language", "core.lang.hint"] {
            for locale in LOCALES {
                let s = t!(key, locale = *locale);
                assert!(!s.trim().is_empty(), "{key} missing for {locale}");
            }
        }
    }

    /// en 与 zh-CN 的 key 集合必须完全一致：fallback 只兜语言，不兜漏翻的
    /// key——漏了就是英文悄悄漏进中文界面（或反过来）。解析走 serde-saphyr
    /// （与 rust-i18n 读取同一套文件），对照源码目录而非嵌入副本。
    #[test]
    fn every_key_exists_in_both_en_and_zh_cn() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "\\locales");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("locales/ directory exists")
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "yml"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no .yml files under locales/");

        let mut locales: BTreeSet<String> = BTreeSet::new();
        let mut by_locale: BTreeSet<(String, String)> = BTreeSet::new(); // (locale, key)
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap();
            let v: serde_json::Value =
                serde_saphyr::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            let root = v.as_object().expect("top level is a mapping");
            for (key, langs) in root {
                if key == "_version" {
                    continue;
                }
                let langs = langs.as_object().unwrap_or_else(|| {
                    panic!(
                        "{}: {key} must map en/zh-CN, not a plain value",
                        f.display()
                    )
                });
                for (locale, val) in langs {
                    locales.insert(locale.clone());
                    assert!(
                        val.is_string() && !val.as_str().unwrap().trim().is_empty(),
                        "{}: {key}.{locale} must be a non-empty string",
                        f.display()
                    );
                    by_locale.insert((locale.clone(), key.clone()));
                }
            }
        }
        // 文件里只写了我们支持的两种语言。
        let declared: BTreeSet<String> = LOCALES.iter().map(|s| s.to_string()).collect();
        assert_eq!(locales, declared, "locales/ must only use en and zh-CN");
        for locale in LOCALES {
            let keys: BTreeSet<String> = by_locale
                .iter()
                .filter(|(l, _)| l == locale)
                .map(|(_, k)| k.clone())
                .collect();
            assert!(!keys.is_empty(), "no keys for {locale}");
            for other in LOCALES {
                if other == locale {
                    continue;
                }
                let theirs: BTreeSet<String> = by_locale
                    .iter()
                    .filter(|(l, _)| l == other)
                    .map(|(_, k)| k.clone())
                    .collect();
                let missing: Vec<&String> = theirs.difference(&keys).collect();
                assert!(
                    missing.is_empty(),
                    "{other} has keys {missing:?} that {locale} lacks"
                );
            }
        }
    }

    #[test]
    fn detect_returns_a_supported_locale() {
        assert!(is_valid(detect()));
    }
}
