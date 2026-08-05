# Day-01 修改/写入文件说明

> 规则匹配模块开发 Day 1：测试先行 + 规则引擎骨架。
> 本文档记录本提交相对项目基线新增或修改的文件及其作用。

## 一、新增文件（backend-rust 规则引擎模块）

| 文件 | 作用 |
|---|---|
| `backend-rust/src/rules/mod.rs` | 模块入口，导出 `RuleEngine`、`Rule`、`RuleMatch`、`build_agent_context` |
| `backend-rust/src/rules/schema.rs` | 六要素规则模型（id/category/industry/severity/source/conditions + patterns + check）+ 文档模型（ParsedDocument/Clause/metrics），含 YAML 反序列化测试 |
| `backend-rust/src/rules/engine.rs` | 三合一匹配器（regex 预编译 + \uXXXX→\x{XXXX} 归一化 + 容错加载；keyword OR/absence 模式；field_compare 保守 false），conditions 优先级 exclude > document_type > trigger，20+ 条单测 |
| `backend-rust/src/rules/context.rs` | `build_agent_context(matches, doc)` 生成"已确认 vs 待审查"双段 Agent 上下文 |
| `backend-rust/src/rules/metrics.rs` | 文档级确定性指标提取（日期→天数、金额→元），供 field_compare 使用 |
| `backend-rust/src/bin/test_rules.rs` | 本地测试二进制：`--sample` 默认合成样本零 API 跑通；也可传入 PDF 路径（带 Python 兜底） |

## 二、修改文件

| 文件 | 修改内容 |
|---|---|
| `backend-rust/Cargo.toml` | 新增 3 个依赖：`serde_yaml = "0.9"`（YAML 规则库加载）、`thiserror = "1"`（引擎错误类型）、`log = "0.4"`（容错告警日志） |
| `backend-rust/src/lib.rs` | 新增 `pub mod rules;` 注册规则引擎模块 |

## 三、新增文件（规则库与测试资产）

| 文件 | 作用 |
|---|---|
| `rules/rules.yml` | 10 条六要素完整规则（DISC-001/BRAND-001/CAPITAL-001/PERF-001/TIME-001/DEPOSIT-001/BUDGET-001/CERT-001/QUAL-001/AWARD-001），项目根 git 核心资产 |
| `golden/expected_matches.json` | Golden Standard 回归集占位（Day 3 人工标注 3 份 × 20+ 条，粒度 `(clause_id, rule_id)`） |
| `benchmark/blind-rule/README.md` | 盲测集冻结方案（blind-a/b 两套、SHA-256 校验、来源与开发集零重叠，Day 4/5 用） |
| `docs/rule-engine-test-plan.md` | 三层测试内容设计（单元/回归/盲测），Day 1 编码前产出；Day 1 验收通过项以 `[√]` 标注 |




