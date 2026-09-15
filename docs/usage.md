# 使用指南

[返回项目首页与快速上手](../README.md)。本页保留规划、画像、账务与生命周期的完整约定；片段中的异步操作放在宿主的 async 函数中执行。

## 按需规划

`run/stream` 统一接收 [SubmissionSpec](../src/contracts/planning.rs)，包含目标、证据、验收、约束、工具及资源字段。
`schema_version` 缺省为 1，`task_type/strategy` 可选，`planning` 缺省为 disabled（不请求规划，要求显式策略）。
未指定类型且不规划时使用 general。显式与按需规划任务都保存有效计划、固定画像快照，再进入同一执行路径。
原 `submit/stream_submission` 方法已删除，调用方直接改用 `run/stream`。

```python
from model_collaboration_engine import parse_config

document = json.loads(Path("config/engine.json").read_text(encoding="utf-8"))
document["routing"]["planner_models"] = [document["providers"][0]["models"][0]["id"]]
config = parse_config(document, base_dir="config")
submission = json.loads(Path("examples/submission.json").read_text(encoding="utf-8"))
submission["task_id"] = str(uuid4())
submission["deadline_ms"] = int(time.time() * 1000) + 60_000
async with await Engine.open(config) as engine:
    result = await engine.run(submission)
```

该片段沿用首页快速上手的其余导入，并补充 parse_config。完整示例为 [examples/submit.py](../examples/submit.py)，从仓库根目录运行
`.venv/bin/python examples/submit.py`（Windows 使用 `.venv/Scripts/python.exe`）。需先配置兼容服务及凭证。
`routing.planner_models` 可省略；缺省为空池，显式执行仍可用，需要规划时会失败或采用宿主显式回退。
池中的候选仍受 `allowed_models`、供应商、地域、本地性和上下文约束，必须支持 `text`、`json`。

| `planning` 字段 | 行为 |
| --- | --- |
| `mode: disabled` | 要求显式策略，缺省类型为 `general`，跳过规划 |
| `mode: auto` | 类型和策略都明确时跳过，否则调用规划模型 |
| `mode: required` | 总是规划；与宿主显式类型或策略冲突的建议被拒绝 |
| `max_calls` | 缺省 1；当前版本只接受 1，不自动重试或换规划器 |
| `max_cost` | 本任务规划阶段费用占用上限，包含未知费用预留；同时受任务预算和 `max_call_cost` 限制 |
| `timeout_ms` | 1 至 86,400,000；受任务剩余执行时间进一步限制 |
| `fallback` | 可选 `{ "task_type": "general", "strategy": "single" }`；缺省不回退，且不得覆盖宿主明确选择 |

类型支持 `general`、`code_generation`、`code_review`、`information_extraction`、`reasoning`、
`writing`、`tool_execution`。提交可通过下节的 `routing_profiles` 启用分类型画像；
未配置时 `routing_profile_version` 为 `global-prior-v1`，同一路由使用固定的全局先验作为冷启动。持久化宿主反馈可为后续任务提供版本化质量样本。

规划只能选择 `single`、`cascade`、`generator_critic`，不能新增工具或放宽硬约束。
建议能力仅支持 `text/json/tools`，按并集增加要求；`tools` 需求必须已有宿主工具声明。
建议验收仅支持原有版本的 `nonempty`、`required_substrings` 和 `json_object`，按 OR/并集加强宿主要求。
未知字段、未知策略/能力、越权字段、工具调用或无效 JSON 均被拒绝，已知 usage 仍先结算。
`classification_confidence` 是模型自报元数据，不参与路由质量评分。

规划消耗同一任务的一次 `max_calls`。生成、评审、工具和续写继续使用余额；没有剩余调用额度时不再派发。
无合法规划候选、无效建议或规划服务失败，仅在提供合法 `fallback` 时才可继续；未知规划费用不会被释放。
任务取消、总截止时间或总预算耗尽、数据库/结算错误均不回退。有效计划找不到执行候选时返回 `routing`，不重新规划。
`different_critic` 原样保留，保证不同候选 ID；不同 ID 不保证供应商底层模型不同。

