# 混合任务评估与运行时升级设计

状态：目标设计，作为下一项优先实现；尚未修改运行时代码。

## 目标

以“确定性规则 + 类型化概率判断 + 可选规划 + 有界运行时升级”形成首轮模型选择和失败兜底闭环。
规则处理宿主可验证的硬信号，决策模型判断需求模糊度、推理强度和协作需求，planner 生成需要的计划补充，
运行时依据实际产物评价决定是否升级。这些模块都不能放宽宿主权限、预算、地域或数据约束；决策模型和
planner 也不能直接指定供应商模型。

这不是训练复杂 ML 分类器，也不是要求一个模型回答笼统的“任务难不难”。目标是把不同来源的证据分层，
保存可复算的合并结果，并复用现有任务适配路由和 cascade 执行循环。

## 领域区分

| 概念 | 表达什么 | 不表达什么 |
| --- | --- | --- |
| 任务事实 | 宿主声明的结构化、可验证事实 | evidence 文本中的自我声明 |
| 规则结论 | 某条版本化规则对最低执行档位的要求 | 最终模型身份或质量概率 |
| 决策信号 | 原子语义问题的类型化值、概率和来源版本 | 宿主事实、权限或单次正确性保证 |
| 规划建议 | 类型、策略、能力、验收与执行档位建议 | 决策概率或工具授权 |
| 任务需求评估 | 规则、宿主选择、有效决策信号和规划建议的固定合并结果 | 任务客观难度或模型必然成功率 |
| 执行档位 | 宿主配置的有序候选组 | 参数量、供应商等级或固定模型 ID |
| 动态升级 | 评价不通过后提高候选要求并重新路由 | 对任意失败无条件换更贵模型 |

首版执行档位使用 `simple`、`medium`、`hard`。这些名称只描述本次任务允许的最低候选组：
例如宿主可以把若干 14B 候选配置为 simple、30B-A3B 配置为 medium、frontier 候选配置为 hard，
但内核不解析参数量，也不假定更大模型一定具有更高任务质量。

## 总体流程

```text
Submission
   │
   ├─ validate host constraints and task facts
   │
   ├─ deterministic rule assessment ─────────────┐
   │                                              │
   ├─ optional typed decision assessment ─────────┤
   │                                              │
   └─ optional bounded planner proposal ──────────┤
                                                  ▼
                                      merge and validate assessment
                                                  │
                                      freeze effective plan + routing snapshot
                                                  │
                                      route within eligible execution classes
                                                  │
                                           execute and evaluate
                                                  │
                         ┌────────────────────────┼────────────────────────┐
                         ▼                        ▼                        ▼
                       pass              retryable quality defect   stop condition
                         │                        │                        │
                      complete       raise class/Q if resources allow  human_required
```

## 模块与 seam

### task assessment：纯规则与合并模块

新增 `src/assessment.rs`，承担两项纯行为：

```rust
pub fn evaluate_rules(
    facts: &TaskFacts,
    rules: &RuleSet,
) -> Result<RuleAssessment>;

pub fn merge_assessment(
    submission: &SubmissionSpec,
    rules: &RuleAssessment,
    decisions: Option<&DecisionAssessment>,
    proposal: Option<&PlannerProposal>,
) -> Result<TaskAssessment>;
```

该模块不访问模型、数据库、文件系统或当前时间。调用方只需理解输入事实和最终 `TaskAssessment`；
规则匹配、优先级、默认值和冲突处理隐藏在模块内部。现有 `planning::validate_plan` 调用合并接口，
不复制规则逻辑。

### decision model：供应商无关的类型化判断

Jev 未开源且只提供托管接口；公开资料不足以复现其模型结构、训练方法和校准过程。因此内核不实现所谓
“Jev 架构”，而定义项目拥有的 `DecisionModel` seam。Jev 只是其中一个 adapter；领域契约不引用供应商 SDK
类型、模型别名或专有的 Choice、Score、Noul 类。

