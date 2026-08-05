//! 规则匹配引擎测试二进制 —— 对样本文档跑通规则引擎，输出命中列表 + Agent 上下文。
//!
//! 规则引擎是确定性匹配（regex / keyword / field_compare），**零 API 调用、纯本地**。
//! 默认 `--sample` 模式用本地合成样本文档直接驱动引擎，不依赖 PDF 解析。
//!
//! ## 运行
//!
//! ```powershell
//! # 从 backend-rust/ 目录运行（让路径解析到项目根目录，见根级 CLAUDE.md）
//! $env:AIBID_DATA_DIR=".."
//! cargo run --bin test_rules -- --sample   # 默认：本地合成样本
//! cargo run --bin test_rules -- <某招标文件.pdf>   # 也可对本地 PDF 跑（无需 API）
//! ```
//!
//! ## 流程
//!
//! 1. 加载 `rules/rules.yml` 规则库
//! 2. 输入文档：合成样本（默认）或本地 PDF → Sections → Chunks
//! 3. `extract_metrics` 提取文档级度量（日期/金额）供 field_compare 使用
//! 4. 运行引擎 → 打印命中列表 + `build_agent_context`

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use ai_bid::domain::chunk::{Chunk, ChunkingConfig};
use ai_bid::paths::data_path_str;
use ai_bid::rules::engine::RuleEngine;
use ai_bid::rules::metrics::extract_metrics;
use ai_bid::rules::schema::{Chapter, Clause, ParsedDocument};
use ai_bid::services::sectionize_service::{Section, sectionize};
use anyhow::Context;

/// 递归收集章节标题链（用于 trigger.chapter_keywords 判定）。
fn collect_chapter_titles(sections: &[Section], out: &mut Vec<String>) {
    for s in sections {
        if !s.title.is_empty() {
            out.push(s.title.clone());
        }
        collect_chapter_titles(&s.children, out);
    }
}

/// 从 Chunk 列表构造 ParsedDocument。
/// - clauses：每个 chunk 一条 clause（id=chunk_id，page=page_start）
/// - chapters：全章节标题
fn build_parsed_document(
    file_name: &str,
    sections: &[Section],
    chunks: &[Chunk],
) -> ParsedDocument {
    let mut chapters = Vec::new();
    collect_chapter_titles(sections, &mut chapters);

    let clauses: Vec<Clause> = chunks
        .iter()
        .map(|c| Clause {
            id: c.chunk_id.clone(),
            page: c.page_start as u32,
            text: c.text.clone(),
        })
        .collect();

    let clause_texts: Vec<&str> = clauses.iter().map(|c| c.text.as_str()).collect();
    let metrics = extract_metrics(&clause_texts);

    ParsedDocument {
        file_name: file_name.to_string(),
        chapters: chapters.into_iter().map(|t| Chapter { title: t }).collect(),
        clauses,
        metrics,
    }
}

/// 构造一份本地合成样本文档（覆盖 regex / keyword / field_compare / all_match / absence）。
fn synthetic_sample() -> ParsedDocument {
    // 日期用"自 2000-01-01 的天数"序号：发出 2025-06-01，截止 2025-06-15（差 14 天 < 20 → TIME-001）。
    let mut metrics = HashMap::new();
    metrics.insert("招标文件发出日期".to_string(), 9289.0);
    metrics.insert("投标截止日期".to_string(), 9303.0);
    metrics.insert("招标项目估算价".to_string(), 50_000_000.0);
    metrics.insert("投标保证金金额".to_string(), 1_500_000.0); // 3% > 2% → DEPOSIT-001

    ParsedDocument {
        file_name: "某市政道路施工招标文件.docx".to_string(),
        chapters: vec![
            // 章节标题需命中各规则的 trigger.chapter_keywords
            Chapter { title: "第一章 投标须知前附表".to_string() },
            Chapter { title: "第二章 投标保证金".to_string() },
            Chapter { title: "第三章 投标人资格要求".to_string() },
            Chapter { title: "第六章 定标与授标".to_string() },
        ],
        clauses: vec![
            Clause {
                id: "cl_01".to_string(),
                page: 1,
                text: "本项目为施工招标。".to_string(),
            },
            Clause {
                id: "cl_02".to_string(),
                page: 3,
                // DISC-001：地域限制
                text: "投标人须在本市注册成立满三年，并在本市设立办公机构。".to_string(),
            },
            Clause {
                id: "cl_03".to_string(),
                page: 5,
                // QUAL-001：模糊资质
                text: "投标人须具备相应资质。".to_string(),
            },
            Clause {
                id: "cl_04".to_string(),
                page: 12,
                // AWARD-001：all_match 需两模式齐备
                text: "中标通知书发出后，招标人有权单方变更中标结果。".to_string(),
            },
            Clause {
                id: "cl_05".to_string(),
                page: 2,
                text: "本项目投标文件递交截止时间为2025年6月15日，招标文件发出日期为2025年6月1日。"
                    .to_string(),
            },
        ],
        metrics,
    }
}

