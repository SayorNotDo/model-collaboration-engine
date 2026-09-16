# 任务适配路由第一阶段实施记录

核对日期：2026-09-17。对应[目标设计](task-fit-routing.md)和[算法契约](task-fit-routing-contract.md)。本记录描述当前工作区实现；尚未提交的工作区内容不代表已合并版本。

## 已实现行为

- 提交契约升级为 schema 2，每个新任务必须提供 `selection`。规划模型不能修改宿主的质量目标、成本尺度或升级门槛；原始提交和有效计划分别保留。
- 新任务保存 routing snapshot schema 4 和 `task-fit-v1` 算法版本。质量画像、选择偏好、榜单证据和算法版本在任务内固定；schema 1–3 继续按原算法复算。
- 路由先执行能力、上下文、模型、供应商、地域、预算和质量下限约束，再计算任务价值。成本和延迟使用宿主提供的固定参考尺度，不以预算或剩余截止时间归一化。
- 外部榜单只在任务价值主评分完全相同时参与排序，不能推翻质量与成本权衡，也不能改变质量 Q 或升级门槛。
- `cascade` 仅在确定性检查返回 Revise 后继续。下一候选必须满足 `Q_next >= Q_previous + min_upgrade_gain`；没有改善候选、候选超预算或违反约束时保留最新产物，记录原因并返回 `human_required`。

该实现优化的是当前单次调用的预期价值。语义 critic 自动触发 cascade、缺陷级模型适配和整个协作路径的预期总成本仍未实现。

## 关键实现位置

- 契约与校验：[`src/contracts/selection.rs`](../../src/contracts/selection.rs)、[`src/contracts/planning.rs`](../../src/contracts/planning.rs)
- 固定快照与评分：[`src/router/profiles.rs`](../../src/router/profiles.rs)、[`src/router/selection.rs`](../../src/router/selection.rs)、[`src/router.rs`](../../src/router.rs)
- 动态升级与终止证据：[`src/engine/execution.rs`](../../src/engine/execution.rs)
- 行为测试：[`tests/task_fit_routing.rs`](../../tests/task_fit_routing.rs)、[`tests/engine.rs`](../../tests/engine.rs)、[`tests/python/test_task_fit_routing.py`](../../tests/python/test_task_fit_routing.py)

## 验证结果

2026-09-17 在 Windows 本地执行：

| 检查 | 结果 |
| --- | --- |
| `cargo test --all-features` | 93 项通过 |
| `cargo clippy --all-features --all-targets -- -D warnings` | 通过，零警告 |
| `cargo fmt --check` | 通过 |
| `maturin develop` | 原生扩展重建成功 |
| `python -m pytest -q` | 187 项通过 |
| `python -m ruff check .` | 通过 |
| 架构图与反馈图 | 各 9/9 结构检查、浏览器检查通过，明暗截图视觉审查通过 |

测试使用离线适配器和本地 HTTP/SSE 模拟服务，没有产生真实供应商请求或费用。合成评分用例验证算法边界，不证明自然任务上的质量收益；真实收益需要按独立业务验收数据评测。

## 兼容与持久化边界

SQLite 仍使用 schema 2，没有自动迁移或清库。旧持久化提交和计划可以读取以保留证据，但新执行入口只接受提交 schema 2 及完整 `selection`。恢复查询为缺少 selection 的当前 schema 历史载荷生成明确标记的只读任务投影，不改写原始证据，也不用于重放。旧 routing snapshot 按其原版本算法读取，不用 `task-fit-v1` 重新解释历史决策。
