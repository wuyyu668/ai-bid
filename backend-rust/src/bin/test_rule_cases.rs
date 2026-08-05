//! 规则正负例回归测试二进制 —— 用 `rules/cases.json` 验证每条规则
//! "应该匹配 / 不该匹配"用例全部符合预期。
//!
//! 这是 Day 2 的规则级回归测试集：新增/修改规则后运行本工具，
//! 任一正例漏检或负例误报都会以非零退出码失败。
//!
//! Run（从 backend-rust/ 目录）:
//! ```powershell
//! $env:AIBID_DATA_DIR=".."
//! cargo run --bin test_rule_cases
//! ```
//!
//! 说明：
//! - absence 类规则（CERT-001/QUAL-002/SAFE-001/SAFE-002）：正例文本"不含"关键词
//!   （文档缺该内容 → 应命中），负例文本"包含"关键词（应不命中）。
//! - 每个用例会单独构建 ParsedDocument，注入该用例的 chapters 与 metrics。

use std::collections::HashSet;

use ai_bid::paths::data_path_str;
use ai_bid::rules::engine::RuleEngine;
use ai_bid::rules::metrics::extract_metrics;
use ai_bid::rules::schema::{Chapter, Clause, ParsedDocument};
use anyhow::Context;
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
    // field_compare 依赖文档级 metrics —— 与 test_rules.rs 一致，从条款文本实时提取，
    // 使正例/负例各自独立（正例文本含"超限金额"，负例文本含"合规金额"）。
    let clause_texts: Vec<&str> = clauses.iter().map(|c| c.text.as_str()).collect();
    let metrics = extract_metrics(&clause_texts);
    ParsedDocument {
        file_name: file_name.to_string(),
        chapters: chapters.iter().map(|t| Chapter { title: t.clone() }).collect(),
        clauses,
        metrics,
    }
}

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    let rules_path = data_path_str("rules/rules.yml");
    let cases_path = data_path_str("rules/cases.json");
    let engine = RuleEngine::load_file(&rules_path)
        .with_context(|| format!("加载规则库失败: {}", rules_path))?;

    let cases_raw = std::fs::read_to_string(&cases_path)
        .with_context(|| format!("读取用例失败: {}", cases_path))?;
    let cases: CaseFile = serde_json::from_str(&cases_raw)
        .with_context(|| "cases.json 解析失败")?;

    println!("规则库 {} 条, 用例 {} 条\n", engine.rule_count(), cases.cases.len());

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut all_failures: Vec<String> = Vec::new();

    for case in &cases.cases {
        let mut rule_failures: Vec<String> = Vec::new();

        // 正例：每条文本单独跑，必须命中目标规则
        for text in &case.positive {
            let doc = build_doc(&case.document_type, &case.chapters, vec![text.clone()]);
            let hits = engine.run(&doc);
            let hit = hits.iter().any(|m| m.rule_id == case.rule_id);
            if !hit {
                rule_failures.push(format!(
                    "  正例未命中: [{}] {}",
                    case.rule_id, text
                ));
            }
        }

        // 负例：每条文本单独跑，必须不命中目标规则
        for text in &case.negative {
            let doc = build_doc(&case.document_type, &case.chapters, vec![text.clone()]);
            let hits = engine.run(&doc);
            let hit = hits.iter().any(|m| m.rule_id == case.rule_id);
            if hit {
                rule_failures.push(format!(
                    "  负例误报: [{}] {}",
                    case.rule_id, text
                ));
            }
        }

        if rule_failures.is_empty() {
            passed += 1;
            println!("✅ {} （正例 {} / 负例 {}）", case.rule_id, case.positive.len(), case.negative.len());
        } else {
            failed += 1;
            println!("❌ {}", case.rule_id);
            all_failures.extend(rule_failures);
        }
    }

    println!("\n结果: {} 通过 / {} 失败", passed, failed);
    if !all_failures.is_empty() {
        println!("\n失败明细:");
        for f in &all_failures {
            println!("{f}");
        }
        std::process::exit(1);
    }

    // 覆盖检查：规则库中未配用例的规则给出提示（不强退）
    let covered: HashSet<&str> = cases.cases.iter().map(|c| c.rule_id.as_str()).collect();
    let uncovered: Vec<&str> = engine.rule_ids().iter().copied().filter(|id| !covered.contains(id)).collect();
    if !uncovered.is_empty() {
        println!("\n⚠️ 未配置用例的规则: {:?}", uncovered);
    } else {
        println!("\n✅ 全部规则均已配置正负例用例");
    }

    Ok(())
}