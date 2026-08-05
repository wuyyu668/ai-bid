# Day-02 修改/写入文件说明

> 规则匹配模块开发 Day 2：规则库（TDD）+ LLM 辅助生成规则。
> 本文档记录本提交相对项目基线（含 Day 1 成果）新增或修改的文件及其作用。
> 覆盖率分析见 `docs/rule-engine-coverage-analysis.md`，验收勾选与推进记录见 `docs/规则匹配模块开发排期说明.md`。

## 一、新增文件（backend-rust 规则引擎模块）

| 文件 | 作用 |
|---|---|
| `backend-rust/src/rules/validator.rs` | `RuleValidator` 5 项静态校验（法条名存在性 / 条款编号存在性 / 正则可编译 / conditions 自洽 / industry 合法）+ `normalize_article`（中文数字归一化）+ `KNOWN_LAWS` 法规清单 + `INDUSTRIES` 分类体系；含 2 个单测 |
| `backend-rust/src/agents/tools/generate_rule.rs` | `GenerateRuleTool`（AgentTool 实现）：LLM YAML 草稿生成 → `RuleValidator` 校验 → 通过落 `rules/_drafts/<id>-<ts>.yml`，注册进 `tools/mod.rs`；含 `extract_yaml`（围栏剥离）2 个单测 |
| `backend-rust/src/agents/prompts.rs` | ⚠️ 修改（新增 `RULE_EXTRACT_SYSTEM_PROMPT`）——Few-shot 2 例（keyword 型 SAFE-003 + field_compare 型 DEPOSIT-003），强约束输出与六要素规则模型一致的 YAML |
| `backend-rust/src/bin/validate_rules.rs` | 规则库全量静态校验二进制：`cargo run --bin validate_rules` → 27/27、通过率 100% |
| `backend-rust/src/bin/test_rule_cases.rs` | 正负例回归工具：读 `rules/cases.json` 逐条断言，失败非零退出；结果 27/27 |
| `backend-rust/src/bin/eval_rule_generation.rs` | LLM 候选通过率评测（验收指标 > 80%）：`--offline` mock 模式 7/8 = 87.5%；在线模式需有效 `DASHSCOPE_API_KEY` |
| `backend-rust/tests/rules_cases.rs` | 回归集成测试 3 项：正负例断言全通过 / 每规则必有用例（覆盖检查）/ 静态校验通过率 > 80% 与规则数 = 27 |

## 二、修改文件（backend-rust）

| 文件 | 修改内容 |
|---|---|
| `backend-rust/src/rules/mod.rs` | 注册 `pub mod validator;`、导出 `RuleValidator` |
| `backend-rust/src/rules/metrics.rs` | 新增 `履约保证金金额` / `中标金额` 提取键（DEPOSIT-002 等 field_compare 依赖），新增 `extract_metrics_for_day2_rules` 单测 |
| `backend-rust/src/rules/engine.rs` | `normalize_regex` 由私有改为 `pub(crate)`（validator 复用） |
| `backend-rust/src/api/handlers.rs` | 新增 `POST /api/v1/rules/validate`（全库静态校验，返回 total/passed/pass_rate/issues，实测 27/27 / 1.0）与 `POST /api/v1/rules/generate`（LLM 草稿 + 校验结果）两个 handler + `GenerateRuleRequest` DTO |
| `backend-rust/src/api/router.rs` | 注册上述两个路由 + OpenAPI `rules` tag / schema / path |

## 三、规则库与测试资产（项目根，git 核心资产）

| 文件 | 作用 |
|---|---|
| `rules/rules.yml` | ⚠️ 修改：规则库 10 → 27 条，覆盖 DISC/QUAL/EXPR/TIME/SAFE/PROC/AWARD/DEPOSIT/BUDGET/TEAM 类别，六要素完整，全库静态校验 27/27（100%） |
| `rules/cases.json` | ⚠️ 新增：27 条规则 × 正负例各 1（含 document_type/chapters 字段；field_compare 规则由引擎从条款文本实时 `extract_metrics`，正负例天然分离）；回归工具 `test_rule_cases` 27/27 |

## 四、文档（docs/）

| 文件 | 作用 |
|---|---|
| `docs/规则匹配模块开发排期说明.md` | ⚠️ 修改：Day 2 四项验收打勾 + 文末追加「推进记录 · Day 2 完成」 |
| `docs/rule-engine-coverage-analysis.md` | ⚠️ 新增：覆盖率分析报告——AI 候选 vs 人工手写对比、RuleValidator 拦截 AI 高频错误汇总、三类 AI 盲区（跨条推断 / 行业知识 / 案例关联）、互补分工与人工确认门槛 |

## 五、验证记录

```powershell
cd backend-rust
cargo check                                # ✅
cargo test --lib rules                     # ✅ 36/36
$env:AIBID_DATA_DIR=".."
cargo test --test rules_cases              # ✅ 3/3
cargo run --bin validate_rules             # ✅ 27/27 通过率 100%
cargo run --bin test_rule_cases            # ✅ 27 通过 / 0 失败
cargo run --bin eval_rule_generation -- --offline --limit=8   # ✅ 7/8 = 87.5% > 80%
cargo clippy --lib --tests -- -D warnings -A clippy::unnecessary_lazy_evaluations -A clippy::unnecessary_sort_by -A clippy::question_mark  # ✅ 0 告警
```

> 说明：clippy 豁免 3 项为本模块之外既存告警（risk_taxonomy.rs / check_cross_reference.rs / embedding_service.rs），本模块 0 告警。
> 全库既有 1 个失败与本模块无关：`services::docx_convert_service::tests::test_find_soffice`（LibreOffice 未安装）。
> 真实 LLM 在线评测（`eval_rule_generation` 在线模式）需配置有效 `DASHSCOPE_API_KEY`。