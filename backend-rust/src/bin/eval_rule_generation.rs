//! LLM 规则生成候选通过率评测（Day 2 验收指标：> 80%）。
//!
//! 两种模式：
//!
//! 1. **在线模式**（默认）：取 `rules/rules.yml` 中现有规则的
//!    (excerpt + suggestion + source.law/article) 作为输入，调用 LLM 重新生成规则草稿，
//!    用 `RuleValidator` 5 项静态校验统计通过率。
//!
//!    ```powershell
//!    $env:AIBID_DATA_DIR=".."
//!    cargo run --release --bin eval_rule_generation -- --limit=8
//!    ```
//!
//!    说明：LLM 不可用（401 / 无 key / 网络错误）时该项计入 `skipped`（环境错误），
//!    不污染"候选不合格"统计；通过率分母只统计实际生成的候选。
//!
//! 2. **离线模式**（`--offline`）：mock LLM 返回预置候选（7 条合法 + 1 条故意非法），
//!    验证通过率统计逻辑与 80% 门槛判定可复现，无需网络。
//!
//!    ```powershell
//!    $env:AIBID_DATA_DIR=".."
//!    cargo run --release --bin eval_rule_generation -- --offline --limit=8
//!    ```

use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ai_bid::agents::react_loop::{ChatMessage, LlmClient, LlmResponse, ToolChoice};
use ai_bid::agents::tools::generate_rule::{GenerateRuleArgs, GenerateRuleTool};
use ai_bid::paths::data_path_str;
use ai_bid::services::llm_client::create_llm_client;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();

    let mut limit = 8usize;
    let mut offline = false;
    for arg in std::env::args().skip(1) {
        if let Some(v) = arg.strip_prefix("--limit=") {
            limit = v.parse()?;
        }
        if arg == "--offline" {
            offline = true;
        }
    }

    let rules_path = data_path_str("rules/rules.yml");
    let rules = ai_bid::rules::load_rules_from_file(&rules_path)?;
    println!(
        "规则库 {} 条，{}，取前 {} 条评测\n",
        rules.len(),
        if offline { "离线(mock LLM)" } else { "在线(真实 LLM)" },
        limit
    );

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut fail_details: Vec<String> = Vec::new();

    let tool = if offline {
        GenerateRuleTool::new(Arc::new(OfflineMockLlm::default()), "rules/_drafts")
    } else {
        let llm = create_llm_client()?;
        GenerateRuleTool::new(Arc::from(llm), "rules/_drafts")
    };

    for rule in rules.iter().take(limit.min(rules.len())) {
        let topic = format!(
            "{}。合规建议：{}",
            rule.source.excerpt.as_deref().unwrap_or(&rule.suggestion),
            rule.suggestion
        );
        let args = GenerateRuleArgs {
            topic,
            law: Some(rule.source.law.clone()),
            article: Some(rule.source.article.clone()),
        };

        match tool.generate(&args).await {
            Ok(result) => {
                if result.passed {
                    passed += 1;
                    println!("  ✅ 通过校验");
                } else {
                    failed += 1;
                    println!("  ❌ 校验失败: {}", result.issues.join("; "));
                    fail_details.push(result.issues.join("; "));
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                // 环境性错误（无 key / 网络 / API 权限）不算"候选不合格"——跳过
                if msg.contains("401")
                    || msg.contains("InvalidApiKey")
                    || msg.contains("API 请求失败")
                    || msg.contains("LLM API 请求失败")
                {
                    skipped += 1;
                    println!(
                        "  ⏭️  已跳过（LLM 环境错误: {}）",
                        msg.lines().next().unwrap_or("")
                    );
                } else {
                    failed += 1;
                    println!("  ❌ 生成/解析失败: {msg}");
                    fail_details.push(msg);
                }
            }
        }
    }

    let total = passed + failed;
    let rate = if total == 0 { 0.0 } else { passed as f64 / total as f64 };
    println!(
        "\n评测结果: {passed}/{total} 通过（跳过 {skipped}）  通过率 = {:.1}%  {}",
        rate * 100.0,
        if rate > 0.8 {
            "✅ 高于 80% 验收门槛"
        } else {
            "⚠️ 低于 80% 验收门槛"
        }
    );
    if !fail_details.is_empty() {
        println!("\n候选不合格明细:");
        for d in fail_details {
            println!("  - {d}");
        }
    }
    Ok(())
}

/// 离线评测用的 mock LLM：第 5 次调用返回故意非法的候选，其余返回合法候选。
struct OfflineMockLlm {
    call_count: Arc<AtomicUsize>,
}

impl Default for OfflineMockLlm {
    fn default() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// 构造一条可入库的合法规则草稿（六要素 YAML）。
fn good_draft() -> &'static str {
    r#"id: "MOCK-001"
category: "资质资格"
industry: "通用"
severity: "high"
source:
  law: "中华人民共和国招标投标法"
  article: "第十八条"
  excerpt: "招标人应当对投标人进行资格审查"
patterns:
  - type: "keyword"
    value: ["资格审查", "资格预审"]
    target: all_clauses
    operator: OR
check: any_match
suggestion: "招标人应依法进行资格审查。"
law_ref: "《中华人民共和国招标投标法》第十八条"
"#
}

/// 故意违反 industry 分类的候选（用于验证 80% 门槛判定）。
fn bad_draft() -> &'static str {
    r#"id: "MOCK-BAD"
category: "其他"
industry: "不存在的行业"
severity: "low"
source:
  law: "中华人民共和国招标投标法"
  article: "第一条"
patterns:
  - type: "keyword"
    value: ["测试"]
check: any_match
suggestion: "建议"
"#
}

#[async_trait::async_trait]
impl LlmClient for OfflineMockLlm {
    async fn chat(
        &self,
        _messages: &[ChatMessage],
        _tools: &[serde_json::Value],
        _tool_choice: &ToolChoice,
    ) -> Result<LlmResponse> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);
        let content = if n == 4 { bad_draft() } else { good_draft() };
        Ok(LlmResponse {
            content: Some(content.to_string()),
            thought: None,
            tool_calls: vec![],
            usage: None,
        })
    }
}