新事件包括 `planning_started`、`planning_completed`、`planning_failed`、`planning_fallback`、`plan_validated`。
规划模型的 `model_started` 使用 `node_id: planner`。规划 JSON 只作为受字节限制的账务证据保存，
不会发出候选 `content_delta`；`planning_completed` 仅表示建议已解析，不代表校验或任务成功。
取消或总资源终止可能先中断实时事件投递，最终以任务结果和持久化证据为准。
新入口沿用流提前退出、Python 重复取消、关闭等待清理的保护机制。

### 规划恢复证据

SQLite 表结构为 schema 2；新库直接创建当前结构，版本不匹配时拒绝打开，不自动迁移或清库。所有新任务的 `tasks.spec` 为 `{payload_version: 1, submission: ...}`；
当前 schema 内已有的任务证据保留读取能力，但不承诺跨 schema 兼容。
原始提交不覆盖，`tasks.plan` 记录规划建议/错误/回退与 `effective_plan`，有效计划和 `planned` 检查点原子保存。
检查点区分 `admitted`、`planning`、`planning_failed`、`planned` 和 `executing`；后续轮次检查点沿用执行证据。
规划原文位于对应 attempt 的 `outcome.planning_output`，包括截断与工具请求标记。

`recovery_records()` 新增 `submission`（旧任务为 null）和 `plan`。旧任务的 `task` 保持原样；新提交的 `task`
为任务投影，已计划时包含有效策略及验收。尚未计划时投影策略可为内部占位 `single`，不能据此推断已选策略，
应以 `submission`、`plan.effective_plan` 和检查点为准。恢复筛选条件不变，已完成且费用核实的任务不一定出现在查询中。
该接口不会恢复执行或重放调用。自定义 Rust `Store` 必须实现提交、计划及评价/反馈/指标事务接口，不再提供兼容占位实现。
旧记录的读取仅用于保留已有费用和恢复证据，不对应另一套执行算法；本次不删除现存数据库。

## 类型/角色画像

在解析配置、打开引擎前显式加载 [config/routing-profiles.json](../config/routing-profiles.json)：

```python
document = json.loads(Path("config/engine.json").read_text(encoding="utf-8"))
document["routing"]["profiles"] = json.loads(
    Path("config/routing-profiles.json").read_text(encoding="utf-8")
)
config = parse_config(document, base_dir="config")
submission["task_type"] = "writing"
submission["strategy"] = "single"  # auto 模式下类型与策略都明确时，不产生规划调用
```

示例数值仅演示格式，不是实测效果。所有任务执行都使用该配置；省略或设为 null 时生成默认快照，质量回退全局先验。
规划器在类型确定前使用宿主指定候选池和全局先验；这属于规划阶段的选择，不是旧接口的执行路径。

| 字段 | 约定 |
| --- | --- |
| `schema_version` / `version` | 当前 schema 为 1；非空版本同时标识画像、父类型、角色映射和权重 |
| `profiles` | 键为 `model_id + model_version + task_type + role + evaluator_version`；后者必须等于有效计划的 `acceptance.version` |
| `prior` / `prior_weight` | 同一验收定义下的校准先验 p∈[0,1]、正权重 k；Q=(k×p+accepted)/(k+samples) |
| `accepted` / `samples` | 宿主提供的通过数和有效样本数；0≤accepted≤samples≤2^53，prior_weight≤2^53 |
| `parents` | 类型的单父级映射，不允许循环，`general` 不能有父级 |
| `role_mappings` | 按原任务类型和实际节点 `invoke/generator/critic` 选择画像类型/角色；不改变节点权限或协作策略 |
| `weights` | 按映射后的画像类型查找权重，依次检查父类型、general，最后使用 `config.weights` |

