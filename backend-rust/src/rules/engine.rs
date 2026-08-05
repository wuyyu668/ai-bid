//! 规则匹配引擎 —— 三合一匹配器（regex / keyword / field_compare）

//! 设计要点：
//!
//! - **regex**：预编译缓存，YAML 里 `\uXXXX` 自动转 `\x{XXXX}`（`normalize_regex`）；
//!   单条正则编译失败只跳过该条并告警，不让整库加载失败。
//! - **keyword**：字符串数组 OR/AND；`match_mode: absence` 支持"缺失匹配"。
//! - **field_compare**：文档级度量比较，`left/operator/right`，right 支持表达式
//!   如 `估算价 * 0.02`；操作数解析失败保守返回 false（不误报）。
//! - **conditions 优先级**：exclude > document_type > trigger。
//! - **check**：`any_match` / `all_match`。

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use thiserror::Error;

use crate::rules::schema::{CheckMode, ParsedDocument, Pattern, PatternType, Rule, RuleMatch};

// ────────────────────────────── 错误类型 ──────────────────────────────

/// 引擎错误类型
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("读取规则文件失败 {path}: {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },

    #[error("YAML 解析失败: {0}")]
    YamlParse(#[from] serde_yaml::Error),
}

// ────────────────────────────── 引擎 ──────────────────────────────

struct CompiledRule {
    rule: Rule,
    /// 预编译正则，与 rule.patterns 中 Regex 模式一一对应（顺序保留）。
    regex_patterns: Vec<Regex>,
}

/// 规则匹配引擎
pub struct RuleEngine {
    rules: Vec<CompiledRule>,
}

impl RuleEngine {
    /// 从单个 YAML 文件加载规则并预编译正则。
    pub fn load_file<P: AsRef<Path>>(path: P) -> Result<Self, EngineError> {
        let path_ref = path.as_ref();
        let text = std::fs::read_to_string(path_ref).map_err(|e| EngineError::FileRead {
            path: path_ref.display().to_string(),
            source: e,
        })?;
        Self::from_yaml(&text)
    }

    /// 从 YAML 文本加载（便于测试）。
    pub fn from_yaml(yaml: &str) -> Result<Self, EngineError> {
        let file: crate::rules::schema::RuleFile = serde_yaml::from_str(yaml)?;

        let mut compiled = Vec::new();
        for rule in file.rules {
            // 跳过已禁用的规则
            if !rule.enabled {
                log::info!("跳过已禁用规则: {}", rule.id);
                continue;
            }

            let mut regex_patterns = Vec::new();
            for p in rule.patterns.iter().filter(|p| p.ptype == PatternType::Regex) {
                if let Some(val) = p.value.as_ref().and_then(|v| v.as_single()) {
                    let normalized = normalize_regex(&val);
                    match Regex::new(&normalized) {
                        Ok(re) => regex_patterns.push(re),
                        // 容错：单条正则语法不兼容时跳过并告警，不让整库加载失败。
                        Err(e) => log::warn!(
                            "规则 {} 的正则编译失败，已跳过：{}\n        pattern = {}",
                            rule.id, e, val
                        ),
                    }
                }
            }
            compiled.push(CompiledRule { rule, regex_patterns });
        }
        Ok(RuleEngine { rules: compiled })
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// 返回所有已加载规则的 ID 列表
    pub fn rule_ids(&self) -> Vec<&str> {
        self.rules.iter().map(|c| c.rule.id.as_str()).collect()
    }

    /// 按 ID 查找规则
    pub fn get_rule(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|c| c.rule.id == id).map(|c| &c.rule)
    }

    /// 对整份文档运行所有规则，返回命中列表（按 severity 排序）。
    pub fn run(&self, doc: &ParsedDocument) -> Vec<RuleMatch> {
        let mut matches = Vec::new();

        for compiled in &self.rules {
            if !check_conditions(&compiled.rule, doc) {
                continue;
            }
            evaluate_rule(compiled, doc, &mut matches);
        }

        matches.sort_by_key(|m| severity_rank(&m.severity));
        matches
    }

    /// 批量处理多份文档
    pub fn run_batch(&self, docs: &[&ParsedDocument]) -> Vec<Vec<RuleMatch>> {
        docs.iter().map(|doc| self.run(doc)).collect()
    }
}

fn severity_rank(sev: &str) -> u8 {
    match sev {
        "critical" => 0,
        "high" => 1,
        "warning" | "medium" => 2,
        _ => 3,
    }
}

/// Unicode 转义正则（缓存避免重复编译）
static UNICODE_ESCAPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\u([0-9A-Fa-f]{4})").unwrap()
});

