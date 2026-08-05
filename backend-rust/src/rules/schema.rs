//! 规则匹配引擎 —— 六要素规则模型与文档模型
//!
//! 按 Day 1 排期拆分为 schema / engine / context 三个文件：
//!
//! - [`crate::rules::schema`]：YAML 规则反序列化模型 + 文档模型（本文件）
//! - [`crate::rules::engine`]：三合一匹配引擎（regex / keyword / field_compare）
//! - [`crate::rules::context`]：`build_agent_context` 生成 Agent System Prompt 上下文
//!
//! ## 设计要点
//!
//! - 六要素规则模型：id / category / industry / severity / source / conditions，
//!   外加 patterns / check / suggestion / law_ref / enabled。
//! - 文本类模式（regex / keyword）按 clause 逐条匹配。
//! - field_compare 是文档级度量比较：文档解析层负责把日期/金额算成
//!   数值放进 `ParsedDocument.metrics`，引擎只按规则做比较。
//! - keyword 的 `match_mode: absence` 在文档级判定"目标范围内是否缺失"。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ────────────────────────────── 强类型枚举 ──────────────────────────────

/// 规则匹配模式：任一命中 / 全部命中
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckMode {
    AnyMatch,
    AllMatch,
}

/// 严重程度等级
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// 转为字符串（用于 RuleMatch 输出）
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }

    /// 排序权重（越小越严重）
    pub fn rank(&self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::High => 1,
            Self::Medium => 2,
            Self::Low => 3,
        }
    }
}

/// 匹配器类型
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PatternType {
    Regex,
    Keyword,
    FieldCompare,
}

// ────────────────────────────── YAML 规则模型 ──────────────────────────────

/// 规则六要素中的可反序列化部分（source / conditions / patterns / check ...）。
#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    pub category: String,
    #[allow(dead_code)]
    pub industry: String,
    pub severity: Severity,
    pub source: Source,
    #[serde(default)]
    pub conditions: Conditions,
    pub patterns: Vec<Pattern>,
    pub check: CheckMode,
    pub suggestion: String,
    #[serde(default)]
    pub law_ref: Option<String>,
    /// 规则启用/禁用（默认 true）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub law: String,
    pub article: String,
    #[allow(dead_code)]
    #[serde(default)]
    pub version: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub effective_date: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Conditions {
    #[serde(default)]
    pub document_type: Option<String>,
    #[serde(default)]
    pub project_types: Option<Vec<String>>,
    #[serde(default)]
    pub trigger: Option<Trigger>,
    #[serde(default)]
    pub exclude: Option<Exclude>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Trigger {
    #[serde(default)]
    pub chapter_keywords: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Exclude {
    #[serde(default)]
    pub project_types: Option<Vec<String>>,
    #[serde(default)]
    pub clause_keywords: Option<Vec<String>>,
}

/// keyword 的 `value` 是字符串数组，regex 的 `value` 是单个字符串——用 untagged 兼容两者。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PatternValue {
    List(Vec<String>),
    Single(String),
}

impl PatternValue {
    pub fn as_list(&self) -> Vec<String> {
        match self {
            PatternValue::List(v) => v.clone(),
            PatternValue::Single(s) => vec![s.clone()],
        }
    }
    pub fn as_single(&self) -> Option<String> {
        match self {
            PatternValue::Single(s) => Some(s.clone()),
            PatternValue::List(v) => v.first().cloned(),
        }
    }
}

/// field_compare 的 `right` 可能是数字（如 20）或表达式字符串（如 "招标项目估算价 * 0.02"）。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Num(f64),
    Text(String),
}