```rust
pub struct DecisionRequest {
    pub question_set_version: String,
    pub state: DecisionState,
    pub questions: Vec<DecisionQuestion>,
}

pub struct DecisionAssessment {
    pub question_set_version: String,
    pub adapter_id: String,
    pub actual_model: String,
    pub signals: Vec<DecisionSignal>,
}

pub trait DecisionModel {
    async fn assess(&self, request: DecisionRequest) -> Result<DecisionAssessment>;
}
```

`DecisionQuestion` 首版支持 `choice`、`score` 和 `boolean_probability` 三种规范类型；
`DecisionSignal` 保存规范化值、完整概率分布、可选置信统计和问题 ID。adapter 负责与供应商协议互转、验证
选项全集、概率范围与总和、实际模型版本及 usage，不能在转换时应用业务阈值。原始供应商 envelope 可作为
有界审计证据保存，但运行时策略只消费规范化信号。

概率到执行档位的映射由独立的纯 `DecisionPolicy` 完成。它接收固定版本的信号、阈值和风险规则，输出建议
档位、需要澄清标记及逐项理由；同一输入必须得到同一结果。adapter 更换、阈值调整或问题含义变化分别升级
adapter、policy 或 question set 版本，不能共享一个含糊的“Jev 版本”。

问题必须原子、封闭且版本化。首版建议并行判断：

- `task_kind`：从宿主配置的任务类型集合选择；
- `ambiguity`：对明确、局部缺失、关键缺失等语义等级评分；
- `reasoning_need`：对直接回答、有限推理、多步推理等等级评分；
- `coordination_need`：判断是否需要多个角色或评价步骤；
- `needs_clarification`：判断继续执行是否缺少关键信息。

代码继续负责计数、算术、日期、预算、权限和候选资格。决策模型不接收价格表或供应商模型 ID，不返回自然
语言理由，也不能直接选择最终执行模型。不同问题相互独立；策略不能假设否定问题、不同原语或重复问法的
概率满足算术恒等式。

Jev adapter 使用固定模型版本，不把会移动的 `latest` 别名作为校准身份。阈值按 question set、模型版本、
任务类型和语言分别评测；供应商 `confidence` 只描述返回分布的集中程度。本项目只有在 shadow evaluation
证明目标切片上的选择性风险可接受后，才允许该概率影响路由。

同时提供确定性的 fake adapter，用于契约、账务、取消和恢复测试。第二个真实 adapter 可以是生成模型的
受限结构化适配器或未来本地模型；兼容相同接口不代表质量或概率已经等价，必须分别校准。

### planner：生成计划，不冒充决策模型

现有 planner 继续在已登记任务、共享预算和无工具权限的条件下运行。它消费规则结论和有效决策信号，负责
补充类型、现有策略、能力与验收；需要自然语言理由时由 planner 产生，不能要求不生成文本的决策模型解释。
proposal 升级为版本 2：

```rust
pub struct PlannerProposal {
    pub proposal_version: u32,
    pub task_type: TaskType,
    pub strategy: Strategy,
    pub suggested_execution_class: ExecutionClass,
    // existing capability, acceptance and reason fields remain
}
```

移除 `classification_confidence` 对决策的影响；planner 在生成文本中自报的置信度与 DecisionSignal 的模型原生
概率是不同证据，前者只作审计信息。planner 能建议更高档位，不能降低规则、宿主、决策策略或 fallback 已
确定的最低档位。

planner 提示中包含经过校验的任务事实、规则结论和已接受的决策信号；事实值仍由宿主提供，模型返回的重复
或冲突事实全部忽略。无效 JSON、越权字段、超时和调用失败沿用现有 fallback 语义。

### 未开源与托管依赖的控制方式

无法审计模型内部实现时，可复算的对象是“当时发送了什么、供应商返回了什么、代码如何合并”，不是供应商
如何产生概率。设计据此遵守以下规则：

1. **核心不依赖 Jev 可用性。** 配置支持 `disabled`、`shadow`、`active`、`required`；规则-only 路径始终可
   运行。首轮上线只使用 shadow，记录信号但不改变路由。
2. **失败策略显式配置。** `active` 模式必须选择保守 fallback 档位或终止评估；超时、限流、未知模型版本、
   非法概率和缺失问题不能静默回落到 simple，也不能被解释为低风险。
