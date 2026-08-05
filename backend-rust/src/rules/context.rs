//! 规则匹配引擎 —— Agent 上下文生成
//!
//! `build_agent_context(matches, doc)` 把规则引擎的命中结果整理为
//! Agent System Prompt 的上下文段落，分两段：
//!
//! 1. **初审结果（规则引擎自动检测）** —— 已由确定性规则确认的问题，可直接引用。
//! 2. **需要深度语义审查的条款** —— 未命中的条款，交给 Agent 语义分析。
//!
//! 接入点：`agents/prompts.rs` 引用该上下文；命中结果同时注入 `session_graph.rs`。

use std::collections::HashSet;

use crate::rules::schema::{ParsedDocument, RuleMatch};

/// 生成 Agent System Prompt 的规则引擎上下文。
pub fn build_agent_context(matches: &[RuleMatch], doc: &ParsedDocument) -> String {
    let mut ctx = String::new();
    ctx.push_str("## 初审结果（规则引擎自动检测）\n\n");
    let criticals = matches.iter().filter(|m| m.severity == "critical").count();
    ctx.push_str(&format!(
        "已标记 {} 个问题（critical: {}）。以下问题已由确定性规则确认，可直接引用到审核报告：\n\n",
        matches.len(),
        criticals
    ));
    for m in matches {
        ctx.push_str(&format!(
            "- [{}] {} ({}): {}\n  依据: {}\n  建议: {}\n",
            m.severity.to_uppercase(),
            m.category,
            m.rule_id,
            m.matched_text,
            m.law_ref,
            m.suggestion
        ));
    }

    ctx.push_str("\n## 需要深度语义审查的条款\n\n");
    let matched_ids: HashSet<&String> = matches.iter().map(|m| &m.clause_id).collect();
    for clause in doc.clauses.iter().filter(|c| !matched_ids.contains(&c.id)).take(10) {
        ctx.push_str(&format!(
            "- 第 {} 页 {}: {}...\n",
            clause.page,
            clause.id,
            clause.text.chars().take(60).collect::<String>()
        ));
    }
    ctx
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::rules::schema::{Chapter, Clause};

    fn doc_with_clauses(texts: Vec<&str>) -> ParsedDocument {
        ParsedDocument {
            file_name: "招标文件.docx".to_string(),
            chapters: vec![Chapter { title: "投标人资格要求".to_string() }],
            clauses: texts
                .into_iter()
                .enumerate()
                .map(|(i, text)| Clause {
                    id: format!("C-{}", i + 1),
                    page: (i + 1) as u32,
                    text: text.to_string(),
                })
                .collect(),
            metrics: HashMap::new(),
        }
    }

    fn match_critical() -> RuleMatch {
        RuleMatch {
            rule_id: "DISC-001".to_string(),
            clause_id: "C-1".to_string(),
            severity: "critical".to_string(),
            category: "排斥性条款".to_string(),
            suggestion: "建议删除地域性要求。".to_string(),
            law_ref: "《中华人民共和国招标投标法》第十八条".to_string(),
            matched_text: "投标人须在本市注册成立满三年。".to_string(),
        }
    }

    /// 上下文包含"已确认"分节，且统计数字正确
    #[test]
    fn context_counts_confirmed_and_critical() {
        let doc = doc_with_clauses(vec!["投标人须在本市注册成立满三年。", "投标人须具备施工总承包二级资质。"]);
        let ctx = build_agent_context(&[match_critical()], &doc);
        assert!(ctx.contains("已标记 1 个问题（critical: 1）"), "应统计命中数与 critical 数");
        assert!(ctx.contains("## 初审结果（规则引擎自动检测）"));
        assert!(ctx.contains("DISC-001"));
        assert!(ctx.contains("《中华人民共和国招标投标法》第十八条"));
    }

    /// 已命中的 clause 不应出现在"待审查"分节
    #[test]
    fn matched_clause_excluded_from_pending() {
        let doc = doc_with_clauses(vec!["投标人须在本市注册成立满三年。", "另一条独立条款。"]);
        let ctx = build_agent_context(&[match_critical()], &doc);
        // C-1 已命中 → 不在待审查；C-2 未命中 → 出现在待审查
        assert!(!ctx.contains("C-1:"), "已命中的 C-1 不应在待审查列表");
        assert!(ctx.contains("C-2:"), "未命中的 C-2 应在待审查列表");
        assert!(ctx.contains("## 需要深度语义审查的条款"));
    }

    /// 无命中时：计数为 0，所有条款进入待审查
    #[test]
    fn empty_matches_all_clauses_pending() {
        let doc = doc_with_clauses(vec!["条款甲。", "条款乙。"]);
        let ctx = build_agent_context(&[], &doc);
        assert!(ctx.contains("已标记 0 个问题（critical: 0）"));
        assert!(ctx.contains("C-1:"));
        assert!(ctx.contains("C-2:"));
    }

    /// 待审查列表最多展示 10 条
    #[test]
    fn pending_list_capped_at_ten() {
        let texts: Vec<String> = (0..15).map(|i| format!("条款{}。", i)).collect();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let doc = doc_with_clauses(text_refs);
        let ctx = build_agent_context(&[], &doc);
        // 每条待审查以 "- 第 X 页 C-N:" 开头
        let pending_entries = ctx
            .lines()
            .filter(|l| l.starts_with("- 第 ") && l.contains(": "))
            .count();
        assert_eq!(pending_entries, 10, "待审查列表应最多展示 10 条");
    }
}
