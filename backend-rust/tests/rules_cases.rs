//! 规则匹配引擎回归测试集（Day 2）。
//!
//! 读取 `rules/cases.json`（27 条规则 × 正负例），逐条断言：
//! - 正例文本必须命中对应规则
//! - 负例文本必须不命中对应规则
//!
//! 任一断言失败即测试失败 —— 这是规则库修改后的回归闸门。
//!
//! 运行（从 backend-rust/ 目录）：
//! ```powershell
//! $env:AIBID_DATA_DIR=".."
//! cargo test --test rules_cases
//! ```

use std::collections::HashSet;

use ai_bid::paths::data_path_str;
use ai_bid::rules::engine::RuleEngine;
use ai_bid::rules::metrics::extract_metrics;
use ai_bid::rules::schema::{Chapter, Clause, ParsedDocument};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    rule_id: String,
    document_type: String,
    #[serde(default)]
    chapters: Vec<String>,
    #[serde(default)]
    positive: Vec<String>,
    #[serde(default)]
    negative: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CaseFile {
    cases: Vec<Case>,
}

fn build_doc(file_name: &str, chapters: &[String], clauses: Vec<String>) -> ParsedDocument {
    let clauses: Vec<Clause> = clauses
        .into_iter()
        .enumerate()
        .map(|(i, text)| Clause {
            id: format!("case_{}", i),
            page: 1,
            text,
        })
        .collect();
    // field_compare 依赖文档级 metrics —— 与生产路径一致，从条款文本实时提取
    let clause_texts: Vec<&str> = clauses.iter().map(|c| c.text.as_str()).collect();
    let metrics = extract_metrics(&clause_texts);
    ParsedDocument {
        file_name: file_name.to_string(),
        chapters: chapters.iter().map(|t| Chapter { title: t.clone() }).collect(),
        clauses,
        metrics,
    }
}

fn load_cases() -> Vec<Case> {
    let path = data_path_str("rules/cases.json");
    let raw = std::fs::read_to_string(&path).expect("读取 cases.json 失败");
    let parsed: CaseFile = serde_json::from_str(&raw).expect("cases.json 解析失败");
    parsed.cases
}

/// 回归测试：每条规则的正负例必须全部符合预期。
#[test]
fn all_rule_cases_pass() {
    let rules_path = data_path_str("rules/rules.yml");
    let engine = RuleEngine::load_file(&rules_path).expect("规则库加载失败");
    let cases = load_cases();
    assert!(
        !cases.is_empty(),
        "cases.json 不应为空（规则回归测试集缺失）"
    );

    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for case in &cases {
        // 正例：必须命中
        for text in &case.positive {
            let doc = build_doc(&case.document_type, &case.chapters, vec![text.clone()]);
            let hits = engine.run(&doc);
            if !hits.iter().any(|m| m.rule_id == case.rule_id) {
                failures.push(format!("正例未命中 [{}]: {}", case.rule_id, text));
            }
            checked += 1;
        }
        // 负例：必须不命中
        for text in &case.negative {
            let doc = build_doc(&case.document_type, &case.chapters, vec![text.clone()]);
            let hits = engine.run(&doc);
            if hits.iter().any(|m| m.rule_id == case.rule_id) {
                failures.push(format!("负例误报 [{}]: {}", case.rule_id, text));
            }
            checked += 1;
        }
    }

    assert!(
        failures.is_empty(),
        "{} 个用例断言失败:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(checked > 0, "至少应检查一个用例");
}

/// 覆盖检查：规则库中每条规则都应有对应用例（规则数 == 用例数）。
#[test]
fn every_rule_has_cases() {
    let rules_path = data_path_str("rules/rules.yml");
    let engine = RuleEngine::load_file(&rules_path).expect("规则库加载失败");
    let cases = load_cases();

    let covered: HashSet<&str> = cases.iter().map(|c| c.rule_id.as_str()).collect();
    let uncovered: Vec<&str> = engine
        .rule_ids()
        .iter()
        .copied()
        .filter(|id| !covered.contains(id))
        .collect();
    assert!(
        uncovered.is_empty(),
        "以下规则未配置正负例用例: {:?}",
        uncovered
    );
}

/// 全库静态校验通过率必须 > 80%（Day 2 验收门槛）。
#[test]
fn rules_pass_static_validation_threshold() {
    let rules_path = data_path_str("rules/rules.yml");
    let rules = ai_bid::rules::load_rules_from_file(&rules_path).expect("规则库加载失败");
    let report = ai_bid::rules::validator::RuleValidator::validate_all(&rules);
    assert!(
        report.pass_rate() > 0.8,
        "规则库静态校验通过率 {:.1}% 低于 80% 门槛。问题明细: {:?}",
        report.pass_rate() * 100.0,
        report
            .issues
            .iter()
            .map(|i| format!("[{}] {}: {}", i.rule_id, i.field, i.message))
            .collect::<Vec<_>>()
    );
    assert_eq!(rules.len(), 27, "Day 2 规则库应扩至 27 条（25-30 条区间内）");
}
