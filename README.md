# Model Collaboration Engine

为 Python 异步代理提供 Rust 执行内核，在调用预算、调用次数和截止时间内协调模型、宿主工具与验收流程，并使用 SQLite 记录任务状态和费用证据。

适合需要显式选择协作策略、追踪调用费用，并保留宿主工具授权控制的应用。Python 使用 JSON 可序列化字典；Rust 可通过组件接口接入自定义适配器、存储和工具执行器。

- **模型协作**：支持 `single`、`cascade`、`generator_critic` 三种策略，以及 `chat_completions`、`responses` 两类端点。
- **预算与账本**：调用前预留，返回已知费用后结算；未知费用保留待对账证据。
- **宿主集成**：异步工具回调、有界事件流、取消清理与分阶段关闭。
- **恢复检查**：读取任务和调用证据，供宿主判断后续处理方式。

当前验证覆盖离线测试和本地模拟服务；真实供应商联调尚未验证，自动恢复与任务重放尚未实现。

## 快速上手

### 1. 从源码安装

需要 Rust 1.88+、Python 3.12+ 和平台 C/C++ 编译工具。默认编译内置 SQLite。以下命令在仓库根目录的 PowerShell 中执行：

```powershell
python -m venv .venv
.venv/Scripts/python.exe -m pip install maturin
.venv/Scripts/python.exe -m maturin develop
```

Linux/macOS 将 `.venv/Scripts/python.exe` 替换为 `.venv/bin/python`。修改原生代码后需要重新运行 `maturin develop`。

### 2. 配置模型服务

编辑 [examples/config.json](examples/config.json)，将服务地址、模型名称、能力、上下文上限和价格替换为实际配置。示例指向本地兼容服务，本项目不负责启动模型服务。

| 配置项 | 使用说明 |
| --- | --- |
| `endpoint` / `base_url` | 选择 `chat_completions` 或 `responses`，填写兼容服务的 API 基地址 |
| `model` / `capabilities` | 填写服务端模型名并如实声明能力；工具调用需要 `tools` 能力 |
| `api_key_env` | 填写保存密钥的环境变量名称，密钥本身不写入配置 |
| `input_price` / `output_price` | 每 token 的整数 microcredits 费用，由调用方配置 |
| `database_path` | 保存任务与账本的 SQLite 文件路径 |

在当前终端设置与 `api_key_env` 对应的变量，再运行程序：

```powershell
$env:MODEL_API_KEY = "<替换为服务凭证>"
```

[examples/task.json](examples/task.json) 默认要求 `local_only: true`。接入远端服务时，同时调整模型的 `local` 声明与任务约束，使其符合实际部署。

### 3. 运行任务

将下面代码保存为仓库根目录的 `quickstart.py`，使用 `.venv/Scripts/python.exe quickstart.py` 运行：

```python
import asyncio
import json
import time
from pathlib import Path
from uuid import uuid4

from model_collaboration_engine import Engine


async def main():
    config = json.loads(Path("examples/config.json").read_text(encoding="utf-8"))
    task = json.loads(Path("examples/task.json").read_text(encoding="utf-8"))
    task["task_id"] = str(uuid4())
    task["deadline_ms"] = int(time.time() * 1000) + 60_000

    async with await Engine.open(config) as engine:
        result = await engine.run(task)
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    asyncio.run(main())
```

每次新任务使用唯一 `task_id`；同一数据库拒绝重复 ID。`deadline_ms` 是绝对 Unix 毫秒时间戳，示例文件中的 `0` 必须在运行前更新。上下文退出时会关闭引擎并等待相应清理。

## 选择协作策略

| `strategy` | 执行方式 |
| --- | --- |
| `single` | 单个生成步骤，可包含工具调用及模型续写；确定性验收未通过时返回 `human_required` |
| `cascade` | 验收失败后选择尚未尝试、质量先验不低于上一模型的候选 |
| `generator_critic` | 生成、确定性验收、独立 JSON 评审，并在轮数上限内修订 |

路由先检查能力、上下文、模型、供应商、地域和预算约束，再按配置权重评分。切换策略前需配置满足条件的候选模型；单模型样例不保证能够升级或使用不同评审模型。

模型调用、工具执行和模型续写共用 `max_calls`。任务总预算使用 `budget`，工具按声明的 `max_cost` 预留费用。完整配置与结果字段见 [src/contracts.rs](src/contracts.rs)，领域术语见 [CONTEXTS.md](CONTEXTS.md)。

## Python 接口速查

| 接口 | 用途 |
| --- | --- |
| `await Engine.open(config)` | 打开引擎与账本 |
| `await engine.run(task, tools=..., on_event=...)` | 消费事件并返回最终结果；事件回调可选 |
| `engine.stream(task, tools=...)` | 创建异步上下文，在其中迭代事件并获取结果 |
| `await engine.recovery_records()` | 只读检查需关注的持久化任务与调用证据 |
| `await engine.close()` | 停止准入，等待任务排空并关闭数据库 |

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

config["models"][0]["capabilities"].append("tools")
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
当前恢复检查支持版本 1 的 SQLite 数据库。

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

## 验证与开发

Rust 检查、Python 扩展重建与测试命令集中维护在 [本地验证流程](.agent/README.md)。共享同一 `target/` 时，Cargo 检查与 maturin 构建按顺序执行。

| 文档 | 内容 |
| --- | --- |
| [领域模型](CONTEXTS.md) | 术语、职责与概念关系 |
| [模型规划与分类型路由设计](docs/designs/planner-routing-design.md) | 待实现方案：规划契约、资源边界、路由画像与分批验收 |
| [架构与关闭流程图](docs/diagrams/README.md) | 系统关系、关闭状态及图形验收记录 |
| [仓库协作指南](AGENTS.md) | 代理工作入口、业务约束和交付要求 |
| [代码参考](.agent/code-map.md) | 模块入口与回归场景 |

## 当前边界

- 已提供宿主异步工具回调、工具预算结算、有界事件订阅、恢复检查及分阶段优雅关闭；两个端点均有本地模拟测试覆盖。
- 健康统计仅在单任务内使用，持久化路由指标尚未实现。
- 恢复检查只读取证据，不自动恢复或重放任务；对账通过 Rust `Store::reconcile` 完成。
- 真实供应商联调尚未验证。预算控制调用准入，不能保证供应商最终报告的实际费用不超出预留。
