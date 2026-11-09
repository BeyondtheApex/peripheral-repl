//! Embedded host catalogs. Plugin catalogs live in their own plugin.json.
use std::{cell::Cell, collections::BTreeMap, sync::OnceLock};
thread_local! {static ENGLISH:Cell<bool>=const{Cell::new(false)};}
fn catalog() -> &'static BTreeMap<String, String> {
    static EN: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    static ZH: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    if ENGLISH.with(Cell::get) {
        EN.get_or_init(|| {
            serde_json::from_str(include_str!("../locales/en.json"))
                .expect("embedded English catalog")
        })
    } else {
        ZH.get_or_init(|| {
            serde_json::from_str(include_str!("../locales/zh-CN.json"))
                .expect("embedded Chinese catalog")
        })
    }
}
pub fn set_locale(locale: &str) {
    ENGLISH.with(|v| v.set(!locale.starts_with("zh")));
}
pub fn text(source: &'static str) -> &'static str {
    catalog().get(source).map(String::as_str).unwrap_or(source)
}
pub fn tr(source: &str) -> String {
    if let Some(value) = catalog().get(source) {
        return value.clone();
    }
    if !ENGLISH.with(Cell::get) {
        return source.into();
    }
    // Source templates can contain named or positional Rust format placeholders.
    // Match complete strings only; never translate arbitrary substrings of plugin data.
    for (template, translation) in catalog().iter().filter(|(k, _)| k.contains('{')) {
        if let Some(values) = match_template(template, source) {
            return interpolate(translation, &values);
        }
    }
    source.into()
}
fn interpolate(template: &str, values: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else { break };
        out.push_str(
            rest[1..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index).copied())
                .unwrap_or(&rest[..=end]),
        );
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}
fn match_template<'a>(template: &str, source: &'a str) -> Option<Vec<&'a str>> {
    let mut rest = source;
    let mut pattern = template;
    let mut values = Vec::new();
    while let Some(open) = pattern.find('{') {
        rest = rest.strip_prefix(&pattern[..open])?;
        let close = pattern[open..].find('}')? + open;
        pattern = &pattern[close + 1..];
        let suffix = &pattern[..pattern.find('{').unwrap_or(pattern.len())];
        if suffix.is_empty() {
            values.push(rest);
            rest = "";
        } else {
            let end = rest.find(suffix)?;
            values.push(&rest[..end]);
            rest = &rest[end..];
        }
    }
    (rest == pattern).then_some(values)
}