画像优先级为精确类型/角色 → 沿配置父类型链同角色 → general/同角色 → `Model.acceptance` 全局先验。
不同模型版本、角色或验收版本的样本不会混用；找不到画像表示缺失，不把质量设为零。
每次选择记录 `exact/parent/general/global_prior` 来源。零样本但有正 prior_weight 的显式画像仍是有效先验。
未知模型 ID、重复画像键、重复角色映射、非法数值/权重/版本在打开引擎时拒绝。
历史模型版本的条目可以保留，但只有与当前候选版本相等的条目能匹配。

默认代码生成任务的评审节点使用 `code_review/critic`，其他节点沿用有效计划的类型/角色。
宿主映射只改变查询画像的标签：把 critic 映射到其他画像角色也不会授予工具权限，
`different_critic`、能力、地域、本地性、上下文、预算及调用上限继续生效。
价格、延迟、可靠性和不确定性独立于质量样本；不会用任务内 Health 通过计数更新质量。
业务反馈必须由宿主明确提交；不会把 critic 输出 pass 的次数当作正确率。

### 快照与复现边界

有效计划保存前固定 `routing_snapshot`，与计划和 planned 检查点一起原子写入 `tasks.plan`。
快照含版本、配置摘要、候选模型配置元数据、逐节点权重与逐模型采用的质量证据；
包含凭证环境变量名，不含环境变量中的密钥值。任务执行、换模、级联和工具续写不刷新画像。
`attempts.metadata.route` 保存选中画像、快照摘要、权重版本及排除原因，`route_inputs` 保存
当次余额、已排除候选、质量门槛和临时 Health；输入 token 估算保存在 `route.estimated_input`。

Rust 的 `router::route_profiled` 可以用原有效 TaskSpec、已保存快照及当次 RouteRequest 复算。
延迟归一化和不可用时间判断采用快照时刻；实际执行仍在每次派发前检查实时截止时间和余额。
同一输入可复现模型、评分、排除原因和质量证据，新生成的 decision_id 不要求相同。
这是纯路由复算，不会重放模型/工具请求，也不保证供应商输出可复现。
SQLite schema 为 2；当前结构内保留恢复查询，新写入统一保存提交、计划和快照。

当前接口与验证记录见 [统一执行路径](../docs/designs/execution-unification.md)。先前的 [C1+C2 实施记录](../docs/designs/task-type-routing-implementation.md) 保留为历史决策。

## 验收反馈与持久化指标

`completed` 表示执行检查通过，不等于业务验收。每个候选产物分别保存确定性检查与
可选 critic 结论；critic 中断或返回非法结构时，已生成的候选及确定性证据仍保留。

```python
result = await engine.run(submission)
await engine.record_feedback({
    "feedback_id": "review-unique-id",  # 宿主生成，可用于网络/取消后重试
    "task_id": result["task_id"],
    "artifact_id": result["artifact"]["artifact_id"],
    "kind": "business_acceptance",
    "evaluator_version": submission["acceptance"]["version"],
    "accepted": False,  # 实际业务检查结果，不复制 completed 状态
    "reason": "人工检查发现事实错误",
})
evaluations = await engine.evaluations(result["task_id"])
metrics = await engine.metrics()
```

- 只接受已终态任务的已保存产物；版本必须等于有效验收的 `acceptance.version`。
- `business_acceptance` 归因到生成该产物的最终模型 attempt，而不是所有重试或工具续写。
  `critic_correctness` 表示宿主认定 critic 判断是否正确（正确否决也可 accepted=true），要求存在有效 critic 结论。
- 模型身份、模型版本及映射后的类型/角色从保存的调用证据推导，不接受宿主指定归因。
  两种反馈分别统计，即使宿主把 critic 映射到生成角色，也不会混算。
