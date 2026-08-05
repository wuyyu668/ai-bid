# Blind Benchmark — 规则匹配模块泛化盲测集（Day 1 冻结）

本目录是规则匹配模块（`backend-rust/src/rules/`）的**泛化盲测集**，用于验证：
**换一批与开发集来源零重叠的真实标书，规则引擎是否仍有泛化能力**（正确命中 + 不误报）。

## 定位

| 集 | 用途 | 是否参与调优 |
|---|---|---|
| `golden/expected_matches.json` | 开发 / 回归集（规则质量衡量） | 是 |
| **本目录（blind-rule-a / blind-rule-b）** | 验收集（泛化能力验收） | **否** |

> 边界纪律：开发集指标好 ≠ 泛化好。本盲测集不得作为训练集、提示词示例或规则调优样本。
> 参考 `benchmark/blind-v2/README.md` 的冻结规则。

## 数据构成（计划）

- 8-10 份真实招标文件，**来源与开发集（3 份）零重叠**
- 覆盖行业：建筑工程 / 政府采购 / IT 等
- 每份人工标注已知问题（规则引擎确定性可判定的条款）：
  - 地域限制（DISC-001）
  - 模糊资质表述（QUAL-001）
  - 投标准备期 < 20 日（TIME-001）
  - 保证金比例超 2%（DEPOSIT-001）
  - 指定品牌 / 供应商（BRAND-001）
  - 日期冲突 / 缺安全生产许可证（CERT-001）
- 拆两套：
  - `blind-rule-a/` —— Day 4 首轮
  - `blind-rule-b/` —— Day 5 修改后复测（独立确认泛化提升）

## 冻结规则

- `data/freeze_manifest.json` 保存原始 PDF、测试 PDF、真值和来源清单的 SHA-256
- 运行前自动复核哈希；任一文件改变即中止盲测
- 校验命令（Day 4 实现后）：

```powershell
python benchmark/build_blind_rule_set.py --verify
```

## 验收

- Day 4：首轮盲测记录（文档、真值、预测、指标、SHA 校验通过）
- Day 4：泛化失败根因分析 + 盲测意见清单
- Day 5：按意见修改规则后，`blind-rule-b` 复测泛化提升；Golden Standard Recall 不下降