/// Rust regex 使用 `\x{XXXX}` 表示 Unicode 码点，而 YAML 里写的是 PCRE 风格 `\uXXXX`。
/// 做一次兼容转换，让 BRAND-001 等含中文码点区间的正则也能编译。
fn normalize_regex(pat: &str) -> String {
    UNICODE_ESCAPE_RE.replace_all(pat, r"\x{$1}").into_owned()
}

// ────────────────────────────── 条件判定 ──────────────────────────────

/// 检查规则的适用条件：exclude 优先于 trigger（命中排除即跳过）。
fn check_conditions(rule: &Rule, doc: &ParsedDocument) -> bool {
    let cond = &rule.conditions;

    // ① 排除条件优先——命中即不触发
    if let Some(exclude) = &cond.exclude {
        let project_text = extract_project_type(doc);
        if let Some(pts) = &exclude.project_types
            && pts.iter().any(|pt| project_text.contains(pt))
        {
            return false;
        }
        if let Some(kws) = &exclude.clause_keywords {
            let any_clause_has =
                doc.clauses.iter().any(|c| kws.iter().any(|kw| c.text.contains(kw)));
            if any_clause_has {
                return false;
            }
        }
    }

    // ② 文档类型匹配
    if let Some(dt) = &cond.document_type
        && !doc.file_name.contains(dt)
    {
        return false;
    }

    // ③ 章节触发条件
    if let Some(trigger) = &cond.trigger
        && let Some(keywords) = &trigger.chapter_keywords
    {
        let any_chapter_has = doc
            .chapters
            .iter()
            .any(|ch| keywords.iter().any(|kw| ch.title.contains(kw)));
        if !any_chapter_has {
            return false;
        }
    }

    true
}

/// 项目类型通常在招标公告首段——取第一条 clause 文本做包含判断。
fn extract_project_type(doc: &ParsedDocument) -> String {
    doc.clauses.first().map(|c| c.text.clone()).unwrap_or_default()
}

// ────────────────────────────── 匹配判定 ──────────────────────────────

/// 单个 pattern 是否在给定 clause 文本上命中（仅文本类：regex / keyword）。
/// regex 通过预编译索引 `re_idx` 取用对应正则。
fn text_pattern_hit(
    p: &Pattern,
    clause_text: &str,
    compiled: &CompiledRule,
    re_idx: &mut usize,
) -> bool {
    match p.ptype {
        PatternType::Regex => {
            let hit = compiled
                .regex_patterns
                .get(*re_idx)
                .map(|re| re.is_match(clause_text))
                .unwrap_or(false);
            *re_idx += 1;
            hit
        }
        PatternType::Keyword => {
            // absence 模式在文档级判定，clause 级视为不成立（由 evaluate_rule 处理）。
            if p.match_mode.as_deref() == Some("absence") {
                return false;
            }
            let words = p.value.as_ref().map(|v| v.as_list()).unwrap_or_default();
            // operator 默认 OR：任一关键词命中即成立。
            words.iter().any(|w| clause_text.contains(w))
        }
        PatternType::FieldCompare => false,
    }
}

/// 评估一条规则，产出的命中追加到 matches。
fn evaluate_rule(compiled: &CompiledRule, doc: &ParsedDocument, matches: &mut Vec<RuleMatch>) {
    let rule = &compiled.rule;
    let has_field = rule.patterns.iter().any(|p| p.ptype == PatternType::FieldCompare);

    if has_field {
        evaluate_field_rule(compiled, doc, matches);
    } else {
        evaluate_text_rule(compiled, doc, matches);
    }
}

