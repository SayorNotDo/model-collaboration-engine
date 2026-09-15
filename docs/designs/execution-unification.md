# 统一执行路径

用户确认：项目初期不默认维护旧 API 与旧路由算法。此决策取代 C1+C2 的双路径兼容方案。

## 当前接口

- Python 只提供 run/stream 两种消费方式，输入均为 SubmissionSpec 字典。原 submit/stream_submission 已删除。
- Rust 使用 run/run_with_host；SubmissionSpec 是统一执行输入，显式 TaskSpec 可经 Into 规范化为 general、disabled 提交。这是构造便捷转换，不是另一套执行器。
- schema_version 缺省 1；planning 缺省 disabled，必须显式指定策略，未给类型则 general。auto 在类型或策略缺失时规划，required 总是规划。
- 所有任务依次经过准入、保存提交、计划校验、固定画像快照、原子保存计划、执行及账务终态。
- 没有画像配置时也创建快照，版本为 global-prior-v1；Q 回退模型全局先验。评分、级联、换模和工具续写共用同一执行路由，任务内通过次数不改变静态 Q。
- 规划阶段尚未确定类型，按宿主候选池与全局先验选择规划器；它仍与执行共用预算、调用次数、取消及关闭清理。

## 存储与约束

所有新任务统一写入提交 envelope、有效计划和路由快照。Store 实现必须提供 create_submission/save_plan；
删除默认报错占位及旧 TaskSpec 独立写入方法 create，仅保留统一提交写入。当前 schema 内的旧任务载荷保留只读解析，以便恢复检查与未知费用对账；
不删除用户数据库，也不因接口升级重新执行历史模型或工具调用。

后续数据库策略已调整为仅接受当前 SQLite schema 2；schema 1 等不匹配数据库拒绝打开，不自动迁移。旧载荷解析与数据库版本兼容是不同层次，当前操作规则见 [README 开发期数据库规则](../../README.md#开发期数据库规则)。

预算预留、真实费用结算、未知费用保留、工具授权和整批检查、取消排空与关闭重试保持既定业务约束。
开发期接口允许有文档、有测试的变更；正式稳定版本发布后再制定兼容承诺。

## 验证

仓库内原 submit 调用迁移至 run；规划模式、类型路由、全局冷启动、显式 TaskSpec 规范化、原生 HTTP/SSE、
工具和取消/关闭测试通过统一路径执行。

最终验证：Rust 50 项通过，Clippy（-D warnings）与 cargo fmt --check 通过；
使用 CARGO_INCREMENTAL=0 重建原生扩展后，Python 53 项通过。检查 56 个文档相对链接和 33 个源码摘要，
git diff --check 通过。测试使用注入适配器及两个本地 HTTP/SSE 端点，未进行真实供应商联调。

现有 tests/engine.rs 为 775 行，本次仅迁移存储夹具；新增统一路径场景在 tests/routing_profiles.rs，
不继续扩充已有测试文件。store.rs 减至 430 行，保留完整事务边界。本批通过 feat/task-type-routing 分支提交与审查。

上述图形待办已由后续 archify 2.17 更新处理；当前产物与各项验收结论见[技术图索引](../diagrams/README.md)，本节测试数量仍是该批历史记录。
