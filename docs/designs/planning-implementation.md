# 第一批规划实现记录

本文保留第一批实现与验证的历史记录，不代表当前接口或数据库兼容承诺。后续 C、D 和图形工作的最新进度见[当前状态](current-status.md)。入口已由[统一执行路径](execution-unification.md)收敛为 `run/stream`；当前数据库仅接受 schema 2，不自动迁移旧库，见 [README 开发期数据库规则](../../README.md#开发期数据库规则)。

基线：`142ff41`；工作分支：`feat/planning-submissions`。
范围：专项设计的 A+B。代码、示例和离线测试已实现；图形生成与验收待完成。C+D 分类型画像与指标闭环未实现。

## 已实现行为

- Rust 新增 `Engine::submit`、`submit_with_host`；Python 新增 `submit`、`stream_submission`。原有 `run/stream` 不触发规划。
- `SubmissionSpec`、`PlannerProposal`、`EffectivePlan` 和版本校验经 `contracts` 导出；提示、纯校验及规划调度为内部模块。
- disabled/auto/required 的入口策略、宿主明确选择冲突拒绝、能力并集和验收加强。建议不接受未知字段、工具权限或资源修改。
- 规划池为配置中的 `planner_models` 候选 ID 子集；沿用模型/供应商/地域/本地性/上下文硬约束，规划要求 text/json，执行工具需求不误用于规划选模。
- 规划和执行共用准入、并发名额、取消、账本、调用次数及总截止时间。规划阶段进一步收紧费用和时间，首版只允许一次尝试。
- 派发后先结算 usage 再解析规划 JSON。缺失 usage 保留预留，显式回退不释放未知费用。存储/结算失败、任务取消、总资源耗尽不回退。
- 保存原始提交、原始输出、解析建议、失败与回退依据、有效计划。有效计划和 planned 检查点在同一事务保存；规划输出不发出 content_delta。
- 新提交的 Python 取消、流提前退出及关闭沿用原有受保护的清理链路。

## 实际接口取舍

| 项目 | 第一批实现 |
| --- | --- |
| 规划次数 | `planning.max_calls` 缺省 1，首版仅接受 1；不提供未实现的重试配置 |
| 规划资源字段 | `max_cost` 与 `timeout_ms` 显式提供；不能增加任务预算或截止时间 |
| 规划失败回退 | 仅宿主显式 fallback；与宿主明确类型/策略冲突时拒绝 |
| 不同评审模型 | 沿用 `different_critic`，保证不同候选 ID，不推断底层模型独立性 |
| 类型画像 | 保存角色映射，code_generation 的 critic 映射到 code_review；评分仍为全局先验，版本为 legacy-global-v1 |
| 验收器版本 | 规划建议必须使用宿主原验收版本；不允许模型引入新检查器 |
| 数据库 | 表结构仍为 schema 1，新增 payload_version=1 envelope；旧 JSON 可读取 |
| 恢复记录 | 保留 task 字段并增加 submission 和 plan；未规划提交的 task 是内部投影，不是已选执行策略 |
| 自定义 Store | 新增默认报不支持的 create_submission/save_plan；旧 run 不受影响，submit 需存储明确实现事务能力 |
| 返回结果 | 沿用 TaskResult；计划摘要通过事件提供，完整计划和建议保存在账本 |

新版本可以打开旧数据库；旧程序无法解码新提交 envelope，不承诺向旧版降级读取。恢复仍为只读检查，不自动重放。

## 模块结构与检查

规划契约放入 `src/contracts/planning.rs`，避免继续扩展主契约文件。提示/校验、引擎调度/单次尝试、存储事务分别位于所属模块，单向依赖；生产代码不依赖测试或示例。

`src/store.rs` 仍超过 400 行评估线但低于 600 行拆分线，新规划写事务已分离。新增 Rust 测试独立于原有较长的 `tests/engine.rs`，通过 support 与 store_fault 夹具组织，主规划测试低于 600 行。

`run_input` 保持活动注册、并发 permit、实际执行和终态写入在同一词法生命周期内，避免拆分时提前释放清理保护。`planning_attempt` 保留候选过滤、事务预留、受保护派发及解析的连续顺序，外层错误直接终止回退；这两处虽超过 100 行，保留完整边界，并以取消、关闭、费用超额和存储故障测试验证。其余新增生产文件均低于 400 行，没有新增依赖。

## 验证结果

环境：Linux，Rust 1.98.1，Python 3.12.14。沿用准备环境时确认的 `CARGO_INCREMENTAL=0`，避开本环境已观察到的增量构建链接故障。Rust 检查与 maturin 按顺序执行。

| 检查 | 结果 |
| --- | --- |
| `cargo test --all-features --locked` | 34 项通过：原有 20 项 + 规划 14 项 |
| `cargo clippy --all-features --all-targets --locked -- -D warnings` | 通过 |
| `cargo fmt --check` | 通过 |
| `python -m maturin develop --locked` | 原生扩展构建并安装成功 |
| `python -m pytest -q` | 43 项通过：原有 17 项 + 两端点规划 26 项 |

覆盖的关键结果：规划成功占一次共享调用；非法 JSON/越权字段先结算后拒绝；无 usage 保留预留；无候选和阶段超时可显式回退；总截止时间、调用额度、用量超额及结算存储故障不产生后续请求；模型数据限制和 critic 身份隔离生效；两个 HTTP/SSE 端点没有规划候选增量；取消、提前退出和关闭等待清理；旧 schema 1 SQL 夹具及新提交跨关闭重开的恢复记录可读取。原有工具批次与回调测试均通过。

没有真实供应商联调，也没有评估规划能否提高业务任务通过率。当前模拟测试证明协议、资源和生命周期行为，不能替代设计中后续的同任务集收益比较。

## 文档与图形状态

README、CONTEXTS、AGENTS、代码参考、提交 JSON 与 Python 示例已同步。架构 JSON 的职责标签与源码映射已更新，关闭状态关系未改变。

仓库要求使用 archify 生成并验收技术图；第一批实现时缺少该技能或生成器，因此尚未重新生成 HTML，也未执行新图的结构、浏览器和视觉验收。随后已在当前环境下载 archify 2.16 并通过自检，但个人技能持久保存未成功；图形生成与验收仍待完成。索引与 review.json 已标明待完成，旧通过回执仅对应原始图产物；没有手改 HTML 或把旧回执当成本次通过结果。

## 后续批次

C：版本化的任务类型/角色画像、父类型回退、同口径 cascade 质量门槛及固定快照。

D：独立业务验收归因、端点 attempt 指标、跨任务持久化反馈与任务集回放比较。外部榜单仍是后续经过校准的先验来源，不在本批运行时调用。
