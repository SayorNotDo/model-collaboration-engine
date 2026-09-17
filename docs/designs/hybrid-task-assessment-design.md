# 混合任务评估与运行时升级设计

状态：目标设计，作为下一项优先实现；尚未修改运行时代码。

## 目标

以“确定性规则 + 受限 LLM 判断 + 有界运行时升级”形成首轮模型选择和失败兜底闭环。
规则处理宿主可验证的硬信号，LLM 判断需求模糊度、推理强度和协作需求，运行时依据实际产物评价决定
是否升级。三者都不直接指定供应商模型，也不能放宽宿主权限、预算、地域或数据约束。

这不是训练复杂 ML 分类器，也不是要求一个模型回答笼统的“任务难不难”。目标是把不同来源的证据分层，
保存可复算的合并结果，并复用现有任务适配路由和 cascade 执行循环。

## 领域区分

| 概念 | 表达什么 | 不表达什么 |
| --- | --- | --- |
| 任务事实 | 宿主声明的结构化、可验证事实 | evidence 文本中的自我声明 |
| 规则结论 | 某条版本化规则对最低执行档位的要求 | 最终模型身份或质量概率 |
| LLM 判断 | 模糊度、推理强度、协作需求及理由 | 工具授权、硬约束或可靠置信概率 |
| 任务需求评估 | 规则、宿主选择和有效 LLM 建议的固定合并结果 | 任务客观难度或模型必然成功率 |
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
   └─ optional bounded LLM assessment ────────────┤
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
    proposal: Option<&PlannerProposal>,
) -> Result<TaskAssessment>;
```

该模块不访问模型、数据库、文件系统或当前时间。调用方只需理解输入事实和最终 `TaskAssessment`；
规则匹配、优先级、默认值和冲突处理隐藏在模块内部。现有 `planning::validate_plan` 调用合并接口，
不复制规则逻辑。

### planner：只判断软信号

现有 planner 继续在已登记任务、共享预算和无工具权限的条件下运行。proposal 升级为版本 2，新增：

```rust
pub enum SignalLevel {
    Low,
    Medium,
    High,
}