3. **供应商约束先于调用。** Jev 是外部数据接收方，必须满足 allowed providers、地域、数据与凭证约束；
   不允许向它发送的任务直接走规则与配置 fallback，不为获得判断而放宽限制。
4. **调用参与统一资源约束。** decision call 在派发前预留预算并占用 `max_calls`，沿用取消、deadline、关闭
   清理和未知费用语义。输入计费与无输出计费只是 adapter 的价格配置，不能写死进内核算法。
5. **固定并保存证据。** 保存 question set 版本、规范 state 摘要、adapter ID、请求模型、实际模型、所有概率、
   usage、阈值版本和最终覆盖理由；密钥和未脱敏原始 state 不进入日志。
6. **供应商声明不作不变量。** “已校准”“不会 hallucinate”、价格、延迟和上下文上限仅作为外部配置或评测
   假设。本地回归和线上反馈决定阈值；模型升级后旧阈值默认失效。
7. **替换按行为验收。** 新 adapter 通过同一契约不代表可互换。每个实现分别验证准确率、困难任务漏升级率、
   选择性风险、ECE/Brier、p50/p95 延迟、未知费用和服务失败路径，再决定是否进入 active。

当前产品事实、公开信息缺口与 shadow evaluation 方法见
[Jev 与“Jev-like”决策模型调研](../research/jev-like-model.md)。

### router：档位是资格条件，Q 是质量证据

每个模型候选新增宿主配置的 `execution_class`。路由先执行现有能力、上下文、模型、供应商、地域、
预算和可用性约束，再排除低于当前最低执行档位的候选，最后使用现有 task-fit 价值函数评分。

执行档位不替代质量画像。相同档位内仍按任务 Q、成本、延迟、可靠性和不确定性选择；更高档位候选
也可以参与低档位任务，但只有在价值评分支持时才会胜出。档位升级后，下一候选仍须满足
`min_upgrade_gain` 的严格 Q 改善，避免把“更贵”误当作“更好”。

RoutingSnapshot 升级到新版本并固定：

- 模型到执行档位的映射；
- TaskAssessment 及其规则、决策信号、planner、宿主来源；
- rule set、validator、question set、decision adapter、本地 decision policy、planner proposal 和路由算法版本；
- 原有质量画像、榜单参考、选择偏好和评分时间。

### execution：运行时升级控制器

现有 `engine/execution.rs` 保持策略循环所有权，新增私有 `EscalationState`，记录当前最低档位、质量下限、
已尝试候选和升级原因。它只消费已持久化的评价与资源状态，不另建后台监控任务。

质量失败和运行故障分开处理：

- 确定性检查或独立 critic 返回 `Revise`：允许质量升级；
- 模型不可用、协议错误或传输失败：在同档位先执行现有有界换候选，不证明任务更难；
- `MissingEvidence`、`Unacceptable`、`HumanRequired`：停止并转人工；
- 工具失败：保留现有“不自动重试”语义；
- 取消、截止时间、账务或存储失败：停止并完成清理，不触发升级；
- 通过：立即结束，不继续追求更高档位。

一次质量升级同时收紧两个条件：最低执行档位最多提高一级，质量下限提高到
`previous_q + min_upgrade_gain`。候选不得重复，且所有规划、生成、critic、工具和续写继续共享
`max_calls`、预算、轮数和截止时间。

## 任务事实与规则契约

内核不能看到宿主文件系统或服务拓扑，因此“文件数 > 5”“涉及 migration”“跨服务”必须由宿主通过
结构化字段提交，不能从 goal 或 evidence 文本自动提升为硬事实。

新提交 schema 3 增加：

```json
{
  "task_facts": {
    "version": "coding-facts-v1",
    "values": {
      "file_count": 7,
      "change_kinds": ["database_migration"],
      "service_count": 2
    }
  },
  "minimum_execution_class": "simple"
}
```

`TaskFactValue` 只允许有界的布尔值、非负整数、短字符串和去重字符串集合。限制事实数量、键和值长度，
拒绝嵌套对象及任意大 JSON。`version` 标识事实生产方式；同名事实跨版本不自动视为相同含义。

规则配置采用受限声明式比较，不执行表达式、脚本或正则：

