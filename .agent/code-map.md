# 代码与验收参考

工作流需要定位实现链路或选择回归场景时查阅本文件。任务到工作流的选择规则见 [task-router.md](task-router.md)。

## 按行为选择入口

下列代码路径相对于仓库根目录；“关联检查与验收”用于选择本次受影响的场景，不要求无关任务扩大全部覆盖。

| 任务涉及的行为 | 优先入口 | 关联检查与验收 |
| --- | --- | --- |
| 配置、任务、错误或结果字段 | `src/contracts.rs` | 对照 JSON 序列化、Python 字典接口及 `examples/`；验证合法输入、边界值和不合法输入，更新用户示例。 |
| 模型选择、硬约束或评分 | `src/router.rs` | 对照 `src/engine.rs` 的调用上下文与预算；检查排除原因、无可用模型、能力与上下文约束，以及级联升级。 |
| 单模型、级联或生成—评审策略 | `src/strategy.rs`、`src/engine.rs` | 验证验收通过、失败、轮数与调用次数耗尽；区分策略节点、一次模型调用和工具续写，保持评审工具边界。 |
| 并发、排队、取消、超时或关闭 | `src/engine.rs` | 追踪 `src/bindings.rs`、`python/model_collaboration_engine/__init__.py` 与 `_run.py`；检查排队、模型调用或工具回调的取消，流提前退出、重复取消，以及关闭宽限期、清理超时保留数据库所有权和关闭重试。 |
| 工具声明、授权边界、调用或结果回传 | `src/tools.rs`、`src/engine.rs` | 契约在 `src/contracts.rs`，桥接在 `src/bindings.rs` 和 `_run.py`；检查缺少宿主、未声明工具、重复调用 ID、回调异常、未知费用、额度不足及结果送回模型。 |
| 流式事件、背压或事件回调 | `src/events.rs`、`src/engine.rs`、`src/adapter.rs` | 追踪绑定和 `_run.py` 的接收端；验证内容提前到达、递增序号、队列满、订阅关闭、回调停滞与提前退出，分别断言事件和最终结果。 |
| Chat Completions / Responses 协议 | `src/adapter.rs` | 对照 `src/contracts.rs` 的端点与能力声明；用本地 HTTP 服务验证请求结构、SSE 内容与工具结果、usage、截断及错误响应。涉及公共行为时覆盖两个端点。 |
| 预算、结算、数据库、对账或恢复 | `src/store.rs`、`src/engine.rs` | 核对事务、任务唯一性和 schema 版本；检查预留、已知/未知费用、超额记账、失败与取消终态。新增恢复时先明确重复副作用与重复计费处理，再实现重放。 |
| Python 公共接口、异步生命周期或扩展桥接 | `python/model_collaboration_engine/__init__.py`、`python/model_collaboration_engine/_run.py`、`src/bindings.rs` | 验证宿主事件循环、回调结果与异常、取消传播及资源释放；重建扩展后运行 Python 测试。 |
| 依赖、特性或打包安装 | `Cargo.toml`、`Cargo.lock`、`pyproject.toml` | 检查 Rust 特性和 Python 模块路径的对应关系，顺序执行 Rust 验证、maturin 构建和 Python 测试，更新受影响的安装说明。 |
| 仓库指南、任务路由或使用说明 | `AGENTS.md`、`.agent/`、`README.md` | 以源码核对事实与边界，保持规则、任务入口和验证命令各自的唯一来源；检查相对链接、新增文件和空白。 |

## 内部模块定位

