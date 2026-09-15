# D：持久化反馈与指标

日期：2026-09-15。基线：master 4f8c1b7；工作分支 feat/feedback-metrics。
本记录描述提交前的实现与本地验证；发布与合并状态以 GitHub PR 为准。

## 实现与边界

1. 候选生成后先保存 EvaluationRecord（产物、确定性检查），再调用 critic；
   有效 critic 结论追加对应 attempt ID。取消或非法 critic 输出不抹去候选证据。
2. Engine/Python 新增 record_feedback、evaluations、metrics。
   反馈只接受终态任务的具体产物，验收版本须匹配有效计划；归因来自实际调用的固定路由证据。
   business_acceptance 与 critic_correctness 分开统计，critic 的正确否决也可以被宿主认可。
3. 同反馈 ID/同内容幂等，不同内容或同产物/种类的第二个 ID 拒绝。
   feedback ID、task ID、artifact ID、验收版本非空且最多 256 字节，reason 最多 4096 字节。
   不提供撤销/改判接口；取消等待后用同一 payload 重试确认。
4. 模型与工具在现有结算事务中保存调用状态及单调时钟耗时。
   规划、失败尝试、工具续写全部统计；费用直接读取账本证据，未知费用不释放也不当作零。
   succeeded 是返回完成结果的调用状态，不表示确定性检查、critic 或业务验收通过。
5. 只在有效计划保存前读取反馈。RoutingSnapshot schema 2 保存反馈 revision/hash，
   已保存的 schema 1 快照仍可复算；提交 envelope 和 RoutingProfiles 配置版本仍为 1。
   运行中任务不刷新。历史反馈与静态配置同键时，持久化计数替换静态计数，避免未知数据交叠双计；
   p/k 使用同键配置，没有同键配置时使用模型全局先验和 k=10。
6. SQLite schema 1→2 事务新增 evaluations、feedback 及索引，不改写历史任务、attempt 或账本。
   只升级已识别的引擎文件；未知/更高版本拒绝，历史候选没有证据时不补造反馈。
   新接口通过 close_gate 与关闭互斥，存储事务仍受所有权锁和 gate 保护。
7. examples/compare_metrics.py 比较两个已导出指标文件，保留评价版本分层、
   未知费用、规划开销及延迟样本分母；不重放外部调用。
   相同任务集的真实模型收益评测仍待开展，不从模拟测试推断改善。

## 指标口径与未实施项

- quality 键：模型配置 ID、模型版本、映射后的任务类型/角色、验收版本及反馈种类。
- calls 按 model/tool/unknown、模型版本、配置摘要、节点/工具名分组。
  工具不伪造模型归因，model_id/model_version 为 unknown；工具名独立返回。
  平均延迟仅对带 latency_ms 的派发样本计算，未派发不计入。
- tasks 只记录执行状态分布，不推断业务成功。终态任务的候选可独立反馈；
  被取消的任务如已有候选也可验收，但没有 critic 记录就不能评价 critic 正确性。
- 暂无外部评分采集、端点健康自动调参、反馈修改、自动任务回放。
- 暂无 task-level 总延迟、百分位延迟、时间窗口/cohort 查询与大库性能基准。
  当前全库读取的复杂度随调用证据增长；离线对比使用独立数据库控制任务集。
- 全库质量样本可以跨配置摘要积累，但模型 ID/版本/验收定义须由宿主正确管理；
  若模型行为或验收定义发生变化，应变更相应版本。

## 结构与规范

新增反馈契约、反馈事务与指标读取分别归属 contracts/feedback、store/feedback、store/metrics；
Router 接收纯数据，不持有数据库。未新增依赖。
store.rs 保留已有账本/所有权事务，新职责在子模块；tests/engine.rs 仅增加 Store 委托和工具指标断言（797 行，未越 800 行拆分线），
新反馈场景放独立 tests/feedback.rs。指标查询拆为质量、任务和调用汇总，保持一个读事务。

## 验证记录

本地 Linux、Rust 1.98.1 / Python 3.12，按顺序实际执行：
- `cargo fmt --check`：通过。
- `CARGO_INCREMENTAL=0 cargo test --all-features --locked`：59 项通过。
- `CARGO_INCREMENTAL=0 cargo clippy --all-features --all-targets --locked -- -D warnings`：通过。
- `CARGO_INCREMENTAL=0 python -m maturin develop --locked`：重建成功。
- `python -m pytest -q`：56 项通过，包含两个本地 HTTP 端点与离线汇总。
- `git diff --check`、受影响 Markdown 相对链接与源码映射 SHA-256 核对：通过。

初次检查发现新快照版本未放行、测试 critic 夹具缺少必需字段及测试格式差异，
均已修正并以上述最终全量验证复核。零预留且未知 usage 的调用也显式计入
unknown_cost_attempts，不会混同已知零费用。
覆盖：幂等与冲突、错误任务/产物/验收版本、critic 分层、取消后的候选保留、
后续选模变化、旧快照不变、模型/验收版本隔离、旧库保留预留费用、规划及失败调用指标、
工具指标、两个 HTTP/SSE 端点的 Python 接口及离线汇总分母。

技术图：模块拓扑和关闭状态不变，但宿主、引擎、存储及路由职责更新。
当前无可用 archify 命令，图源与源码证据更新；HTML 未重生成，
结构/浏览器/视觉验收未执行，旧回执不代表本批通过。