fn main() -> anyhow::Result<()> {
    dotenv::dotenv().ok();

    // 1. 加载规则库（rules/rules.yml，git 管理核心资产）
    let rules_path = data_path_str("rules/rules.yml");
    let engine = RuleEngine::load_file(&rules_path)
        .with_context(|| format!("加载规则库失败: {}", rules_path))?;
    println!("规则库: {}  (已加载 {} 条规则)", rules_path, engine.rule_count());
    for id in engine.rule_ids() {
        println!("  - {}", id);
    }

    // 2. 输入：--sample（默认）或本地 PDF
    let args: Vec<String> = std::env::args().collect();
    let pdf_arg = args.iter().skip(1).find(|a| !a.starts_with("--")).cloned();

    let doc = if let Some(input_path) = pdf_arg {
        // PDF 模式：Rust 引擎失败时走 Python 兜底（main.rs 同款），全程本地
        let input = Path::new(&input_path);
        anyhow::ensure!(input.exists(), "文件不存在: {}", input.display());
        println!("\n输入文件: {}", input.display());

        let raw_doc = match ai_bid::services::pdf_extract_service::extract_pdf_to_raw_json(
            &input_path,
        ) {
            Ok(d) => d,
            Err(e) => {
                println!("  Rust pdfplumber 失败: {}，切换 Python 兜底...", e);
                let json_path = format!("{}/../output/raw_json/test_rules_raw.json", data_path_str("."));
                ai_bid::services::pdf_extract_service::extract_with_python(
                    &input_path,
                    &json_path,
                )
                .with_context(|| "Python 兜底提取失败".to_string())?;
                let json_str = std::fs::read_to_string(&json_path)
                    .with_context(|| "读取 Python 兜底输出失败".to_string())?;
                serde_json::from_str(&json_str).with_context(|| "Python 兜底 JSON 解析失败".to_string())?
            }
        };
        println!("  解析成功: {} 页", raw_doc.pages.len());

        let sections_output = sectionize(&raw_doc);
        let chunking_config = ChunkingConfig::default();
        let mut chunks = ai_bid::services::chunking_service::chunk_sections(
            &sections_output.sections,
            &chunking_config,
        );
        ai_bid::services::chunking_service::populate_bbox_refs(&mut chunks, &raw_doc);
        println!("  章节: {} 个, 条款: {} 条", sections_output.sections.len(), chunks.len());
        build_parsed_document(&input_path, &sections_output.sections, &chunks)
    } else {
        println!("\n使用本地合成样本（--sample）。也可传入本地 PDF 路径。");
        synthetic_sample()
    };

    println!("  文档级 metrics: {:?}", doc.metrics);

    // 3. 运行引擎
    let matches = engine.run(&doc);
    println!("\n命中 {} 条", matches.len());
    let mut printed_ids = HashSet::new();
    for m in &matches {
        if printed_ids.insert(m.rule_id.clone()) {
            println!(
                "  [{}] {} ({}) — 依据: {}",
                m.severity.to_uppercase(),
                m.category,
                m.rule_id,
                m.law_ref
            );
        }
        println!("    ↳ {}: {}", m.clause_id, m.matched_text);
    }

    // 4. Agent 上下文
    println!("\n══════════ Agent 上下文（build_agent_context）══════════");
    println!("{}", ai_bid::rules::build_agent_context(&matches, &doc));

    Ok(())
}
