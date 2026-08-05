//! 规则匹配引擎 —— 文档级 metrics 提取（field_compare 的数据源）
//!
//! `field_compare` 需要数值化的文档度量（投标截止日、发出日、估算价、预算、
//! 保证金等）。本项目已有 `agents/tools/calculate_timeline.rs`（日期差计算）
//! 与 `validate_calculation.rs`（金额校验），此处先实现一个确定性的文本提取器，
//! 把常见度量从条款文本中解析为数值放入 `ParsedDocument.metrics`。
//!
//! Day 1 约定：提取规则确定性、保守（解析不了就不放，让 field_compare 保守 false）。
//! 后续可扩展为复用 calculate_timeline / validate_calculation 的日期金额解析能力。

use std::collections::HashMap;

use regex::Regex;

/// 日期统一转为"自 2000-01-01 的天数"序号，便于 field_compare 做差比较。
pub fn date_to_day_number(year: i32, month: u32, day: u32) -> Option<f64> {
    let days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    if !(1..=12).contains(&month) {
        return None;
    }
    let max_day = if month == 2 && leap { 29 } else { days_in_month[month as usize - 1] };
    if day == 0 || day > max_day {
        return None;
    }
    let mut total: f64 = 0.0;
    for y in 2000..year {
        let ly = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        total += if ly { 366.0 } else { 365.0 };
    }
    for m in 0..(month - 1) {
        total += if m == 1 && leap { 29.0 } else { days_in_month[m as usize] as f64 };
    }
    Some(total + day as f64)
}

/// 解析形如 `2025-06-22` / `2025年6月22日` / `2025.6.22` 的日期。
pub fn parse_date(text: &str) -> Option<f64> {
    // 允许 4 位年份 + 分隔符（- / . 年 月）+ 1-2 位月日
    let re = Regex::new(
        r"(20\d{2})\s*[年./-]\s*(\d{1,2})\s*[月./-]\s*(\d{1,2})\s*日?",
    )
    .expect("日期正则应可编译");
    let c = re.captures(text)?;
    let year: i32 = c.get(1)?.as_str().parse().ok()?;
    let month: u32 = c.get(2)?.as_str().parse().ok()?;
    let day: u32 = c.get(3)?.as_str().parse().ok()?;
    date_to_day_number(year, month, day)
}

/// 金额统一换算为"元"（万元/亿元 → 元）。
fn parse_amount(text: &str) -> Option<f64> {
    let re = Regex::new(
        r"([0-9]+(?:\.[0-9]+)?)\s*(万亿元|亿万元|亿元|万元|万|元)?",
    )
    .expect("金额正则应可编译");
    let c = re.captures(text)?;
    let value: f64 = c.get(1)?.as_str().parse().ok()?;
    let unit = c.get(2).map(|m| m.as_str()).unwrap_or("");
    let multiplier = match unit {
        "万亿元" => 1e12,
        "亿万元" => 1e12,
        "亿元" => 1e8,
        "万元" => 1e4,
        "万" => 1e4,
        _ => 1.0,
    };
    Some(value * multiplier)
}

/// 在文本中查找 `关键词(正则) ... 值` 的数值（取第一个）。
fn find_near(text: &str, keyword_pattern: &str, parse: &dyn Fn(&str) -> Option<f64>) -> Option<f64> {
    let re = Regex::new(keyword_pattern).ok()?;
    let m = re.find(text)?;
    let tail = &text[m.end()..];
    // 最多向后找 60 个字符，避免串到下一句。
    let tail: String = tail.chars().take(60).collect();
    parse(&tail)
}

/// 从文档全部条款文本中提取文档级 metrics。
///
/// 提取键与 `rules/rules.yml` 中 field_compare 的 `left/right` 对齐：
///
/// - `招标文件发出日期` / `投标截止日期` —— 日期序号（天）
/// - `投标保证金金额` / `招标项目估算价` / `采购预算金额` / `投标报价总额`
///   / `履约保证金金额` / `中标金额` —— 元
pub fn extract_metrics(clause_texts: &[&str]) -> HashMap<String, f64> {
    let mut metrics = HashMap::new();
    let mut all_text = String::new();
    for t in clause_texts {
        all_text.push_str(t);
        all_text.push('\n');
    }

    // 日期类（按优先级取第一个出现）
    let date_pairs: &[(&str, &str)] = &[
        ("投标截止日期", "递交.*截止|投标截止"),
        ("招标文件发出日期", "招标文件.{0,4}(发出|发售|获取)"),
    ];
    for (key, pattern) in date_pairs {
        if let Some(d) = find_near(&all_text, pattern, &|s| parse_date(s)) {
            metrics.insert(key.to_string(), d);
        }
    }

    // 金额类
    let amount_pairs: &[(&str, &str)] = &[
        ("投标保证金金额", "投标保证金"),
        ("招标项目估算价", "估算价"),
        ("采购预算金额", "预算"),
        ("投标报价总额", "投标报价"),
        ("履约保证金金额", "履约保证金"),
        ("中标金额", "中标金额"),
    ];
    for (key, keyword) in amount_pairs {
        if let Some(v) = find_near(&all_text, keyword, &|s| parse_amount(s)) {
            metrics.insert(key.to_string(), v);
        }
    }

    metrics
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_to_day_number_basic() {
        // 2000-01-01 → 1
        assert_eq!(date_to_day_number(2000, 1, 1), Some(1.0));
        // 2000-02-29（闰年）有效
        assert!(date_to_day_number(2000, 2, 29).is_some());
        // 2001-02-29（非闰年）无效
        assert!(date_to_day_number(2001, 2, 29).is_none());
    }

    #[test]
    fn parse_date_variants() {
        assert_eq!(parse_date("2025-06-22"), parse_date("2025年6月22日"));
        assert_eq!(parse_date("2025.6.22"), parse_date("2025-06-22"));
        assert!(parse_date("无日期文本").is_none());
    }

    #[test]
    fn parse_amount_units() {
        assert_eq!(parse_amount("100元"), Some(100.0));
        assert_eq!(parse_amount("50万元"), Some(500_000.0));
        assert_eq!(parse_amount("1.5亿元"), Some(150_000_000.0));
    }

    #[test]
    fn extract_metrics_from_text() {
        let texts = [
            "招标文件发出日期为2025年6月1日。",
            "投标文件递交截止时间为2025年6月22日。",
            "本项目估算价为5000万元，投标保证金为50万元。",
        ];
        let m = extract_metrics(&texts);
        let issue = m.get("招标文件发出日期").expect("应提取发出日期");
        let deadline = m.get("投标截止日期").expect("应提取截止日期");
        let gap = deadline - issue;
        assert!((gap - 21.0).abs() < 1e-6, "6月22日 - 6月1日 = 21 天，实际 {gap}");
        assert_eq!(m.get("招标项目估算价"), Some(&50_000_000.0));
        assert_eq!(m.get("投标保证金金额"), Some(&500_000.0));
    }

    #[test]
    fn extract_metrics_for_day2_rules() {
        // DEPOSIT-002 履约保证金 与 中标金额（Day 2 新增 field_compare 所需）
        let texts = [
            "本项目履约保证金为80万元。",
            "中标金额为500万元。",
        ];
        let m = extract_metrics(&texts);
        assert_eq!(m.get("履约保证金金额"), Some(&800_000.0));
        assert_eq!(m.get("中标金额"), Some(&5_000_000.0));
    }
}
