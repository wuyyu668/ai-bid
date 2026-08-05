//! RuleValidator —— 规则静态校验（Day 2）
//!
//! LLM 生成的候选规则在落 `rules/_drafts/` 前必须通过 5 项静态校验：
//!
//! 1. **法条名称存在性** — 对照本地法规清单（项目无 Neo4j，用内置清单）
//! 2. **条款编号存在性** — `第十八条` 与 `第18条` 归一化后格式合法
//! 3. **正则语法可编译** — 复用引擎的 `normalize_regex`（\uXXXX → \x{XXXX}）
//! 4. **conditions 自洽** — trigger / exclude 不矛盾（同一项目类型既触发又排除）
//! 5. **industry 合法** — 在分类体系内（建筑工程 / 政府采购 / IT / 通用）
//!
//! 统计：`ValidationReport { total, passed, issues }`，通过率 = passed / total。
//! Day 2 验收门槛：LLM 生成候选规则校验通过率 > 80%。

use crate::rules::engine::normalize_regex;
use crate::rules::schema::Rule;
use regex::Regex;

// ────────────────────────────── 常量清单 ──────────────────────────────

/// 本地法规清单（法条名称 → 最高条款序号）。
///
/// 项目暂无 Neo4j 法规知识库，先用内置清单覆盖常用法规。
/// 清单需人工维护：新增规则使用新法条时，在此登记条款上限。
pub const KNOWN_LAWS: &[(&str, u32)] = &[
    ("中华人民共和国招标投标法", 68),
    ("中华人民共和国招标投标法实施条例", 87),
    ("中华人民共和国政府采购法", 88),
    ("中华人民共和国政府采购法实施条例", 81),
    ("政府采购货物和服务招标投标管理办法", 76),
    ("建设工程安全生产管理条例", 70),
    ("危险性较大的分部分项工程安全管理规定", 45),
    ("中华人民共和国民法典", 1260),
    ("中华人民共和国建筑法", 85),
    ("中华人民共和国合同法", 428),
];

/// 行业分类体系
pub const INDUSTRIES: &[&str] = &["建筑工程", "政府采购", "IT", "通用"];

/// 检查结果条目
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub rule_id: String,
    pub field: &'static str,
    pub message: String,
}

/// 校验报告
#[derive(Debug, Clone, Default)]
pub struct ValidationReport {
    pub total: usize,
    pub passed: usize,
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// 通过率（0.0 ~ 1.0）。无规则时视为 1.0。
    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }

    pub fn is_pass(&self) -> bool {
        self.issues.is_empty()
    }
}

// ────────────────────────────── 校验器 ──────────────────────────────

/// 规则静态校验器。
pub struct RuleValidator;

