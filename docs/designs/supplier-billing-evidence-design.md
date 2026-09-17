# 供应商可核账费用证据设计

状态：目标设计，尚未实现；优先级位于混合任务评估与运行时升级之后。

## 目标

让每次真实模型调用保存足以解释本地账本费用、并可与供应商账单逐项关联的证据。
缺失用量、缓存分类、计费时间或账单明细时保留未知，不把价格估算描述为供应商实扣。

本设计深化现有 `ModelEvidence`、派发结算和 `Store` 事务，不建立第二套调用或账本流程。
工具费用继续采用宿主报告的 `actual_cost`，不在本阶段接入供应商模型账单契约。

## 核心决定

一次模型调用存在两个相互独立的状态维度：

| 维度 | 用途 | 状态 |
| --- | --- | --- |
| 本地账务 | 任务预算准入、预留释放和引擎指标 | `pending`、`settled`、`unresolved` |
| 供应商核实 | 证明供应商实际扣款及其来源 | `unverified`、`verified` |

本地账务使用任务开始时固定的价格快照，将已知用量换算为整数 microcredits。
它可以是精确估算或保守上界，但始终属于引擎的记账口径。供应商扣款保存原币种和账单身份；
只有宿主同时提供有版本的换算证据时，才额外保存可比较的 microcredits 值。

供应商扣款不会反向改变任务的预算账本。原因是任务预算使用宿主配置的统一 microcredits，
供应商账单可能采用另一币种、汇率、税费、批量折扣或账后调整。指标分别汇总两种口径，
不将它们相加或相互覆盖。

## 模块与 seam

### adapter：协议证据归一化

`ModelAdapter` 负责把协议字段归一化为供应商无关的 `UsageEvidence`，不计算全局预算，
也不解释供应商账单。Chat Completions 和 Responses 是该 seam 上已有的两个协议 adapter。

`ModelEvidence` 继续作为调用 future 与结算之间的共享证据所有者。建议将当前分散参数收敛为：

```rust
pub struct UsageEvidence {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: Option<u64>,
}

pub struct ProviderCallEvidence {
    pub request_id: Option<String>,
    pub usage: Option<UsageEvidence>,
    pub supplier_billed_at_ms: Option<u64>,
}

impl ModelEvidence {
    pub fn record(&self, evidence: ProviderCallEvidence) -> Result<()>;
}
```

`cached_input_tokens` 是 `input_tokens` 的子集，存在时必须不大于总输入量。缺失表示供应商没有提供
可验证分类，不表示零。重复帧允许补充空缺字段；同一字段出现冲突值时返回协议错误，同时保留首次收到的
证据供结算使用。供应商专用原始字段不进入策略、路由或公共任务契约。

引擎记录 `dispatched_at_ms`、`received_at_ms`；adapter 只在协议确实提供时记录
`supplier_billed_at_ms`。本地时间不能冒充供应商计费时间。

### billing：纯费用计算模块

新增 `src/billing.rs`，通过一个小接口隐藏缓存分类、价格有效期、舍入和溢出规则：

```rust
pub fn estimate_charge(
    usage: &UsageEvidence,
    tariff: &TariffSnapshot,
    timing: &BillingTiming,
) -> Result<ChargeEstimate>;
```

此模块不访问网络、数据库或当前时间，不引入 trait。当前只有一套统一计算规则；出现第二种实际算法前，
新的可替换 seam 没有收益。

`ChargeEstimate` 至少包含：

- `amount_microcredits`；
- `basis`：`exact` 或 `upper_bound`；
- 使用的 token 分类和缺失分类；
- `tariff_version`、模型版本及价格有效期；
- 计算规则版本。

所有金额计算使用受检整数运算。溢出不得饱和成最大值；调用证据照常保存，账务保持 `unresolved`，
并以预算错误终止当前执行路径。

### engine：固定价格快照并组织结算

每次 `reserve` 时把 `TariffSnapshot` 放入 attempt metadata，使配置重载不能改变已发生调用的价格依据。
快照包含：

- 模型候选 ID、供应商、供应商侧模型名及模型版本；
- `price_version` 和计算规则版本；
- 统一记账币种；
- 普通输入、缓存输入和输出的每 token microcredits；
- 价格生效起止时间；
- 可选的原币种及换算版本。

配置仍以 microcredits 作为预算单位。新增缓存价时，缺失缓存分类按普通输入价形成保守上界；
配置校验要求缓存输入价不高于普通输入价。请求跨越价格有效期，且缺少供应商计费时间或明确适用规则时，
账务保持 `unresolved`，不能拿调用开始时间推断供应商一定采用旧价格。预算准入仍使用已有保守预留。