- 同 feedback_id、同内容重复提交成功且只计一次；同 ID 内容冲突或同产物/同种类的新 ID 拒绝。
  本批不提供反馈撤销或改判接口。取消等待不保证写入撤销，应以同 ID、同内容重试确认。
- 新任务在计划保存前读取一致性指标快照，保存 `feedback_revision` 与 `feedback_hash`。
  相同键有持久化反馈时，其 accepted/samples **替换**配置样本数，避免未知重叠造成双计；
  p/k 仍取同键配置，没有配置时采用 Model.acceptance 和 k=10。
  仍按精确类型、父类型、general、全局先验回退；任务内不会刷新。
- `metrics.quality` 保留版本化键和反馈种类；`tasks` 是执行终态数量。
  `calls` 区分 model/tool/unknown，按配置摘要、模型版本、节点或工具名分组，
  包含规划、重试、工具及续写的状态、整数 microcredits 已知费用、未知预留及平均派发耗时。
  工具组的模型身份为 unknown，具体工具见 tool_name；历史无指标证据的调用状态记 unknown。
  succeeded 仅表示调用返回完成结果，不等于产物通过业务验收。
- 延迟为本地派发到返回/错误/取消的单次耗时，不含排队/账务，也不是任务总延迟；
  未派发不计延迟样本。缺失费用不是零；即使预留为零，unknown_cost_attempts 仍标识费用未核实的调用，后续对账会反映在指标中。
- 指标当前为全库查询，不是常数时间计数缓存；大规模历史库的分页、分 cohort 查询及性能优化后续处理。

离线对比：对同一任务集、相同验收定义分别使用独立数据库，导出 `metrics()` 为两个 JSON 后执行
`python examples/compare_metrics.py baseline.json candidate.json`。
示例只汇总已导出的证据，不重放模型或工具；分别报告执行完成率、人工介入率、分版本业务反馈、
费用、调用耗时与规划开销。未反馈样本不能算失败或成功，也不能据此宣称真实收益。

### 开发期数据库规则

当前不维护跨版本迁移链，已移除 `SqliteStore::migrate` 和 schema 1→2 自动升级路径。
打开不匹配的 schema 时返回 `schema` 错误，details 包含 current、target 和
`action=backup_and_rebuild`。原文件保持不变，数据库打开操作不会删除、重建或补造历史记录。

需要重建时，先关闭所有使用该库的引擎：

1. 确认是否包含真实调用、未结算费用或需要复现的证据；这些数据应先备份并完成必要对账。
2. 将配置文件的 `storage.database_path` 改为新的、尚不存在的文件路径，重新加载配置并打开引擎。
3. 新库不继承旧任务、反馈或费用。旧库保留，确认无用后再由宿主显式删除。

不需要为纯模拟数据维护迁移；回归样本以测试夹具保存在 Git。
真实调用费用保留到对账完成，需要复现的问题和算法对比样本选择性归档。
跨版本迁移在正式发布前按实际需要设计；不会因为处于开发期而自动丢弃账务证据。

实现范围与验证见 [反馈与指标实施记录](../docs/designs/feedback-metrics-implementation.md)。

## 宿主工具

下面的片段放在快速上手的 `main()` 中，替换原来的引擎上下文；沿用已加载的 `config` 和 `task`。服务端模型必须实际支持工具调用。

```python
async def lookup(request):
    # 宿主在这里按请求授权，并验证业务参数。只执行注册的本地查询。
    args = request["call"]["arguments"]
    if not isinstance(args, dict) or set(args) != {"query"} or not isinstance(args["query"], str):
        raise ValueError("invalid lookup arguments")
    value = {"engine": "模型协作执行内核"}.get(args["query"], "未找到")
    return {"output": value, "actual_cost": 0}

document = json.loads(Path("config/engine.json").read_text(encoding="utf-8"))
document["models"][0]["capabilities"].append("tools")
config = parse_config(document, base_dir="config")
task["tools"] = [{
    "name": "lookup", "description": "查询本地术语表",
    "parameters": {"type": "object", "properties": {"query": {"type": "string"}},
                   "required": ["query"], "additionalProperties": False},
    "max_cost": 10,
}]

async with await Engine.open(config) as engine:
    async with engine.stream(task, tools={"lookup": lookup}) as run:
        async for event in run:
            if event["kind"] == "content_delta":
                print(event["data"]["text"], end="", flush=True)
        result = await run.result()
        print(result)
```