/// 文本类规则：regex / keyword 按 clause 逐条判定 any_match / all_match。
fn evaluate_text_rule(compiled: &CompiledRule, doc: &ParsedDocument, matches: &mut Vec<RuleMatch>) {
    let rule = &compiled.rule;

    // 先处理文档级的 absence 关键词（如 CERT-001）：目标范围内无任何关键词即命中。
    let absence_hit = rule
        .patterns
        .iter()
        .filter(|p| p.ptype == PatternType::Keyword && p.match_mode.as_deref() == Some("absence"))
        .any(|p| {
            let words = p.value.as_ref().map(|v| v.as_list()).unwrap_or_default();
            !doc.clauses.iter().any(|c| words.iter().any(|w| c.text.contains(w)))
        });
    if absence_hit && rule.check == CheckMode::AnyMatch {
        matches.push(make_match(rule, "<document>", "（目标章节缺少要求的内容）"));
        return;
    }

    let text_patterns: Vec<&Pattern> = rule
        .patterns
        .iter()
        .filter(|p| {
            p.ptype == PatternType::Regex
                || (p.ptype == PatternType::Keyword && p.match_mode.as_deref() != Some("absence"))
        })
        .collect();

    for clause in &doc.clauses {
        // 每条 clause 重新从头对齐预编译正则索引。
        let mut re_idx = 0usize;
        let mut results = Vec::with_capacity(text_patterns.len());
        for p in &text_patterns {
            results.push(text_pattern_hit(p, &clause.text, compiled, &mut re_idx));
        }
        let triggered = match rule.check {
            CheckMode::AnyMatch => results.iter().any(|&b| b),
            CheckMode::AllMatch => !results.is_empty() && results.iter().all(|&b| b),
        };
        if triggered {
            matches.push(make_match(rule, &clause.id, &clause.text));
        }
    }
}

/// field_compare 规则：文档级度量比较。
fn evaluate_field_rule(compiled: &CompiledRule, doc: &ParsedDocument, matches: &mut Vec<RuleMatch>) {
    let rule = &compiled.rule;
    let fc: Vec<&Pattern> = rule
        .patterns
        .iter()
        .filter(|p| p.ptype == PatternType::FieldCompare)
        .collect();

    let results: Vec<bool> = fc.iter().map(|p| field_compare_hit(p, doc)).collect();
    let triggered = match rule.check {
        CheckMode::AllMatch => !results.is_empty() && results.iter().all(|&b| b),
        CheckMode::AnyMatch => results.iter().any(|&b| b),
    };
    if triggered {
        // 用第一个可解析的表达式做证据展示。
        let evidence = fc
            .iter()
            .map(|p| describe_compare(p))
            .next()
            .unwrap_or_else(|| "字段比较命中".to_string());
        matches.push(make_match(rule, "<metrics>", &evidence));
    }
}

/// 判断单个 field_compare 是否成立。无法解析操作数时返回 false（保守：不误报）。
fn field_compare_hit(p: &Pattern, doc: &ParsedDocument) -> bool {
    let (Some(left), Some(op), Some(right)) = (
        p.left.as_ref(),
        p.operator.as_ref(),
        p.right.as_ref(),
    ) else {
        log::debug!("field_compare 缺少必要字段 (left/operator/right)");
        return false;
    };
    let (Some(l), Some(r)) = (
        resolve_operand(left, &doc.metrics),
        resolve_operand(&right.as_expr(), &doc.metrics),
    ) else {
        log::debug!(
            "field_compare 操作数解析失败: left={}, right={}",
            left,
            right.as_expr()
        );
        return false;
    };
    match op.as_str() {
        "<" => l < r,
        "<=" => l <= r,
        ">" => l > r,
        ">=" => l >= r,
        "==" => (l - r).abs() < f64::EPSILON,
        "!=" => (l - r).abs() >= f64::EPSILON,
        _ => {
            log::debug!("field_compare 不支持的操作符: {}", op);
            false
        }
    }
}

/// 解析操作数表达式：支持字面量、字段名、以及 A-B / A+B / A*k 简单二元式。
fn resolve_operand(expr: &str, metrics: &std::collections::HashMap<String, f64>) -> Option<f64> {
    let e = expr.trim();
    if let Ok(n) = e.parse::<f64>() {
        return Some(n);
    }
    if let Some(v) = metrics.get(e) {
        return Some(*v);
    }
    for (op, f) in [
        (" - ", (|a: f64, b: f64| a - b) as fn(f64, f64) -> f64),
        (" + ", (|a, b| a + b) as fn(f64, f64) -> f64),
        (" * ", (|a, b| a * b) as fn(f64, f64) -> f64),
    ] {
        if let Some(idx) = e.find(op) {
            let l = resolve_operand(&e[..idx], metrics)?;
            let r = resolve_operand(&e[idx + op.len()..], metrics)?;
            return Some(f(l, r));
        }
    }
    None
}