配置契约具体增加：全局 `accounting_currency`；模型级 `cached_input_price`、
`price_effective_from_ms`、`price_effective_until_ms`，以及成对出现的可选 `source_currency`、
`conversion_version`。microcredit 固定为记账币种主要单位的百万分之一；所有模型必须使用同一记账币种，
因此任务预算仍可相加。`cached_input_price` 缺失表示与普通输入价相同，不表示免费；有效期端点缺失表示
对应方向无界。仓库内配置、Python 解析、示例和测试调用方在同一批迁移。

派发完成后的顺序保持不变：读取共享证据、计算费用、在同一结算事务中保存 outcome 和账本，
再向策略返回调用结果。取消、超时和协议解析失败不能跳过该顺序。

### store：账本事务与供应商核实

SQLite 当前 schema 从 2 更新为 3，不提供自动迁移。schema 3 新增 `billing_reconciliations` 表，
而调用时的用量、价格快照和本地估算继续随 attempt 保存。建议字段：

```text
billing_reconciliations
├── reconciliation_id TEXT PRIMARY KEY
├── payload_hash TEXT NOT NULL
├── task TEXT NOT NULL
├── attempt TEXT NOT NULL
├── kind TEXT NOT NULL
├── provider TEXT
├── statement_id TEXT
├── line_item_id TEXT
├── payload TEXT NOT NULL
└── created_at_ms INTEGER NOT NULL
```

`(provider, statement_id, line_item_id)` 对供应商扣款建立唯一约束，避免换一个本地 ID 重复导入同一账单行。
表中的 `payload` 保存版本化结构，索引字段只承担身份与查询。所有账本、attempt、核实记录和审计更新仍在
一个事务内完成。

真实费用数据库不原地重建。升级时先备份并保留 schema 2 数据库用于已有费用证据查询和对账，
再为 schema 3 使用显式的新 `database_path`。

## 公共契约

新增一个深模块接口，而不是分别暴露多组存储方法：

```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BillingReconciliation {
    AccountedCost(AccountedCostReconciliation),
    SupplierDebit(SupplierDebitReconciliation),
}

impl Engine {
    pub async fn reconcile_billing(&self, value: &BillingReconciliation) -> Result<()>;
}
```

Python 对应为：

```python
await engine.reconcile_billing(record)
```

公共输入共同包含 `version`、`reconciliation_id`、`task_id`、`attempt_id` 和有限长度的
`evidence_ref`。`evidence_ref` 是宿主管理的账单或审计定位符，不保存密钥、完整账单文件或任意大文本。

### `accounted_cost`

用于解决原始 usage 缺失导致的本地 `unresolved`：

- 输入 `cost_microcredits` 和核实依据；
- 只允许处理尚未结算的 attempt；
- 在同一事务中释放预留、增加 settled、写入核实记录和审计；
- 实际值超过预留时先提交，再返回预算错误；
- 已结算 attempt 不通过该类型改价。

这取代当前仅靠 `(task, attempt, cost, evidence)` 判断重复的内部接口，使幂等身份可持久化查询。

### `supplier_debit`

保存一次供应商账单行与调用的关联：

- `provider`、`provider_request_id`；
- `statement_id`、`line_item_id`；
- `currency`、非负整数 `amount_minor` 和 `minor_unit_exponent`；
- 可选 `billed_at_ms`；
- 可选 `normalized_microcredits`、`conversion_version`。

存在已保存 request ID 时必须精确匹配；缺少 request ID 时允许由宿主使用账单行身份关联，但在记录中标为
弱关联。`normalized_microcredits` 与换算版本必须同时出现。供应商扣款可以在本地账务仍 unresolved 时保存，
但不会自动推导本地费用。

相同 `reconciliation_id` 和相同 payload 幂等成功；相同 ID 不同 payload、相同账单行关联不同 attempt、
或同一 attempt 重复导入另一账单行均拒绝为冲突。首版不提供修改、撤销或拆分账单行接口；发生账后调整时
保留原记录并进入后续独立设计。

## 状态和失败语义