模型配置需声明 `tools` 能力。工具函数在宿主 asyncio 循环执行，必须返回 awaitable，
不能阻塞事件循环，且应配合取消。事件回调也不能阻塞事件循环；异步事件回调受执行截止时间限制。
引擎检查工具名、对象参数和同一模型对话内重复调用 ID；
完整 JSON Schema 与业务授权由宿主验证。整批工具请求检查通过后串行执行，工具错误不自动重试。
评审节点不接收工具定义，也不执行工具请求。

回调请求包含 `task_id`、`execution_id`、`model_attempt_id`、`call`（`id/name/arguments`）、
`max_cost` 和执行截止时间 `deadline_ms`。`execution_id` 是账本尝试 ID，也可供宿主追踪执行；
不代表跨任务去重或 exactly-once 保证。远端副作用不一定能随本地取消而撤销。
回调返回 `output` 字符串与 `actual_cost`（非负整数或 `null`）。异常、取消或未知成本保留预留金额；
已知业务失败可返回描述失败的 `output` 及已知费用，交由模型继续处理。

## 事件订阅

`run()` 可通过 `on_event` 接收事件；需要自行消费时使用 `stream()`，如上例所示。

事件包括 `task_started`、`model_started`、`content_delta`、`tool_requested`、`tool_completed`、
`evaluation`，共享单任务递增序号。`model_started`/`tool_requested` 表示准备调用，后续预算检查仍可能拒绝调用。
内容增量属于候选产物；最终状态、产物和账本以 `await run.result()` 为准，流结束本身不表示成功。
编排事件写入 SQLite，内容增量仅实时传输；数据库中的账本审计事件不是可重放的统一流。

事件队列受 `event_capacity` 与 `event_max_bytes` 限制，消费者变慢会产生背压；超过执行截止时间将结束任务。
先消费流再等待结果，或并行消费与等待；只等待结果而不消费流可能触发背压超时。
提前退出流的上下文会取消运行并等待清理。Rust 可用 `event_channel()` 和 `run_with_host()`，
注入 `ToolExecutor`；关闭 Rust 事件接收端只停止订阅，取消执行需要传递取消令牌。

## 执行与持久化约定

调用前事务性预留预算并计数；返回 usage 后结算，未知用量保留预留金额。
适配器收到有效 usage 后立即保留用量与请求身份；后续工具参数解析、SSE 解析失败或取消、超时仍按已知用量结算。失败状态与费用核实分别记录，费用超出预留时仍先记账再报错。
Rust 自定义适配器可通过 `InvokeRequest.evidence.record` 在收到证据时记录 usage 与请求身份；应在后续可失败解析或 `.await` 前调用，以便引擎在 future 被取消后完成结算。正常返回的 `ModelOutput.usage` 仍用于成功返回路径的结算。
模型报错后的换模型尝试也占用总调用额度。实际用量超过预留时先记账再报错。
每次工具执行也计入 `max_calls`，工具费用按声明的 `max_cost` 预留；模型续写重新计算完整消息与工具定义的输入预算。
工具批次不提供事务回滚，后续工具遇到额度不足时，之前完成的工具及费用仍然保留。
预算是调用准入限制，无法阻止供应商报告超出约定的实际费用。
输入估算使用序列化消息字节数加 framing 余量，保守估算可能排除实际可用模型。

