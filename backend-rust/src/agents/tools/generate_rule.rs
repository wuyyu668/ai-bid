//! `generate_rule` 工具 — LLM 规则草稿生成（Day 2）。
//!
//! 流程：用户（或 Agent / API）提供"条款样本 + 法律依据"→ 调用 LLM
//! （[`RULE_EXTRACT_SYSTEM_PROMPT`]）生成 YAML 规则草稿 → `RuleValidator` 静态校验
//! → 校验通过则写入 `rules/_drafts/<id>-<timestamp>.yml`，等待人工确认后移入
//! `rules/rules.yml`。
//!
//! 设计要点：
//! - 草稿只落 `_drafts/`，绝不直接改正式规则库 —— 保证规则库变更始终有人工确认。
//! - 校验不过的草稿不落盘，返回校验报告（含 5 项校验问题明细），由调用方决定是否重试。
//! - LLM 生成候选的校验通过率是 Day 2 验收指标（> 80%），可由 API / CLI 批量调用统计。
//!
//! ## 调用示例（Agent 工具方式）
//!
//! ```json
//! {
//!   "topic": "投标保证金不得超过招标项目估算价的2%",
//!   "law": "中华人民共和国招标投标法实施条例",
//!   "article": "第二十六条"
//! }
//! ```

use anyhow::{Context, Result};
use serde::Deserialize;
use std::sync::Arc;

use crate::agents::prompts::RULE_EXTRACT_SYSTEM_PROMPT;
use crate::agents::react_loop::{ChatMessage, LlmClient, ToolChoice};
use crate::rules::validator::RuleValidator;
use crate::rules::schema::Rule;

use super::AgentTool;

/// `generate_rule` 工具的参数。
#[derive(Debug, Deserialize)]
pub struct GenerateRuleArgs {
    /// 条款样本/主题（必填）—— 从招标文件摘录的原文，或要规范化的风险点描述
    pub topic: String,
    /// 法律依据名称（可选，默认让 LLM 判断）
    #[serde(default)]
    pub law: Option<String>,
    /// 法律条款编号（可选，如 "第二十六条"）
    #[serde(default)]
    pub article: Option<String>,
}

/// LLM 规则草稿生成工具。
pub struct GenerateRuleTool {
    llm: Arc<dyn LlmClient>,
    /// 草稿目录（相对数据根）：rules/_drafts
    drafts_dir: String,
}

impl GenerateRuleTool {
    /// 创建工具实例。
    ///
    /// * `llm` — LLM 客户端（与 ReAct 循环共用同一协议抽象）
    /// * `drafts_dir` — 草稿落盘目录，如 `rules/_drafts`
    pub fn new(llm: Arc<dyn LlmClient>, drafts_dir: &str) -> Self {
        Self {
            llm,
            drafts_dir: drafts_dir.to_string(),
        }
    }

    /// 从 LLM 回复中提取 YAML 正文（剥离可能的 ```yaml 围栏与前后杂讯）。
    fn extract_yaml(content: &str) -> String {
        let trimmed = content.trim();
        // 优先取第一个 ```yaml / ``` 围栏内的内容
        let fence = "```";
        if let Some(start) = trimmed.find(fence) {
            let after = &trimmed[start + fence.len()..];
            if let Some(rest) = after.strip_prefix("yaml").or_else(|| after.strip_prefix("yml")) {
                if let Some(end) = rest.find(fence) {
                    return rest[..end].trim().to_string();
                }
            } else if let Some(end) = after.find(fence) {
                return after[..end].trim().to_string();
            }
        }
        trimmed.to_string()
    }