pub struct PlannerProposal {
    pub proposal_version: u32,
    pub task_type: TaskType,
    pub strategy: Strategy,
    pub ambiguity: SignalLevel,
    pub reasoning: SignalLevel,
    pub coordination: SignalLevel,
    pub suggested_execution_class: ExecutionClass,
    // existing capability, acceptance and reason fields remain
}
```

移除 `classification_confidence` 对决策的影响；可以为审计保留原始值，但模型自报置信度不参与质量概率、
档位降低或升级门槛。planner 能建议更高档位，不能降低规则、宿主或 fallback 已确定的最低档位。

planner 提示中包含经过校验的任务事实和规则结论，让模型解释剩余软信号；事实值仍由宿主提供，
模型返回的重复或冲突事实全部忽略。无效 JSON、越权字段、超时和调用失败沿用现有 fallback 语义。

### router：档位是资格条件，Q 是质量证据

每个模型候选新增宿主配置的 `execution_class`。路由先执行现有能力、上下文、模型、供应商、地域、
预算和可用性约束，再排除低于当前最低执行档位的候选，最后使用现有 task-fit 价值函数评分。

执行档位不替代质量画像。相同档位内仍按任务 Q、成本、延迟、可靠性和不确定性选择；更高档位候选
也可以参与低档位任务，但只有在价值评分支持时才会胜出。档位升级后，下一候选仍须满足
`min_upgrade_gain` 的严格 Q 改善，避免把“更贵”误当作“更好”。

RoutingSnapshot 升级到新版本并固定：

- 模型到执行档位的映射；
- TaskAssessment 及其规则、planner、宿主来源；
- rule set、validator、planner proposal 和路由算法版本；
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
档位有效及规则数量上限。任务缺少某事实时规则结果为 `not_observed`，不当作 false，也不由 LLM 补值。

`RuleAssessment` 保存每条规则的 `matched`、`not_matched` 或 `not_observed`、所见值摘要及规则版本。
日志和事件只输出 rule ID 与结果；完整任务事实随计划持久化，但遵守提交上下文大小限制。

## 合并规则

最终最低执行档位取以下有效输入的最大值：

1. 宿主显式 `minimum_execution_class`；
2. 所有命中规则的最低档位；
3. 有效 planner proposal 的建议档位；
4. planner 失败时配置 fallback 的档位。

宿主显式 `task_type`、strategy、constraints、工具声明和 acceptance 继续优先；planner 只能按现有规则补充类型、
选择现有策略、增加能力要求和加强验收。规则及 planner 都不能扩大 allowed models/providers/regions、预算、
调用次数、轮数、deadline 或工具集合。

`TaskAssessment` 结构包含最终档位、三个软信号、命中的规则 ID、缺失事实、proposal ID、所有版本和合并理由。
它与 EffectivePlan、RoutingSnapshot 原子保存。任务运行期间配置、规则或历史反馈变化不改变该快照。

## 状态与失败语义

| 场景 | 行为 |
| --- | --- |
| 无 task facts、规划 disabled | 使用宿主最低档位；未观察规则不触发升级 |
| 硬规则命中 hard、LLM 建议 simple | hard；LLM 不能降低规则下限 |
| 无规则命中、LLM 判断推理 high | 使用 LLM 建议档位并保存软信号来源 |
| LLM proposal 非法且有 fallback | 使用规则、宿主和 fallback 的最大档位 |
| LLM proposal 非法且 required 无 fallback | 规划失败，已发生费用照常结算 |
| 首轮产物通过 | 完成，不升级 |
| 首轮产物 Revise | 在资源允许时提高一档及 Q 下限 |
| 同档候选传输失败 | 有界换候选，不提高任务档位 |
| 没有严格 Q 改善候选 | 保留产物并返回 human_required |
| 更高档候选超预算或违反地域约束 | 不派发，返回具体停止原因 |
| critic、升级或结算期间取消 | 完成费用处理及终态写入后退出 |

## 契约与持久化版本

- 新提交写 `schema_version: 3`；schema 2 只作为当前数据库内的历史恢复证据读取，不接受为新提交。
- `PlannerProposal.proposal_version`、`EffectivePlan.plan_version` 和计划 checkpoint 升级到 2。
- RoutingSnapshot 使用下一版本和 `hybrid-demand-v1` 算法标识；旧快照继续按其原算法复算。
- 任务事实、规则结论、planner 建议和最终 TaskAssessment 分别保存，不能只保存最后档位。
- 本设计只增加版本化 JSON，不要求改变 SQLite 表结构；数据库 schema 仍为 2。

## 实施步骤

### 1. 纯规则模块

新增 TaskFacts、RuleSet、RuleAssessment、ExecutionClass 和 TaskAssessment 契约，实现受限操作符、输入上限和
确定性合并。先覆盖缺失事实、类型错误、多规则取最大档位、宿主下限和规则顺序不影响结果。

### 2. 配置与候选档位

为配置增加 rule set 及模型 execution class，迁移文件加载、内存配置、示例和 Python 调用方。验证未知档位、
重复规则、错误操作数和没有任何候选覆盖某档位的配置错误。

### 3. LLM planner v2

更新提示、proposal 解析和纯校验。规则评估在 planner 调用前完成并进入提示；验证模型不能改事实、降低档位、
扩大权限或覆盖宿主明确选择。规划仍无工具权限并共享资源上限。

### 4. 有效计划与路由快照

合并并原子保存 TaskAssessment，升级 routing snapshot，在硬约束后增加档位过滤。路由决定记录候选档位、
最低档位来源和排除原因；旧快照复算测试保持原行为。

### 5. 运行时升级

将 EscalationState 接入现有 cascade，区分质量失败和运行故障；随后把可选独立 critic 接入 cascade 的
语义质量判断。验证每次只升一档、严格 Q 改善、候选不重复及资源耗尽终止。

### 6. 跨层验证和真实评测

Rust 使用注入 adapter/store 验证规则、规划、路由、预算、取消和结算；Python 对两个协议运行 schema 3、
planner v2 及升级路径。真实评测至少比较规则-only、LLM-only 和混合方案的首轮通过率、总费用、延迟、
升级率与人工介入率，不能只报告分类一致率。

### 7. 文档与技术图

实现时同步 CONTEXTS、README、usage、配置指南、示例和代码入口，并按 archify 工作流更新架构、评测及反馈
技术图。目标设计不改写当前实现图。

## 完成条件

1. 给定相同事实、规则版本和 proposal，可复算出完全相同的 TaskAssessment。
2. goal/evidence 中伪造“只有一个文件”等文本不能改变规则事实。
3. planner 不能降低宿主或规则档位，不能扩大工具、数据、预算或候选权限。
4. 首轮路由同时满足硬约束、最低档位和 task-fit 价值排序，并保存完整解释。
5. 质量失败才提高档位；传输、协议、工具、账务和取消路径保持各自失败语义。
6. 每次升级满足严格 Q 改善，受共享预算、调用、轮数和 deadline 约束，并能有界终止。
7. schema 2 历史提交仍可恢复，RoutingSnapshot 1–4 仍按旧算法复算；schema 3 新任务保存四层证据。
8. Rust、Clippy、fmt、原生扩展重建、Python、Ruff 和文档检查通过；离线模拟与真实收益评测分开报告。

## 暂缓范围

- 训练监督式或在线学习的复杂度分类器；
- 从仓库、工单或工具结果自动生成 task facts；
- 用参数量自动推断 execution class；
- 根据模型自报置信度直接选择候选；
- 无业务验收证据的自动阈值学习；
- 跨任务自动重放或 exactly-once 工具执行。