Rust 调用方通过 `CancellationToken` 取消，并继续等待 `run` 返回，以完成结算与任务状态写入。
直接丢弃 future 或进程崩溃可能留下 running 记录；通过 `Store::records` 检查，
使用有证据的 `Store::reconcile` 对未知费用对账。当前不自动重放恢复任务。
对账在同一账本事务中追加 `reconciliation_evidence`，保留已有请求身份、调用状态、耗时与规划证据；重复确认相同费用不会重复计费，冲突费用被拒绝。缺失的原始调用指标不会因对账而补造。
Python 包装器将 asyncio 取消传递给 Rust，并等待本次清理；引擎用完后调用 `close` 或退出引擎上下文。
执行截止时间保留 `finalization_ms` 用于结束写入，但数据库写入没有硬实时完成保证。

## 恢复检查

`await engine.recovery_records()` 返回需要关注的持久化任务及调用证据，Rust 对应
`Engine::recovery_records()` / `Store::records()`。查询不会重放模型或工具调用，也不会修改任务状态或账本。

```python
records = await engine.recovery_records()
for record in records:
    print(record["task"]["task_id"], record["status"], record["ledger"])
    for attempt in record["attempts"]:
        print(attempt["attempt_id"], attempt["state"], attempt["cost"])
```

筛选范围为 `running`、`human_required`、仍有预留费用，或存在 `pending` / `unresolved`
调用的任务；包括预留为 0 但结果未知的调用。每条记录包含原始 `task`、`config_hash`、
`status`、`checkpoint`、`ledger`、可空的 `result`，以及按创建顺序排列的全部 `attempts`。
调用证据包含 `attempt_id`、`amount`（预留额）、可空的 `cost`、`state`、`metadata` 与可空的 `outcome`。
未知费用用 `null` 表示，与已确认费用为 0 不同。

查询使用一致的数据库读事务；正在执行的任务也可能出现在结果中，因此 `running` 不等于崩溃遗留。
记录按任务 ID 排序，目前一次返回全部匹配记录，适合受控规模的本地账本。
关闭开始后引擎拒绝新查询；成功重开数据库后可再次检查。查询结果不授予重试副作用的权限，
也不保证原任务配置与当前配置相同。对账仍由 Rust `Store::reconcile` 接收有证据的实际费用。
恢复检查要求数据库与当前 SQLite schema 2 匹配；schema 1 等不匹配版本会在打开时被拒绝，原文件保持不变。备份与显式重建步骤见[开发期数据库规则](#开发期数据库规则)。当前结构内旧任务载荷的解析能力不代表支持打开旧 schema 数据库。

## 优雅关闭

`await engine.close()` 首先停止新任务准入，排队任务返回 `closed`；已经开始的任务继续执行。
在 `close_grace_ms` 内等待这些任务自然结束，超时后取消仍在执行的任务，再等待最多 `cleanup_timeout_ms`
完成费用处理与终态写入。只有任务排空后才关闭数据库；关闭不会取消调用方令牌关联的其他任务。
两个参数均以毫秒计，允许范围为 0 到 86,400,000；0 表示不提供相应等待期。

若清理超时，返回 `cleanup_timeout`，数据库所有权继续保留，引擎保持停止接收任务的状态。
活动任务仍可完成清理，调用方随后可以再次调用 `close()`。已成功关闭后重复或并发关闭均可安全返回。
数据库自身的关闭操作及宿主回调清理不受上述两个等待期的硬时间保证。

Python 的关闭操作由受保护的后台任务执行；调用方取消或重复取消 `close()` 时，仍等待关闭处理完成，
再传播取消异常。关闭还会等待正在处理的宿主工具回调清理，回调必须配合取消。
在工具回调内部关闭其所属引擎会形成等待自身的循环，因此该调用被明确拒绝；由回调外的宿主管理关闭。
关闭后，调用方仍应等待自己创建的任务以取得结果或异常。
Rust 调用方若直接丢弃 `close` future，应再次调用并等待关闭；直接丢弃运行 future 的恢复边界仍适用。