| 场景 | 本地账务 | 供应商核实 | 执行结果 |
| --- | --- | --- | --- |
| 派发前取消或超时 | 以 0 结算 | 不需要 | 返回取消或截止错误 |
| usage 完整且价格适用 | 精确结算 | 未核实 | 保留模型结果或原错误 |
| usage 有总量但无缓存分类 | 按普通输入价以上界结算 | 未核实 | 保留模型结果或原错误 |
| usage 缺失 | 保留预留，标为 unresolved | 未核实 | 返回调用结果对应的终态 |
| usage 已收到后解析失败、取消或超时 | 按已收到证据结算 | 未核实 | 返回协议、取消或截止错误 |
| 费用计算溢出或无法证明上界 | 保留预留，标为 unresolved | 未核实 | 返回预算或计费错误 |
| 导入供应商账单行 | 不改变 | verified | 不改变原任务终态 |
| accounted cost 超出预留 | 先结算并释放预留 | 不改变 | 提交后返回预算错误 |

结算或核实记录的存储失败不回退成其他候选，也不把费用视为零。关闭流程必须等待正在执行的结算事务；
账单核实是独立宿主调用，不参与任务 deadline 或模型调用次数。

## 查询与指标

`RecoveryRecord.attempts[]` 增加结构化 `billing` 投影，至少返回 usage、时间、价格快照、估算依据和关联的
核实记录。旧的 `metadata`、`outcome` 动态 JSON 在 schema 3 内仍保留作审计载荷，但公共调用方不需要自行
解析它们才能判断费用状态。

`metrics()` 保持现有本地账务统计，并新增独立字段：

- 精确估算、上界估算和 unresolved 的调用数；
- 已关联供应商扣款的调用数；
- 按原币种分组的供应商扣款；
- 有换算证据时的 normalized microcredits 总额；
- 本地估算与 normalized supplier debit 的差异，且只比较口径完整的样本。

不同币种或缺失换算版本的数据不得合计。供应商已核实不表示业务验收通过，也不更新质量画像。

## 实施步骤

### 1. 契约与纯计算

新增 `src/contracts/billing.rs` 和 `src/billing.rs`，迁移 `Usage`，补齐配置字段及校验。
先用表驱动测试固定普通输入、缓存输入、缺失分类、价格有效期、零费用和整数溢出行为。

### 2. adapter 与取消证据

更新两种协议的 usage 解析和 `ModelEvidence` 合并规则。覆盖正常完成、usage 后非法工具参数、尾帧损坏、
取消和超时；每个场景同时断言 request ID、usage 分类、时间和账本状态。

### 3. 派发与持久化

在 reserve 时保存价格快照，由 billing 模块计算 `ChargeEstimate`，更新 schema 3、结算事务和恢复投影。
`Store` 测试覆盖重开数据库、结算失败、超预留先记账和并发读取的一致性。

### 4. 核实接口与 Python 桥接

实现 `BillingReconciliation`、单一 Store 方法、Engine 方法和 Python JSON 接口。测试幂等、冲突、错误
request ID、重复账单行、原币种零扣款及带版本的换算证据。

### 5. 指标、示例与真实试验

扩展 metrics 和离线计费审计示例。选择少量真实调用，将供应商请求 ID 与账单行关联，分别报告本地估算、
供应商原币种扣款和换算差异。无法获得逐调用账单时只报告证据缺口。

### 6. 文档与技术图

实现时同步 `README.md`、`docs/usage.md`、`config/README.md`、示例配置、代码入口和当前状态。
该实现会改变 adapter、engine、store 的跨层费用流，因此届时按技术图工作流更新架构图 JSON 并重新执行
结构、浏览器和视觉验收；本目标设计不修改当前实现图。

## 完成条件

1. 两种内置协议均能持久化可选缓存分类、请求 ID 和调用时间，且异常路径不会丢失已收到证据。
2. 每个已派发 attempt 都能区分精确估算、上界估算和 unresolved；缺失字段不解释为零。
3. 价格快照可在数据库重开后复算出相同本地费用，配置变化不改变历史证据。
4. accounted cost 和 supplier debit 均具有持久化幂等身份，冲突不会重复记账或重复导入。
5. 供应商扣款按原币种查询；只有存在换算版本时才参与 microcredits 差异统计。
6. 超预留、取消、超时、协议错误和存储错误保持现有终止与清理语义。
7. Rust、Clippy、fmt、原生扩展重建、Python、Ruff 和文档检查通过；真实联调与离线模拟分别报告。

## 暂缓范围

- 自动下载或解析供应商账单；
- 税费、返利、批量折扣及账后调整的自动分摊；
- 自动数据库迁移；
- 自动任务恢复或重放；
- 用供应商扣款直接训练质量画像或改变运行中路由；
- semantic critic 和协作总成本预测。
