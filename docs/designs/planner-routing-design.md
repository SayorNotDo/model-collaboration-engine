# 模型规划与分类型路由专项设计

日期：2026-09-15　版本：0.1

状态：目标架构方向已在会话确认；本文将其具体化为待实现设计。接口名称、默认值及拆分方式属于本设计建议，实现尚未落地。

基线：SayorNotDo/model-collaboration-engine，提交 f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d。

## 1. 背景与目标

当前内核由 Python 宿主提交完整任务，Rust 执行 single、cascade、generator_critic 策略。Router 使用硬约束和全局模型先验评分；SQLite 记录调用预留、结算与任务终态。

原文档已有策略与路由分离、宿主权限、预算、取消和恢复检查的设计。持久化路由指标被列为未实现边界。当前分支未见主模型规划、分任务类型画像或外部评分融合的完整规格。因此本文是增量设计，不代表这些能力原本已实现。

目标：对信息不足的任务调用规划模型，确定任务类型和现有协作策略；具体选模与执行仍由确定性内核约束。后续用分类型质量证据改善选择。

首期不包含任意执行图、子任务并行、自动重规划、自动恢复、外部榜单采集和学习型路由。

## 2. 职责与依赖

| 组件 | 职责 | 边界 |
| --- | --- | --- |
| Host | 提交目标、硬约束、工具与业务验收 | 工具权限和外部副作用由宿主决定 |
| Planner | 识别类型，提出策略、能力需求和验收建议 | 无工具执行权，不能修改硬约束 |
| Plan Validator | 将建议与宿主要求合成为有效计划 | 无效建议不进入执行 |
| Router | 过滤候选、按画像和端点指标排序 | 不自行改变任务目标与权限 |
| Engine | 准入、调用、取消、结算和终态 | 所有规划及执行调用共用资源边界 |
| Evaluator | 检查业务产物 | 模型自报通过不构成独立正确性证据 |
| Store | 保存提交、建议、有效计划和调用证据 | 事务语义保持集中 |

建议 planner.rs 保存规划输入输出与提示构造，planning/validation.rs 保存纯校验规则，engine/planning.rs 负责调用调度。Planner 不直接写数据库、不拥有独立 HTTP 客户端。Engine 持有执行状态，Store 持有持久化事务；Router 不依赖 Python 类型。首版内部模块不增加公共导出，公共入口通过契约与 Engine 提供。

复用 ModelAdapter 和调用结算路径，但不要直接把 planner 当作 generator 塞入现有 conversation()：当前非 critic 分支会携带任务工具，规划必须显式禁止工具，并使用独立 JSON 契约。

## 3. 提交与计划契约

保留现有 TaskSpec 和 run/stream 行为。建议新增 submit/stream_submission 入口接收 SubmissionSpec，避免让旧 TaskSpec 的必填字段突然变为可选。

SubmissionSpec 包含：schema_version、task_id、goal、evidence、宿主 acceptance、constraints、tools、全部现有资源限制、可选 task_type 和 strategy，以及 planning 配置。

| planning 字段 | 建议语义 |
| --- | --- |
| mode | disabled、auto、required |
| fallback | 可选宿主显式提供的 task_type 与 strategy；缺省不回退 |
| max_calls | 本任务规划调用上限，建议首版默认 1 |
| max_cost | 规划阶段累计费用占用上限，不新增任务预算 |
| timeout_ms | 规划执行时间上限，不能超过任务剩余执行时间 |

disabled 要求显式策略；缺省类型为 general。auto 在类型与策略都明确时跳过，否则规划。required 总是规划，但宿主已明确的策略及类型不能被覆盖。分类置信度属于模型自报元数据，不直接用作路由质量概率。

PlannerProposal 包含 proposal_version、task_type、classification_confidence、strategy、required_capabilities、suggested_acceptance、reason。模型输出不能携带新的工具定义、预算、截止时间或供应商许可；未知字段拒绝。

EffectivePlan 包含 plan_version、task_type、strategy、role_profiles、有效约束、有效验收、submission_hash、proposal_id（可空）、validator_version 和 routing_profile_version。原始输入、模型建议与有效计划分别保留。

规划只能选现有三种策略。required_capabilities 仅允许受支持能力，并只能增加需求。验收建议只允许引用宿主注册的检查器和受支持参数；首版沿用 nonempty、required_substrings、json_object 等现有检查，可加强但不能撤销原要求。自然语言建议仅作为说明，不转换为可执行代码或未经验证的业务通过标准。

