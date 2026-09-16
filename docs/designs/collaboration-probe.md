# 有界协作协议联调

[探针](../../examples/collaboration_probe.py) 顺序提交最多四个任务，各自使用全新 SQLite 数据库。
它验证真实供应商接口上的编排路径，不估计自然任务质量收益，也不把完成状态等同于业务验收。

## 场景与证据

| 场景 | 输入设计 | `path_observed` 证据 |
| --- | --- | --- |
| cascade | 第一版故意缺少必需标记，后续版本补齐 | 持久化拒绝评价及至少两个不同模型的调用 |
| generator_critic | generator 首版故意答错算术；critic 独立检查 | critic 的 revise 记录及带反馈的后续 generator 快照 |
| planner | required 规划，显式 single 策略，最多一次规划调用 | planner 预留记录、有效计划及执行调用 |
| tools | 请求一次固定只读 lookup | 宿主授权日志、已结算工具预留及模型续写调用 |

模型可能不遵从故意失败的测试指令，因此未观察到路径不自动说明内核存在故障。
critic 场景仅证明反馈传递与修订路径，不证明 critic 普遍正确；不自动提交质量反馈样本。
工具回调逐次检查任务身份、函数名、完整参数、零费用及单次权限，不访问外部服务。

## 运行与边界

```powershell
.venv/Scripts/python.exe examples/collaboration_probe.py --config <已核价配置.json> --output <新的输出目录> --total-budget 4000000
```

凭证仅从配置指定的环境变量读取，不自动载入 `.env`。配置需至少两个候选及一个支持 tools
的候选。价格版本必须显式填写且不得使用 example 占位值。供应商实际收费仍以账单为准。
`routing.planner_models` 必须明确配置非空池；critic 设置 `different_critic=true`。
默认每任务分配总预算的四分之一，每调用上限为任务预算的四分之一；供应商超支无法由客户端
绝对阻止。任务最多四次调用、两轮、一次失败尝试；规划最多两次共享调用，工具最多三次。
每任务截止时间 120 秒，预留两秒收尾；输出默认 4096 tokens，可用 `--output-tokens` 调至
16384。总预算余数不使用，不恢复、不重放既有输出目录。

任一未知费用（包括预留为零）、实际费用超出预留、预算或存储错误都会停止后续任务。
关闭受重复取消保护，取消后保留数据库及可读取的终态证据；失败不得自动重新调用供应商。

`manifest.json` 保存配置与限额，`report.json` 保存提交、结果、评价、调用快照、路由计划、
账本及供应商响应 ID（仅服务返回时有值）。SQLite 是完整原始证据，JSON 不导出供应商任意
错误消息。该目录应按运行证据保存；配置 URL 和模型输出仍可能包含内部信息。
路径观察要求调用已结算且结果成功；`final_artifact_accepted` 单独校验最终 JSON 的 answer。
`status` 表示运行调度生命周期；`validation_passed` 仅在四项均 completed、观察到目标路径且
最终答案验收通过时为 true。CLI 同时输出该字段，未通过时返回非零退出码。
读取使用只读连接和单一事务；不能依赖 `recovery_records` 枚举已正常完成的任务。

实现核对依据：[执行循环](../../src/engine/execution.rs)、
[预留及快照](../../src/engine/invocation.rs)、
[规划调用](../../src/engine/planning/attempt.rs)、[当前数据库结构](../../src/store/schema.rs)。
离线测试见 [test_collaboration_probe.py](../../tests/python/test_collaboration_probe.py)，
与真实供应商运行分别报告。
