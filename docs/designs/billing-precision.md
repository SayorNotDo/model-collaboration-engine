# 费用精度与供应商对账证据

## 本次完成的边界

新增 [离线账务审计示例](../../examples/billing_audit.py)，不调用模型、不修改数据库。
它可以汇总归档中的独立 cohort 指标，或根据显式提供的缓存 usage 和时段价表复算单次费用。
复算结果始终标记 `supplier_bill_verified: false`：提供价表与 usage 不等于取得供应商账单。
核心账本仍使用配置价与总输入/输出 usage；本次没有修改预算、预留、结算或恢复契约。

## 核实的供应商事实

2026-09-16T14:36:13Z 直接获取的[官方人民币价格页](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)：
原始 HTML SHA-256 为 `a1602748f50a9baacd416f4b831438896ba1351160f1f9afe89a9423c4b76cf3`。
下表为当时页面的人民币/百万 token 价格，不是未来价格承诺：

| 模型 | 时段 | 输入缓存命中 | 输入未命中 | 输出 |
| --- | --- | ---: | ---: | ---: |
| Flash | 空闲 | 0.02 | 1 | 4 |
| Flash | 高峰 | 0.04 | 2 | 8 |
| V4 Pro | 空闲 | 0.15 | 4.5 | 13.5 |
| V4 Pro | 高峰 | 0.30 | 9 | 27 |

页面规定高峰为北京时间周一至周五 09:00–12:00、14:00–18:00，其余为空闲。
页面未在本次读取内容中定义跨时段长请求使用何种计费时间点；不能将本地开始时间自动当成供应商的结算时点。
直接页面说明 9 月 14 日之后继续提供 V4 Pro 且计费不变；搜索缓存曾返回不同的路由说明，故不以搜索摘要覆盖直接页面。

[官方 usage 契约](https://api-docs.deepseek.com/api/create-chat-completion/)规定输入总数等于
`prompt_cache_hit_tokens + prompt_cache_miss_tokens`；缓存命中为零与字段缺失必须区分。
[缓存说明](https://api-docs.deepseek.com/guides/kv_cache/)说明缓存是尽力提供，重复提示不能证明命中。

## 归档结论

`artifacts/live-20260916-020845` 的三个 metrics 文件合计 25 次调用、29,970 microcredits
已记账配置价估算、0 未结算预留、0 未知费用调用。该目录的 `pricing.json` 明确采用高峰未命中上界估算，
规定每人民币元为 1,000,000 microcredits；因此 0.029970 元是该配置口径的估算，不是供应商实扣。
汇总前提是各 metrics 文件的调用互不重叠；工具会在报告中保留这一前提，不能用于累积快照相加。[本轮离线输出](../../artifacts/billing-audit-20260916.json)保留缺失证据与未知实扣金额。

归档指标缺少逐请求缓存拆分、适用计费时段和对应供应商账单。即使未结算预留为零，也不能宣布供应商对账完成。
旧的 `live-20260916-020623` 带有质量无效标记；本次没有将其质量结果纳入有效实验。

## 离线用法与输入

```powershell
.venv/Scripts/python.exe examples/billing_audit.py artifacts/live-20260916-020845
.venv/Scripts/python.exe examples/billing_audit.py --call-evidence call-evidence.json
```

单次复算输入如下（全部为合成样例，价格不代表当前供应商）：

```json
{
  "request_id": "synthetic-request",
  "model_version": "synthetic-v1",
  "usage": {
    "prompt_tokens": 1000,
    "completion_tokens": 100,
    "prompt_cache_hit_tokens": 800,
    "prompt_cache_miss_tokens": 200
  },
  "pricing": {
    "currency": "CNY",
    "version": "synthetic-rate-v1",
    "source": "synthetic test fixture",
    "band": "off_peak",
    "band_evidence": "synthetic supplier classification",
    "cache_hit_per_million": "0.1",
    "cache_miss_per_million": "1",
    "output_per_million": "2"
  }
}
```

价格必须为十进制字符串；使用 Decimal 复算原始币种金额，避免二进制浮点误差。
单个 JSON 文件最多 1 MiB；脚本校验对象形状、非负整数 usage、缓存分项一致性和有限非负价格，
畸形证据统一报告 `ValueError`；不验证外部证据真实性、换汇、供应商舍入或时段归属。
三个价格分项合计后除以一百万；结果是舍入前金额，不能直接用来覆盖整数账本。
`band` 接受 `peak`、`off_peak`、`flat`，每种都要求明确来源依据，不从文件夹日期猜测。

## 后续核心契约建议（尚未实现）

详细的目标接口、状态语义、数据库设计与实施步骤见
[供应商可核账费用证据设计](supplier-billing-evidence-design.md)。

1. 在 adapter 保留可选缓存分类和供应商响应 ID；缺失分类保留未知，协议扩展不进入策略逻辑。
2. 固定价表版本、原币种、计量单位、适用模型版本及价格生效期；预算准入继续使用保守上界。
3. 分开记录本地派发/接收时间和供应商计费时间依据。跨时段请求需供应商规则或逐请求账单支持。
4. 将 tariff estimate 与 supplier debit 分层保存。未知费用继续保留预留，不以复算成功代替供应商实扣证据。
5. 对账输入携带账单身份、币种、作用范围和调用关联，幂等 ID 拒绝冲突；审计调整与余额更新同事务。
6. 核心落地时同步 Rust/Python JSON、当前 schema、示例和异常路径测试；不得为新字段重建含真实费用的旧数据库。

本次验证：32 项离线测试通过，限定文件 Ruff 通过；实际运行归档汇总。
没有新模型调用、供应商账单导入或核心代码变更；Rust 构建与真实供应商对账不在本次已验证范围内。