```json
{
  "version": "coding-demand-v1",
  "rules": [
    {"id": "many-files", "fact": "file_count", "op": "gt", "value": 5,
     "minimum_execution_class": "medium"},
    {"id": "migration", "fact": "change_kinds", "op": "contains",
     "value": "database_migration", "minimum_execution_class": "hard"},
    {"id": "cross-service", "fact": "service_count", "op": "gt", "value": 1,
     "minimum_execution_class": "hard"}
  ]
}
```

首版操作符限定为 `eq`、`gte`、`gt`、`contains`。加载配置时检查操作符和值类型匹配、rule ID 唯一、
档位有效及规则数量上限。任务缺少某事实时规则结果为 `not_observed`，不当作 false，也不由决策模型或
planner 补值。

`RuleAssessment` 保存每条规则的 `matched`、`not_matched` 或 `not_observed`、所见值摘要及规则版本。
日志和事件只输出 rule ID 与结果；完整任务事实随计划持久化，但遵守提交上下文大小限制。

## 合并规则

最终最低执行档位取以下有效输入的最大值：

1. 宿主显式 `minimum_execution_class`；
2. 所有命中规则的最低档位；
3. 有效决策信号经版本化本地策略映射出的档位；
4. 有效 planner proposal 的建议档位；
5. decision model 或 planner 失败时各自配置的 fallback 档位。

宿主显式 `task_type`、strategy、constraints、工具声明和 acceptance 继续优先；planner 只能按现有规则补充类型、
选择现有策略、增加能力要求和加强验收。规则及 planner 都不能扩大 allowed models/providers/regions、预算、
调用次数、轮数、deadline 或工具集合。

`TaskAssessment` 结构包含最终档位、规范化决策信号、命中的规则 ID、缺失事实、decision assessment ID、
proposal ID、所有版本和合并理由。
它与 EffectivePlan、RoutingSnapshot 原子保存。任务运行期间配置、规则或历史反馈变化不改变该快照。

## 状态与失败语义

| 场景 | 行为 |
| --- | --- |
| 无 task facts、规划 disabled | 使用宿主最低档位；未观察规则不触发升级 |
| 硬规则命中 hard、决策策略或 planner 建议 simple | hard；软信号不能降低规则下限 |
| 无规则命中、决策信号表示高推理需求且达到已校准阈值 | 使用本地策略映射档位并保存概率与版本 |
| decision model 为 shadow | 保存判断与后续结果，不影响本次有效档位 |
| decision model 超时、非法或版本未知且有 fallback | 使用规则、宿主和 decision fallback 的最大档位 |
| decision model required 且无 fallback | 评估失败，已发生费用照常结算 |
| planner proposal 非法且有 fallback | 使用规则、宿主、决策策略和 planner fallback 的最大档位 |
| planner proposal 非法且 required 无 fallback | 规划失败，已发生费用照常结算 |
| 首轮产物通过 | 完成，不升级 |
| 首轮产物 Revise | 在资源允许时提高一档及 Q 下限 |
| 同档候选传输失败 | 有界换候选，不提高任务档位 |
| 没有严格 Q 改善候选 | 保留产物并返回 human_required |
| 更高档候选超预算或违反地域约束 | 不派发，返回具体停止原因 |
| critic、升级或结算期间取消 | 完成费用处理及终态写入后退出 |

## 契约与持久化版本

- 新提交写 `schema_version: 3`；schema 2 只作为当前数据库内的历史恢复证据读取，不接受为新提交。
- `DecisionRequest`、`DecisionAssessment`、`PlannerProposal`、`EffectivePlan` 和计划 checkpoint 各自版本化；
  供应商模型版本不能代替 question set 或本地合并策略版本。
- RoutingSnapshot 使用下一版本和 `hybrid-demand-v1` 算法标识；旧快照继续按其原算法复算。
- 任务事实、规则结论、决策信号、planner 建议和最终 TaskAssessment 分别保存，不能只保存最后档位。
- 本设计只增加版本化 JSON，不要求改变 SQLite 表结构；数据库 schema 仍为 2。

## 实施步骤

### 1. 纯规则模块