fn describe_compare(p: &Pattern) -> String {
    format!(
        "{} {} {}",
        p.left.clone().unwrap_or_default(),
        p.operator.clone().unwrap_or_default(),
        p.right.as_ref().map(|r| r.as_expr()).unwrap_or_default()
    )
}

fn make_match(rule: &Rule, clause_id: &str, matched_text: &str) -> RuleMatch {
    let law_ref = rule
        .law_ref
        .clone()
        .unwrap_or_else(|| format!("《{}》{}", rule.source.law, rule.source.article));
    RuleMatch {
        rule_id: rule.id.clone(),
        clause_id: clause_id.to_string(),
        severity: rule.severity.as_str().to_string(),
        category: rule.category.clone(),
        suggestion: rule.suggestion.clone(),
        law_ref,
        matched_text: matched_text.chars().take(120).collect(),
    }
}

// ────────────────────────────── 独立加载函数 ──────────────────────────────

/// 从 YAML 文件加载规则列表（不创建引擎）
pub fn load_rules_from_file<P: AsRef<Path>>(path: P) -> Result<Vec<Rule>, EngineError> {
    let path_ref = path.as_ref();
    let text = std::fs::read_to_string(path_ref).map_err(|e| EngineError::FileRead {
        path: path_ref.display().to_string(),
        source: e,
    })?;
    load_rules_from_str(&text)
}

