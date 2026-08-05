//! 规则匹配引擎模块
//!
//! 确定性规则引擎：加载 YAML 规则库，对解析后的文档运行
//! regex / keyword / field_compare 三合一匹配，输出 `RuleMatch[]`，
//! 并生成 Agent System Prompt 上下文（`build_agent_context`）。
//!
//! 模块结构（对应 `docs/规则匹配模块开发排期说明.md` Day 1）：
//!
//! - [`schema`]：六要素规则模型 + 文档模型（YAML 反序列化）
//! - [`engine`]：三合一匹配器 + 预编译正则 + 容错加载
//! - [`context`]：`build_agent_context` 生成 Agent 上下文
//!
//! ## 与 Agent 的边界
//!
//! 规则引擎的输出不是终端报告，而是 Agent 的 System Prompt 上下文。
//! 能用"是/否"回答的问题 → 规则引擎；需要语义理解的问题 → Agent。

pub mod context;
pub mod engine;
pub mod metrics;
pub mod schema;

pub use context::build_agent_context;
pub use engine::{EngineError, RuleEngine, load_rules_from_file, load_rules_from_str};
pub use metrics::extract_metrics;
pub use schema::{ParsedDocument, RuleMatch};