新增 TaskFacts、RuleSet、RuleAssessment、ExecutionClass 和 TaskAssessment 契约，实现受限操作符、输入上限和
确定性合并。先覆盖缺失事实、类型错误、多规则取最大档位、宿主下限和规则顺序不影响结果。

### 2. 配置与候选档位

为配置增加 rule set 及模型 execution class，迁移文件加载、内存配置、示例和 Python 调用方。验证未知档位、
重复规则、错误操作数和没有任何候选覆盖某档位的配置错误。

### 3. 决策协议与 fake adapter

实现供应商无关的 DecisionRequest、DecisionQuestion、DecisionSignal 和 DecisionAssessment，先用确定性 fake
adapter 验证输入上限、规范化、概率校验、预算预留、结算、取消、超时和恢复证据。领域层与 Python 公共接口
不暴露供应商 SDK 类型。

### 4. Jev shadow adapter 与离线评测

在 adapter 层实现 TypeSafe System One 协议，只允许固定模型版本。先以 shadow 模式采集本项目任务的预测与
实际结果，完成中文、长上下文、否定、多跳、对抗文本、服务失败和版本漂移切片评测；未达到漏升级率与选择
性风险门槛前不影响生产路由。

### 5. Planner v2

更新提示、proposal 解析和纯校验。规则评估与有效决策信号在 planner 调用前进入提示；验证模型不能改事实、
伪造决策概率、降低档位、扩大权限或覆盖宿主明确选择。规划仍无工具权限并共享资源上限。

### 6. 有效计划与路由快照

合并并原子保存 TaskAssessment，升级 routing snapshot，在硬约束后增加档位过滤。路由决定记录候选档位、
最低档位来源和排除原因；旧快照复算测试保持原行为。

### 7. 运行时升级

将 EscalationState 接入现有 cascade，区分质量失败和运行故障；随后把可选独立 critic 接入 cascade 的
语义质量判断。验证每次只升一档、严格 Q 改善、候选不重复及资源耗尽终止。

### 8. 跨层验证和真实评测

Rust 使用注入 adapter/store 验证规则、规划、路由、预算、取消和结算；Python 对两个协议运行 schema 3、
decision model、planner v2 及升级路径。真实评测至少比较规则-only、生成式 LLM 判断、Jev、规则 + Jev 和
完整混合方案的首轮通过率、困难任务漏升级率、总费用、延迟、升级率与人工介入率，不能只报告分类一致率。

### 9. 文档与技术图

实现时同步 CONTEXTS、README、usage、配置指南、示例和代码入口，并按 archify 工作流更新架构、评测及反馈
技术图。目标设计不改写当前实现图。

## 完成条件

1. 给定相同事实、规则、DecisionAssessment、本地策略和 proposal，可复算出完全相同的 TaskAssessment。
2. goal/evidence 中伪造“只有一个文件”等文本不能改变规则事实。
3. decision model 与 planner 都不能降低宿主或规则档位，不能扩大工具、数据、预算或候选权限。
4. 首轮路由同时满足硬约束、最低档位和 task-fit 价值排序，并保存完整解释。
5. 质量失败才提高档位；传输、协议、工具、账务和取消路径保持各自失败语义。
6. 每次升级满足严格 Q 改善，受共享预算、调用、轮数和 deadline 约束，并能有界终止。
7. decision model disabled、shadow、成功、超时、非法响应、未知版本和未知费用路径均有界终止并保留证据。
8. schema 2 历史提交仍可恢复，RoutingSnapshot 1–4 仍按旧算法复算；schema 3 新任务保存五层证据。
9. Rust、Clippy、fmt、原生扩展重建、Python、Ruff 和文档检查通过；离线模拟与真实收益评测分开报告。

## 暂缓范围

- 训练监督式或在线学习的复杂度分类器；
- 从仓库、工单或工具结果自动生成 task facts；
- 用参数量自动推断 execution class；
- 未经本项目校准就根据决策概率或生成模型自报置信度直接选择候选；
- 复刻 Jev 未公开的模型结构、训练数据或 RLCD 算法；
- 无业务验收证据的自动阈值学习；
- 跨任务自动重放或 exactly-once 工具执行。
