# Model Collaboration Engine

领域术语与概念关系见 [CONTEXTS.md](CONTEXTS.md)。

Rust 执行内核与 Python 异步接口。当前实现显式选择的三种策略：

- `single`：单个生成步骤（可包含工具调用及模型续写）后执行确定性验收；未通过返回 `human_required`。
- `cascade`：验收失败后选择尚未尝试、质量先验不低于上一模型的候选。
- `generator_critic`：生成、确定性验收、独立 JSON 评审，以及有轮数上限的修订。

路由先检查能力、上下文、模型/供应商/地域限制和预算，再按配置权重评分。
所有费用均为整数 microcredits；价格是每 token 费用，由调用方配置。

## 构建与验证

需要 Rust 1.88+、Python 3.12+，以及平台 C/C++ 编译工具（默认编译内置 SQLite）。

```powershell
cargo test --all-features
python -m venv .venv
.venv/Scripts/python -m pip install maturin
.venv/Scripts/python -m maturin develop
.venv/Scripts/python -m pip install pytest pytest-asyncio
.venv/Scripts/python -m pytest
```

Python 使用 JSON 可序列化字典；完整字段见 `src/contracts.rs`，配置及任务样例见 `examples/`。

```python
import asyncio
import json
import time
from model_collaboration_engine import Engine

async def main():
    with open("examples/config.json", encoding="utf-8") as f:
        config = json.load(f)
    with open("examples/task.json", encoding="utf-8") as f:
        task = json.load(f)
    task["deadline_ms"] = int(time.time() * 1000) + 60_000
    async with await Engine.open(config) as engine:
        result = await engine.run(task)
        print(result)

asyncio.run(main())
```

样例使用本地兼容服务和占位模型，运行前需要修改服务地址、模型名称、价格和凭证环境变量。
相同数据库中的 `task_id` 必须唯一。API 密钥只通过环境变量读取，不写入配置。

## 宿主工具与事件订阅

`Engine.run(task, tools=..., on_event=...)` 返回最终结果；可选 `on_event` 接收事件字典。
`Engine.stream(task, tools=...)` 返回异步上下文管理器，进入后可以异步迭代事件：

```python
async def lookup(request):
    # 宿主在这里按请求授权，并验证业务参数。只执行注册的本地查询。
    args = request["call"]["arguments"]
    if set(args) != {"query"} or not isinstance(args["query"], str):
        raise ValueError("invalid lookup arguments")
    value = {"engine": "模型协作执行内核"}.get(args["query"], "未找到")
    return {"output": value, "actual_cost": 0}

task["tools"] = [{
    "name": "lookup", "description": "查询本地术语表",
    "parameters": {"type": "object", "properties": {"query": {"type": "string"}},
                   "required": ["query"], "additionalProperties": False},
    "max_cost": 10,
}]

async with engine.stream(task, tools={"lookup": lookup}) as run:
    async for event in run:
        if event["kind"] == "content_delta":
            print(event["data"]["text"], end="", flush=True)
    result = await run.result()
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
模型报错后的换模型尝试也占用总调用额度。实际用量超过预留时先记账再报错。
每次工具执行也计入 `max_calls`，工具费用按声明的 `max_cost` 预留；模型续写重新计算完整消息与工具定义的输入预算。
工具批次不提供事务回滚，后续工具遇到额度不足时，之前完成的工具及费用仍然保留。
预算是调用准入限制，无法阻止供应商报告超出约定的实际费用。
输入估算使用序列化消息字节数加 framing 余量，保守估算可能排除实际可用模型。

Rust 调用方通过 `CancellationToken` 取消，并继续等待 `run` 返回，以完成结算与任务状态写入。
直接丢弃 future 或进程崩溃可能留下 running 记录；通过 `Store::records` 检查，
使用有证据的 `Store::reconcile` 对未知费用对账。当前不自动重放恢复任务。
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
本次扩展未改变 SQLite schema，现有版本 1 数据库可直接读取。

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

## 当前边界

已打通 Rust 执行闭环、SQLite 账本、Python 扩展和离线验收测试。
已接入宿主异步工具回调、结果回传、工具预算结算和有界事件订阅，覆盖两个端点的本地模拟联调。
健康统计目前仅在单任务内使用。
已接入分阶段优雅关闭与 Python 关闭取消保护；`close_grace_ms`、`cleanup_timeout_ms` 已参与调度。
自动恢复和持久化路由指标仍待后续实现。
已提供 Rust/Python 恢复检查入口，可查看调用证据；检查不会自动恢复或重放任务。
真实供应商联调尚未验证。