- `src/contracts/planning.rs`：SubmissionSpec、规划模式、建议与有效计划；经 `contracts` 稳定导出。
- `src/contracts/profiles.rs`：可选静态画像、父类型/角色映射和权重契约及纯配置校验；通过 `contracts` 导出。
- `src/router/profiles.rs`：画像回退、角色映射及固定 RoutingSnapshot；不持有网络或数据库。`router::route_profiled` 用快照与当次请求复算，`route` 仅用于类型确定前的规划候选选择，执行统一使用 `route_profiled`。
- `src/planning.rs`、`src/planning/validation.rs`：内部提示构造及纯合并/越权校验，不持有网络和数据库。
- `src/engine/planning.rs`、`src/engine/planning/attempt.rs`：受准入保护的规划、回退与阶段资源边界；复用 dispatch 的结算屏障。
- `src/store/planning.rs`：payload_version 1 envelope 入库，有效计划与检查点原子保存；`store/recovery.rs` 同时解码旧任务和新提交。

- `src/engine.rs`：引擎公共入口、准入、终态写入、事件与关闭生命周期。
- `src/engine/execution.rs`：策略轮次、评审及产物结果。
- `src/engine/invocation.rs`：模型选择、调用预留、失败换模与工具续写调度；`src/engine/invocation/dispatch.rs` 将模型派发与结算保持在同一步骤，结算失败直接终止，不进入换模重试。
- `src/engine/tools.rs`：整批工具校验、逐次工具预留与执行、工具结果回填。
- `src/store/recovery.rs`：同一读事务内组合任务、账本和调用证据；`src/store.rs` 保留数据库所有权及写入事务。
- `python/model_collaboration_engine/_types.py`：宿主工具和事件回调的共享类型约定，不依赖包入口。

这些子模块保持内部可见性，对外仍通过原有 Engine 与 Store 模块调用。`store.rs` 超过 400 行的评估线，但未达 600 行拆分线；本次已提取恢复查询，其余写事务保留原有完整边界。`tests/engine.rs` 的场景分组按功能定位，目前未达到 800 行拆分要求；本轮沿用这些公共接口回归场景验证重构。

- `src/contracts/feedback.rs`：分层评价、宿主反馈及指标契约。
- `src/store/schema.rs`：当前 schema 一次性建库、版本拒绝及备份重建提示，不提供迁移或清库。
- `src/store/feedback.rs`：候选证据及幂等反馈归因。
- `src/store/metrics.rs`：一致性读事务汇总，不推断业务验收；路由仅消费所读快照。
- `examples/compare_metrics.py`：导出指标的离线分组比较，不执行回放或外部调用。

## 测试定位

- `tests/feedback.rs`：反馈幂等、归因、快照固定、版本隔离及调用指标。
- `tests/python/test_feedback.py`、`test_metrics_summary.py`：两个端点的反馈 API 与离线汇总口径。


- `tests/routing_profiles.rs`、`tests/routing/support.rs`：类型排序、画像版本隔离、回退、权重、级联同口径、存储复算及取消；复用 planning 的注入适配器夹具。
- `tests/python/test_routing_profiles.py`：两个 HTTP/SSE 端点的画像提交/流、缺省 general 快照与配置拒绝。

- `tests/database_schema.rs`：新库初始化及重开、旧版/未知版/外部数据库拒绝且原文件不变；使用旧 schema 1 夹具验证拒绝，非迁移承诺。
- `tests/planning.rs`、`tests/planning/`：规划模式、权限、资源、回退、故障注入及当前结构中的恢复证据；新规划场景不继续扩充原 `tests/engine.rs`。
- `tests/python/test_planning.py`：两个端点真实 HTTP/SSE、本地规划服务、原生入口、增量隔离及取消/关闭清理。

- `tests/engine.rs`：通过注入适配器与宿主工具验证执行结果、调用次数、事件和 SQLite 账本。
- `tests/python/test_engine.py`：原生扩展的基础调用与 Python 取消链路。
- `tests/python/test_tools_stream.py`：两个端点的工具回传、流式消费、宿主循环、异常及清理。

行为断言以结果、外部调用和持久化状态为准。既有离线测试覆盖不代表真实供应商已联调；进度和能力状态引用根目录 README，不在本文件保存测试数量或完成百分比。