    /// 生成草稿（含校验），返回草稿信息。
    pub async fn generate(&self, args: &GenerateRuleArgs) -> Result<GenerateRuleResult> {
        let mut user_content = format!("条款样本/主题：\n{}\n", args.topic);
        if let Some(law) = &args.law {
            user_content.push_str(&format!("\n法律依据：{}", law));
        }
        if let Some(article) = &args.article {
            user_content.push_str(&format!(" 第{}条", article.trim_start_matches("第").trim_end_matches("条")));
        }

        let messages = vec![
            ChatMessage::System {
                content: RULE_EXTRACT_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage::User {
                content: user_content,
            },
        ];

        let resp = self
            .llm
            .chat(&messages, &[], &ToolChoice::Auto)
            .await
            .context("LLM 规则草稿生成失败")?;

        let content = resp
            .content
            .clone()
            .or(resp.thought.clone())
            .unwrap_or_default();
        if content.trim().is_empty() {
            anyhow::bail!("LLM 未返回规则草稿内容");
        }

        let yaml_text = Self::extract_yaml(&content);

        // 反序列化为 Rule（与 rules.yml 同模型）
        let draft: Rule = serde_yaml::from_str(&yaml_text).map_err(|e| {
            anyhow::anyhow!("LLM 输出无法解析为规则模型: {e}\n--- 原始输出 ---\n{content}")
        })?;

        // 静态校验
        let report = RuleValidator::validate(&draft);

        // 校验通过才落盘
        let mut draft_path = None;
        if report.is_pass() {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let dir = crate::paths::data_path(&self.drafts_dir);
            std::fs::create_dir_all(&dir).context("创建草稿目录失败")?;
            let path = dir.join(format!("{}-{}.yml", draft.id, ts));
            std::fs::write(&path, &yaml_text).context("写入规则草稿失败")?;
            draft_path = Some(path.to_string_lossy().to_string());
        }

        Ok(GenerateRuleResult {
            rule_id: draft.id.clone(),
            yaml: yaml_text,
            passed: report.is_pass(),
            pass_rate: report.pass_rate(),
            issues: report
                .issues
                .iter()
                .map(|i| format!("[{}] {}", i.field, i.message))
                .collect(),
            draft_path,
        })
    }
}

/// 生成结果（JSON 序列化返回给调用方）。
#[derive(Debug, serde::Serialize)]
pub struct GenerateRuleResult {
    pub rule_id: String,
    pub yaml: String,
    /// 是否通过 RuleValidator 5 项静态校验
    pub passed: bool,
    /// 通过率（0.0 ~ 1.0）
    pub pass_rate: f64,
    /// 校验问题明细（校验失败时非空）
    pub issues: Vec<String>,
    /// 草稿文件路径（仅校验通过时存在）
    pub draft_path: Option<String>,
}

#[async_trait::async_trait]
impl AgentTool for GenerateRuleTool {
    fn name(&self) -> &str {
        "generate_rule"
    }

    fn definition(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "generate_rule",
                "description": "【使用场景】根据条款样本与法律依据，用 LLM 生成一条规则草稿（六要素 YAML），\
                    静态校验通过后写入 rules/_drafts/ 待人工确认。\
                    【不使用场景】直接审查条款是否合规——用 ReAct 审查流程。\
                    【topic】必填：条款样本原文或风险点描述。\
                    【law】可选：法律依据名称（如 中华人民共和国招标投标法实施条例）。\
                    【article】可选：条款编号（如 第二十六条）。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "topic": {
                            "type": "string",
                            "description": "条款样本原文或要规范化的风险点描述"
                        },
                        "law": {
                            "type": "string",
                            "description": "法律依据名称（可选）"
                        },
                        "article": {
                            "type": "string",
                            "description": "法律条款编号（可选，如 第二十六条）"
                        }
                    },
                    "required": ["topic"]
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let parsed: GenerateRuleArgs = serde_json::from_value(args)?;
        let result = self.generate(&parsed).await?;
        Ok(serde_json::to_value(&result)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_yaml_strips_fences() {
        let with_fence = "```yaml\nid: TEST-001\ncategory: 测试\n```\n补充说明";
        assert_eq!(GenerateRuleTool::extract_yaml(with_fence), "id: TEST-001\ncategory: 测试");
    }

    #[test]
    fn extract_yaml_passes_through_plain() {
        let plain = "id: TEST-002\ncategory: 测试\n";
        assert_eq!(GenerateRuleTool::extract_yaml(plain), "id: TEST-002\ncategory: 测试");
    }
}