计划选择不同 critic 时，必须保留不同候选 ID 约束；不同 ID 不保证供应商底层模型不同，此边界需对外说明。

## 4. 生命周期与预算

1. 校验提交格式、工具注册及宿主边界；登记活动任务并取得并发名额。
2. 创建任务和账本，保存不可变提交，检查点标记 admitted。
3. 无需规划时编译直接计划；需要规划时保存 planning 阶段并执行规划调用。
4. 通过专用候选池和确定性规则选规划模型，先预留费用并计数，再调用和结算。
5. 保存建议证据，校验并原子保存有效计划及 planned 检查点。
6. 转入 executing，复用现有策略循环；最终写入 completed、human_required、failed 或 cancelled。

阶段是检查点信息，不替代现有任务终态。所有模型尝试、规划尝试、工具和续写共享 max_calls、budget、deadline_ms。规划阶段上限是额外收紧；有效上限取阶段剩余与任务剩余的较小值。未知规划费用继续占用预留，回退也不能释放它。没有执行余量时终止，不透支预算。

规划池是宿主配置的候选 ID 子集，候选必须满足任务数据地域、本地性、供应商许可和上下文要求，并支持 text、json。不可为规划绕过数据限制。规划阶段不按尚未识别的任务类型递归选择规划器。首批可沿用现有评分，首版不自动换模重试；未来放开尝试数也必须受共享计数约束。

解析发生在费用结算之后，非法 JSON 仍需记账。抽取公共 dispatch/accounting 辅助逻辑时必须保留结算错误终止换模的边界。

## 5. 失败、回退与关闭

| 失败 | 行为 |
| --- | --- |
| 无合法规划候选 | 无外部调用；可使用显式 fallback |
| JSON 或建议校验失败 | 保存失败依据；仅显式 fallback 可继续 |
| 供应商错误、规划超时 | 保留未知费用；有 fallback 且任务资源仍足够时继续 |
| 任务取消、总截止时间、总预算耗尽 | 终止，不回退 |
| 数据库或结算错误 | 终止，不继续外部调用 |
| 有效计划无执行候选 | 返回 routing 错误，首版不自动重新规划 |

fallback 与普通计划经过同一校验。规划失败与回退原因均可观察；原始无效输出受字节限制保存，不作为可执行指令。错误类型建议 planning、plan_validation，沿用 storage、budget、deadline、cancelled 等基础错误。

规划属于活动任务：close 停止准入，按现有宽限期等待，再取消并等待清理；超时保留数据库所有权。Python 提前退出流或取消任务时继续等待原生结算和宿主清理。不能单独创建未登记的规划后台任务。

## 6. 分类型路由：第二批

初始类型为 general、code_generation、code_review、information_extraction、reasoning、writing、tool_execution。类型描述目标，text/json/tools 描述调用能力，两者正交。父类型表及角色映射由配置版本化管理，禁止继承循环。

画像键为模型配置身份、模型版本、任务类型、角色和验收器版本。端点可靠性、价格、延迟独立维护。代码生成的 critic 可映射到 code_review/critic；映射由有效计划中的确定性规则产生。

画像回退顺序：精确类型及角色、配置的父类型及角色、general 及角色，最后使用旧全局先验。缺失画像属于未知，不当作质量为零；记录使用的回退层级。

质量估计可采用 Q=(k*p+s)/(k+n)，其中 p 是校准后的先验、k 是先验权重、s/n 是同一评价定义下通过数与有效样本数。旧全局先验可作为冷启动后备。跨任务统计需要持久化，不能沿用每任务临时 Health 充当长期表现。

生成结果的确定性检查、critic 结论和最终业务验收分别记录。critic 的正确性需依赖独立测试或人工反馈，不能用其输出 pass 的次数评分。失败与延迟按每个 attempt 统计；业务质量只归因于明确的候选产物，不能把最终成功平均归功于所有工具续写调用。

评分继续结合质量、偏好能力、可靠性、费用、延迟和不确定性，但权重可按任务类型选择。cascade 的质量门槛改用相同画像口径，并固定本任务画像快照以便复现。

外部评分属于后续先验来源：保留出处、时间、模型版本和测试类别，经业务测试校准后使用；未校准榜单成绩不解释为验收通过率。运行路由只读取本地版本化快照。

## 7. 持久化与事件

