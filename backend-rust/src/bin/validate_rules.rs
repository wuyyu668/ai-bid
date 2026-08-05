//! 规则库静态校验二进制 —— 对 `rules/rules.yml` 跑 RuleValidator 5 项校验。
//!
//! Run（从 backend-rust/ 目录）:
//! ```powershell
//! $env:AIBID_DATA_DIR=".."
//! cargo run --bin validate_rules
//! ```
//!
//! 输出每类问题的明细 + 通过率。Day 2 验收：通过率 > 80%。

use ai_bid::paths::data_path_str;
use ai_bid::rules::validator::{RuleValidator, ValidationIssue};
use anyhow::Context;

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let rules_path = data_path_str("rules/rules.yml");
    let rules = ai_bid::rules::load_rules_from_file(&rules_path)
        .with_context(|| format!("加载规则库失败: {}", rules_path))?;

    println!("规则库: {}  (共 {} 条)", rules_path, rules.len());

    let report = RuleValidator::validate_all(&rules);
    let rate = report.pass_rate();

    println!(
        "\n校验结果: {}/{} 通过  通过率 = {:.1}%",
        report.passed,
        report.total,
        rate * 100.0
    );

    // 按 field 分组打印问题
    let mut by_field: Vec<(&'static str, Vec<&ValidationIssue>)> = Vec::new();
    for issue in &report.issues {
        if let Some(entry) = by_field.iter_mut().find(|(f, _)| *f == issue.field) {
            entry.1.push(issue);
        } else {
            by_field.push((issue.field, vec![issue]));
        }
    }

    for (field, issues) in &by_field {
        println!("\n[{field}] {} 条问题:", issues.len());
        for issue in issues {
            println!("  - {}: {}", issue.rule_id, issue.message);
        }
    }

    if report.is_pass() {
        println!("\n✅ 全部规则通过静态校验");
    } else {
        println!("\n⚠️ 有 {} 条规则存在校验问题", report.total - report.passed);
    }

    Ok(())
}