impl RuleValidator {
    /// 对单条规则执行 5 项静态校验，返回校验报告（total = 1）。
    pub fn validate(rule: &Rule) -> ValidationReport {
        let mut issues = Vec::new();

        // ① 法条名称存在性
        if !KNOWN_LAWS.iter().any(|(law, _)| *law == rule.source.law) {
            issues.push(ValidationIssue {
                rule_id: rule.id.clone(),
                field: "source.law",
                message: format!(
                    "法条名称不在本地法规清单内: '{}'（请登记到 KNOWN_LAWS）",
                    rule.source.law
                ),
            });
        }

        // ② 条款编号存在性（归一化：第十八条 / 第18条 / 十八条）
        let max_article = KNOWN_LAWS
            .iter()
            .find(|(law, _)| *law == rule.source.law)
            .map(|(_, max)| *max);
        match normalize_article(&rule.source.article) {
            Some(n) => {
                if let Some(max) = max_article
                    && n > max
                {
                    issues.push(ValidationIssue {
                        rule_id: rule.id.clone(),
                        field: "source.article",
                        message: format!(
                            "条款序号 {} 超出《{}》上限（{} 条）",
                            n, rule.source.law, max
                        ),
                    });
                }
            }
            None => issues.push(ValidationIssue {
                rule_id: rule.id.clone(),
                field: "source.article",
                message: format!(
                    "条款编号格式不合法: '{}'（支持 '第十八条' / '第18条'）",
                    rule.source.article
                ),
            }),
        }

        // ③ 正则语法可编译（含 \uXXXX 归一化）
        for (i, p) in rule.patterns.iter().enumerate() {
            if p.ptype == crate::rules::schema::PatternType::Regex {
                if let Some(val) = p.value.as_ref().and_then(|v| v.as_single()) {
                    let normalized = normalize_regex(&val);
                    if let Err(e) = Regex::new(&normalized) {
                        issues.push(ValidationIssue {
                            rule_id: rule.id.clone(),
                            field: "patterns.regex",
                            message: format!("第 {} 个 regex 编译失败: {e}（pattern = {val}）", i + 1),
                        });
                    }
                } else {
                    issues.push(ValidationIssue {
                        rule_id: rule.id.clone(),
                        field: "patterns.regex",
                        message: "regex pattern 缺少 value".to_string(),
                    });
                }
            }
        }

        // ④ conditions 自洽：trigger 与 exclude 不矛盾
        if let (Some(tkw), Some(ekw)) = (
            rule.conditions.trigger.as_ref().and_then(|t| t.chapter_keywords.as_ref()),
            rule.conditions.exclude.as_ref().and_then(|e| e.clause_keywords.as_ref()),
        ) {
            let overlap: Vec<&String> =
                tkw.iter().filter(|k| ekw.contains(k)).collect();
            if !overlap.is_empty() {
                issues.push(ValidationIssue {
                    rule_id: rule.id.clone(),
                    field: "conditions",
                    message: format!(
                        "trigger.chapter_keywords 与 exclude.clause_keywords 重叠: {:?}",
                        overlap
                    ),
                });
            }
        }
        // exclude.project_types 与 project_types 同时命中同一值 → 自相矛盾
        if let (Some(pts), Some(excl)) = (
            rule.conditions.project_types.as_ref(),
            rule.conditions.exclude.as_ref().and_then(|e| e.project_types.as_ref()),
        ) {
            let overlap: Vec<&String> = pts.iter().filter(|p| excl.contains(p)).collect();
            if !overlap.is_empty() {
                issues.push(ValidationIssue {
                    rule_id: rule.id.clone(),
                    field: "conditions",
                    message: format!(
                        "project_types 与 exclude.project_types 重叠: {:?}",
                        overlap
                    ),
                });
            }
        }

        // ⑤ industry 在分类体系内
        if !INDUSTRIES.contains(&rule.industry.as_str()) {
            issues.push(ValidationIssue {
                rule_id: rule.id.clone(),
                field: "industry",
                message: format!(
                    "industry '{}' 不在分类体系内 {:?}",
                    rule.industry, INDUSTRIES
                ),
            });
        }

        let total = 1;
        let passed = if issues.is_empty() { 1 } else { 0 };
        ValidationReport {
            total,
            passed,
            issues,
        }
    }

    /// 对规则列表批量校验，汇总报告（total = 规则数）。
    pub fn validate_all(rules: &[Rule]) -> ValidationReport {
        let mut report = ValidationReport::default();
        for rule in rules {
            report.total += 1;
            let r = Self::validate(rule);
            report.passed += r.passed;
            report.issues.extend(r.issues);
        }
        report
    }

    /// 校验通过的规则子集
    pub fn passing(rules: &[Rule]) -> Vec<&Rule> {
        rules.iter().filter(|r| Self::validate(r).is_pass()).collect()
    }
}

// ────────────────────────────── 工具函数 ──────────────────────────────

/// 条款编号归一化：`第十八条` / `第18条` / `十八条` → 数字。
/// 中文数字支持一~九十九（以及十、二十等）。
pub fn normalize_article(article: &str) -> Option<u32> {
    let s = article.trim();
    let s = s.strip_prefix('第')?.trim_end_matches('条');
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u32>() {
        return Some(n);
    }
    // 中文数字
    let digits = ['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'];
    let mut total: u32 = 0;
    let mut cur: u32 = 0;
    for ch in s.chars() {
        match ch {
            '十' => {
                if cur == 0 {
                    cur = 1;
                }
                total += cur * 10;
                cur = 0;
            }
            '百' => {
                total += cur.max(1) * 100;
                cur = 0;
            }
            c => {
                let d = digits.iter().position(|&d| d == c)?;
                cur = d as u32;
            }
        }
    }
    Some(total + cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn article_normalization_works() {
        assert_eq!(normalize_article("第十八条"), Some(18));
        assert_eq!(normalize_article("第18条"), Some(18));
        assert_eq!(normalize_article("第2条"), Some(2));
        assert_eq!(normalize_article("第一百二十条"), Some(120));
        assert_eq!(normalize_article("第3款"), None);
        assert_eq!(normalize_article(""), None);
    }

    #[test]
    fn known_laws_include_common_ones() {
        assert!(KNOWN_LAWS
            .iter()
            .any(|(law, _)| *law == "中华人民共和国招标投标法"));
        assert!(INDUSTRIES.contains(&"建筑工程"));
    }
}