建议提交记录增加版本化 JSON envelope，保留 legacy TaskSpec 解码路径。当前 tasks.spec 恢复为 TaskSpec 的假设必须调整；仅向 JSON 写入新字段并不能保证 recovery_records 兼容。

首批优先复用 tasks.plan/checkpoint 与 attempts.metadata/outcome，但需增加 Store 的保存有效计划事务接口，原子更新 plan 与 checkpoint 并记录审计事件。原始提交不覆盖。若不改变表结构，可保持 SQLite schema 版本并新增 payload_version；必须以旧数据库夹具验证。若采用新列或指标表，则显式升 schema 版本并编写迁移，不假定现有 migrate 已提供转换。

新增 planning_started、planning_completed、planning_failed、plan_validated、planning_fallback 事件；带 task_id、attempt_id、计划版本与原因。模型事件 node_id 使用 planner，与任务共享递增序号。敏感证据不复制进简要事件。

规划输出不作为用户候选产物增量显示，避免 UI 把计划 JSON 当最终答案。规划响应在适配层有界累积，生命周期事件仍走有界通道。流结束不表示成功，最终结果和账本保持权威。

恢复查询返回规划阶段与调用证据，不自动恢复或重放。未完成规划和已经验证的计划必须可以区分。

## 8. 代码和文档实施范围

| 路径 | 目标变更 |
| --- | --- |
| src/contracts.rs | 提交、建议、有效计划、画像和版本契约 |
| src/engine.rs | 创建账本后按需规划，再进入策略 |
| src/engine/planning.rs（建议新增） | 规划调度、阶段资源限制、回退 |
| src/planner.rs、src/planning/validation.rs（建议新增） | 提示构造、建议解析、纯规则校验；实际目录按实现时结构规范统一 |
| src/engine/invocation/dispatch.rs | 复用受保护派发结算边界 |
| src/router.rs | 规划候选池，后续分类型画像 |
| src/store.rs、src/store/recovery.rs | 保存有效计划与兼容恢复查询 |
| Python 包与 src/bindings.rs | 新入口、规划事件与取消传播 |
| src/strategy.rs、src/engine/execution.rs | 接收有效计划，维持现有三策略 |

CONTEXTS.md 增补领域定义；AGENTS.md 增补规划不变量；README.md 与 examples 增补可运行用法、费用和失败语义；code-map 更新入口。现有架构图继续标注当前实现，后续按仓库 archify 工作流新增或更新目标设计图及源码证据。本文件不改现有图，不声称图形验收已完成。

## 9. 交付分批与验收

A：规划契约、纯校验、显式兼容入口。验证 disabled/auto/required 分支，非法字段和越权建议拒绝。

B：规划生命周期、预算、事件与恢复证据。验证规划成功消耗一次调用；无 usage 保留预留；解析失败仍结算；取消及关闭等待清理；结算失败不回退；规划后额度不足无后续请求；已有工具批次语义不变。

C：分类型画像与路由。用固定候选验证同一模型在不同任务类型下排序不同；角色映射、缺失回退、相同画像口径的级联门槛和快照复现。

D：验收及指标闭环。验证 critic 否决不计为最终成功，调用失败统计完整，不同模型/验收版本不混算；历史任务回放比较通过率、成本、延迟及人工介入率。

A+B 为第一批，C+D 为第二批。规划价值验证使用同一任务集对比显式执行与按需规划，单独报告新增规划开销。通过标准在实施前结合实际任务集设定，不预先承诺收益。

兼容门槛：旧 Python 调用不新增规划请求；旧配置可加载；旧 SQLite 文件和 recovery_records 可读取；Rust 检查与 Python 扩展重建按仓库验证流程顺序执行。测试必须覆盖两个端点的规划 JSON、异常和 usage 语义。离线模拟通过不能替代真实供应商联调。

## 10. 依据与本次验证

以下链接固定到分析基线。模块路径已与该版本文件树核对，目标接口与新增路径均为设计建议；本次未修改代码、未执行运行测试、未生成架构图。

- [原有领域定义](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/CONTEXTS.md)
- [现有执行与边界](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/README.md)
- [执行入口](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/src/engine.rs)
- [路由实现](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/src/router.rs)
- [存储事务](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/src/store.rs)
- [当前架构图说明](https://github.com/SayorNotDo/model-collaboration-engine/blob/f94a9bcd4966673c9e1072cdb4d42cc32c7d8b5d/docs/diagrams/README.md)