/// 从 YAML 文本加载规则列表（不创建引擎）
pub fn load_rules_from_str(yaml: &str) -> Result<Vec<Rule>, EngineError> {
    let file: crate::rules::schema::RuleFile = serde_yaml::from_str(yaml)?;
    Ok(file.rules)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::rules::schema::{Chapter, Clause, Severity};

    /// 一条可复用的最小规则 YAML。
    fn yaml_with(body: &str) -> String {
        format!(
            r#"
rules:
  - id: "TEST-001"
    category: "测试"
    industry: "通用"
    severity: "high"
    source:
      law: "测试法"
      article: "第一条"
{body}
    suggestion: "建议"
"#
        )
    }

    /// 构建一份简单文档。
    fn doc(
        file_name: &str,
        chapters: Vec<&str>,
        clauses: Vec<(&str, u32, &str)>,
        metrics: HashMap<String, f64>,
    ) -> ParsedDocument {
        ParsedDocument {
            file_name: file_name.to_string(),
            chapters: chapters.into_iter().map(|t| Chapter { title: t.to_string() }).collect(),
            clauses: clauses
                .into_iter()
                .map(|(id, page, text)| Clause {
                    id: id.to_string(),
                    page,
                    text: text.to_string(),
                })
                .collect(),
            metrics,
        }
    }

    fn empty_metrics() -> HashMap<String, f64> {
        HashMap::new()
    }

    /// keyword OR 命中
    #[test]
    fn keyword_or_hits_on_any_word() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["差别待遇", "歧视待遇"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "招标文件.docx",
            vec!["投标人资格要求"],
            vec![("C-1", 1, "招标人不得对潜在投标人实行差别待遇。")],
            empty_metrics(),
        );
        let hits = engine.run(&d);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rule_id, "TEST-001");
    }

    /// keyword 未命中（负例）
    #[test]
    fn keyword_misses_on_normal_text() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["差别待遇", "歧视待遇"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "招标文件.docx",
            vec!["投标人资格要求"],
            vec![("C-1", 1, "投标人须具备施工总承包二级及以上资质。")],
            empty_metrics(),
        );
        assert!(engine.run(&d).is_empty(), "无违规关键词不应命中");
    }

    /// regex 命中（含 \uXXXX 归一化为 \x{XXXX}）
    #[test]
    fn regex_hits_with_unicode_normalization() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "regex"
        value: "(投标人).*(本市|本省|[\\u4e00-\\u9fa5]{2}辖区).*(注册)"
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "招标文件.docx",
            vec!["投标人资格要求"],
            vec![("C-2", 3, "投标人须在本市注册成立满三年。")],
            empty_metrics(),
        );        let hits = engine.run(&d);
        assert_eq!(hits.len(), 1, "Unicode 码点区间应被归一化后命中");
        assert_eq!(hits[0].clause_id, "C-2");
    }

    /// regex 编译失败容错：单条正则失败只跳过该 pattern，不让整库加载失败
    #[test]
    fn bad_regex_skipped_but_rule_still_loads() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "regex"
        value: "["
      - type: "keyword"
        value: ["正常关键词"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("坏正则不应导致整库加载失败");
        assert_eq!(engine.rule_count(), 1);
        // 正则被跳过，keyword 仍可用
        let d = doc(
            "招标文件.docx",
            vec!["投标人资格要求"],
            vec![("C-1", 1, "包含正常关键词的文本。")],
            empty_metrics(),
        );
        let hits = engine.run(&d);
        assert_eq!(hits.len(), 1, "keyword pattern 应仍可命中");
    }

    /// field_compare：left < right 命中
    #[test]
    fn field_compare_hits_when_condition_true() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "field_compare"
        left: "投标截止日期 - 招标文件发出日期"
        operator: "<"
        right: 20
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let mut metrics = HashMap::new();
        metrics.insert("招标文件发出日期".to_string(), 100.0);
        metrics.insert("投标截止日期".to_string(), 115.0); // 差 15 天
        let d = doc("施工招标文件.docx", vec!["投标须知前附表"], vec![("C-1", 1, "公开招标")], metrics);
        let hits = engine.run(&d);
        assert_eq!(hits.len(), 1, "15 天 < 20 天应命中");
    }

    /// field_compare：满足条件时不命中（不误报）
    #[test]
    fn field_compare_no_hit_when_condition_false() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "field_compare"
        left: "投标截止日期 - 招标文件发出日期"
        operator: "<"
        right: 20
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let mut metrics = HashMap::new();
        metrics.insert("招标文件发出日期".to_string(), 100.0);
        metrics.insert("投标截止日期".to_string(), 125.0); // 差 25 天
        let d = doc("施工招标文件.docx", vec!["投标须知前附表"], vec![("C-1", 1, "公开招标")], metrics);
        assert!(engine.run(&d).is_empty(), "25 天 ≥ 20 不应命中");
    }

    /// field_compare：操作数解析失败保守返回 false
    #[test]
    fn field_compare_unresolvable_operand_is_conservative() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "field_compare"
        left: "不存在的字段"
        operator: ">"
        right: 10
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc("招标文件.docx", vec![], vec![("C-1", 1, "公开招标")], empty_metrics());
        assert!(engine.run(&d).is_empty(), "操作数无法解析应保守不命中");
    }

    /// conditions：exclude 优先于 trigger —— 命中 exclude 即不触发
    #[test]
    fn conditions_exclude_beats_trigger() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["地域限制关键词"]
    conditions:
      project_types: ["施工招标"]
      trigger:
        chapter_keywords: ["投标人资格"]
      exclude:
        project_types: ["国际招标"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        // 首条 clause 含"国际招标"→ 命中 exclude → 不触发（即使 trigger 满足、关键词命中）
        let d = doc(
            "招标文件.docx",
            vec!["投标人资格"],
            vec![("C-0", 1, "本项目为国际招标。"), ("C-1", 2, "含地域限制关键词的文本。")],
            empty_metrics(),
        );
        assert!(engine.run(&d).is_empty(), "命中 exclude.project_types 应被排除");
    }

    /// conditions：document_type 不匹配则不触发
    #[test]
    fn conditions_document_type_must_match() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["某关键词"]
    conditions:
      document_type: "投标文件"
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "招标文件.docx",
            vec![],
            vec![("C-1", 1, "包含某关键词的文本。")],
            empty_metrics(),
        );
        assert!(engine.run(&d).is_empty(), "文档类型不匹配不应触发");
    }

    /// conditions：trigger 章节不匹配则不触发
    #[test]
    fn conditions_trigger_chapter_required() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["某关键词"]
    conditions:
      trigger:
        chapter_keywords: ["投标人资格"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "招标文件.docx",
            vec!["其他章节"],
            vec![("C-1", 1, "包含某关键词的文本。")],
            empty_metrics(),
        );
        assert!(engine.run(&d).is_empty(), "触发章节不满足不应触发");
    }

    /// check: all_match —— 两个 pattern 同时命中才触发
    #[test]
    fn all_match_requires_all_patterns() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["中标通知书"]
      - type: "regex"
        value: "(有权|可)(单方)?变更中标(结果|人)"
    check: all_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");

        // 两模式都命中 → 触发
        let d1 = doc(
            "招标文件.docx",
            vec!["定标与授标"],
            vec![("C-9", 12, "中标通知书发出后，招标人有权单方变更中标结果。")],
            empty_metrics(),
        );
        assert_eq!(engine.run(&d1).len(), 1, "all_match 两模式齐备应触发");

        // 只命中关键词 → 不触发
        let d2 = doc(
            "招标文件.docx",
            vec!["定标与授标"],
            vec![("C-9", 12, "中标通知书发出后对招标人和中标人具有法律效力。")],
            empty_metrics(),
        );
        assert!(engine.run(&d2).is_empty(), "all_match 缺任一模式不应触发");
    }

    /// absence 缺失匹配：文档中没有该关键词即命中
    #[test]
    fn absence_mode_hits_when_keyword_missing() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["安全生产许可证"]
        match_mode: "absence"
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "投标文件.docx",
            vec!["资质证书"],
            vec![("C-1", 1, "营业执照、建筑业企业资质证书。")],
            empty_metrics(),
        );
        let hits = engine.run(&d);
        assert_eq!(hits.len(), 1, "缺少安全生产许可证应命中（absence）");
        assert_eq!(hits[0].clause_id, "<document>");
    }

    /// absence 缺失匹配：文档中有该关键词则不命中
    #[test]
    fn absence_mode_no_hit_when_keyword_present() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["安全生产许可证"]
        match_mode: "absence"
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("规则加载失败");
        let d = doc(
            "投标文件.docx",
            vec!["资质证书"],
            vec![("C-1", 1, "营业执照、安全生产许可证。")],
            empty_metrics(),
        );
        assert!(engine.run(&d).is_empty(), "已含安全生产许可证不应命中");
    }

    /// enabled=false 的规则不被加载
    #[test]
    fn disabled_rule_skipped() {
        let yaml = r#"
rules:
  - id: "TEST-001"
    category: "测试"
    industry: "通用"
    severity: "high"
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
        let engine = RuleEngine::from_yaml(yaml).expect("加载失败");
        assert_eq!(engine.rule_count(), 0, "enabled=false 的规则应被跳过");
    }

    /// 空规则集 + 空文档均不 panic
    #[test]
    fn empty_rules_and_doc_no_panic() {
        let engine = RuleEngine::from_yaml("rules: []").expect("空规则应加载成功");
        assert_eq!(engine.rule_count(), 0);
        let d = ParsedDocument::default();
        assert!(engine.run(&d).is_empty());
    }

    /// 命中结果按严重程度排序（critical 在前）
    #[test]
    fn matches_sorted_by_severity() {
        let yaml = r#"
rules:
  - id: "LOW-001"
    category: "测试"
    industry: "通用"
    severity: "low"
    source: { law: "测试法", article: "第一条" }
    patterns:
      - type: "keyword"
        value: ["低风险关键词"]
    check: any_match
    suggestion: "建议"
  - id: "CRIT-001"
    category: "测试"
    industry: "通用"
    severity: "critical"
    source: { law: "测试法", article: "第一条" }
    patterns:
      - type: "keyword"
        value: ["高风险关键词"]
    check: any_match
    suggestion: "建议"
"#;
        let engine = RuleEngine::from_yaml(yaml).expect("加载失败");
        let d = doc(
            "招标文件.docx",
            vec![],
            vec![("C-1", 1, "同时包含低风险关键词与高风险关键词。")],
            empty_metrics(),
        );
        let hits = engine.run(&d);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].rule_id, "CRIT-001", "critical 应排在 low 前面");
    }

    /// get_rule 按 ID 查找
    #[test]
    fn get_rule_by_id() {
        let yaml = yaml_with(
            r#"
    patterns:
      - type: "keyword"
        value: ["测试"]
    check: any_match
"#,
        );
        let engine = RuleEngine::from_yaml(&yaml).expect("加载失败");
        let rule = engine.get_rule("TEST-001").expect("规则应存在");
        assert_eq!(rule.severity, Severity::High);
        assert!(engine.get_rule("NOPE-999").is_none());
    }
}