impl Scalar {
    pub fn as_expr(&self) -> String {
        match self {
            Scalar::Num(n) => n.to_string(),
            Scalar::Text(s) => s.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pattern {
    #[serde(rename = "type")]
    pub ptype: PatternType,
    #[serde(default)]
    pub value: Option<PatternValue>,
    /// keyword 的 "OR" 或 field_compare 的比较符 "<" ">" "==" 等，共用此字段。
    #[serde(default)]
    pub operator: Option<String>,
    #[serde(default)]
    pub match_mode: Option<String>,
    // field_compare 专用
    #[serde(default)]
    pub left: Option<String>,
    #[serde(default)]
    pub right: Option<Scalar>,
    #[allow(dead_code)]
    #[serde(default)]
    pub target: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub description: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RuleFile {
    pub rules: Vec<Rule>,
}

// ────────────────────────────── 文档模型 ──────────────────────────────

/// 解析后的文档：由上游文档解析层产出。
#[derive(Debug, Clone, Default)]
pub struct ParsedDocument {
    pub file_name: String,
    /// 章节标题（用于 trigger.chapter_keywords 判定）
    pub chapters: Vec<Chapter>,
    /// 条款正文（regex / keyword 逐条匹配的对象）
    pub clauses: Vec<Clause>,
    /// 结构化度量（field_compare 的数据来源）：如日期序号、金额等
    pub metrics: HashMap<String, f64>,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Clause {
    pub id: String,
    pub page: u32,
    pub text: String,
}

/// 引擎输出的一条命中结果。
#[derive(Debug, Clone, PartialEq)]
pub struct RuleMatch {
    pub rule_id: String,
    pub clause_id: String,
    pub severity: String,
    pub category: String,
    pub suggestion: String,
    pub law_ref: String,
    pub matched_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CheckMode 枚举反序列化（snake_case）
    #[test]
    fn check_mode_deserializes() {
        let any: CheckMode = serde_yaml::from_str("any_match").expect("any_match 应可反序列化");
        assert_eq!(any, CheckMode::AnyMatch);
        let all: CheckMode = serde_yaml::from_str("all_match").expect("all_match 应可反序列化");
        assert_eq!(all, CheckMode::AllMatch);
    }

    /// Severity 枚举反序列化
    #[test]
    fn severity_deserializes() {
        let c: Severity = serde_yaml::from_str("critical").expect("critical 应可反序列化");
        assert_eq!(c, Severity::Critical);
        assert_eq!(c.as_str(), "critical");
        assert_eq!(c.rank(), 0);
    }

    /// PatternType 枚举反序列化（三型全支持）
    #[test]
    fn pattern_type_deserializes() {
        for (y, expect) in [
            ("regex", PatternType::Regex),
            ("keyword", PatternType::Keyword),
            ("field_compare", PatternType::FieldCompare),
        ] {
            let t: PatternType = serde_yaml::from_str(y).unwrap_or_else(|_| panic!("{y} 应可反序列化"));
            assert_eq!(t, expect);
        }
    }

    /// Rule 完整反序列化（六要素 + enabled 默认 true）
    #[test]
    fn rule_full_deserializes_with_enabled_default() {
        let yaml = r#"
id: "TEST-001"
category: "测试"
industry: "通用"
severity: "high"
source:
  law: "测试法"
  article: "第一条"
conditions:
  document_type: "招标文件"
patterns:
  - type: "keyword"
    value: ["测试"]
check: any_match
suggestion: "建议"
"#;
        let rule: Rule = serde_yaml::from_str(yaml).expect("Rule 应可反序列化");
        assert_eq!(rule.id, "TEST-001");
        assert_eq!(rule.severity, Severity::High);
        assert!(rule.enabled, "enabled 缺省应为 true");
        assert_eq!(rule.patterns.len(), 1);
        assert_eq!(rule.patterns[0].ptype, PatternType::Keyword);
        assert_eq!(rule.patterns[0].value.as_ref().unwrap().as_list(), vec!["测试"]);
    }

    /// enabled 显式 false 可反序列化
    #[test]
    fn rule_enabled_false_deserializes() {
        let yaml = r#"
id: "TEST-002"
category: "测试"
industry: "通用"
severity: "low"
enabled: false
source:
  law: "测试法"
  article: "第一条"
patterns:
  - type: "keyword"
    value: ["测试"]
check: any_match
suggestion: "建议"
"#;
        let rule: Rule = serde_yaml::from_str(yaml).expect("Rule 应可反序列化");
        assert!(!rule.enabled, "enabled: false 应保留");
    }

    /// field_compare 的 right 支持数字与表达式字符串（untagged）
    #[test]
    fn field_compare_scalar_untagged() {
        let yaml = r#"
id: "TEST-003"
category: "测试"
industry: "通用"
severity: "medium"
source:
  law: "测试法"
  article: "第一条"
patterns:
  - type: "field_compare"
    left: "投标保证金金额"
    operator: ">"
    right: 20
  - type: "field_compare"
    left: "投标保证金金额"
    operator: ">"
    right: "招标项目估算价 * 0.02"
check: any_match
suggestion: "建议"
"#;
        let rule: Rule = serde_yaml::from_str(yaml).expect("Rule 应可反序列化");
        let fc: Vec<&Pattern> = rule
            .patterns
            .iter()
            .filter(|p| p.ptype == PatternType::FieldCompare)
            .collect();
        assert_eq!(fc.len(), 2);
        assert_eq!(fc[0].right.as_ref().unwrap().as_expr(), "20");
        assert_eq!(fc[1].right.as_ref().unwrap().as_expr(), "招标项目估算价 * 0.02");
    }

    /// regex 的 value 是单个字符串
    #[test]
    fn regex_value_single_string() {
        let yaml = r#"
id: "TEST-004"
category: "测试"
industry: "通用"
severity: "high"
source:
  law: "测试法"
  article: "第一条"
patterns:
  - type: "regex"
    value: "(投标人).*(本市|本省)"
check: any_match
suggestion: "建议"
"#;
        let rule: Rule = serde_yaml::from_str(yaml).expect("Rule 应可反序列化");
        assert_eq!(
            rule.patterns[0].value.as_ref().unwrap().as_single(),
            Some("(投标人).*(本市|本省)".to_string())
        );
    }

    /// 空文档默认值
    #[test]
    fn parsed_document_default_is_empty() {
        let doc = ParsedDocument::default();
        assert!(doc.clauses.is_empty());
        assert!(doc.chapters.is_empty());
        assert!(doc.metrics.is_empty());
        assert!(doc.file_name.is_empty());
    }
}
