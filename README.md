# Model Collaboration Engine

Rust 执行内核与 Python 异步接口。当前实现显式选择的三种策略：

- `single`：调用一次并执行确定性验收；未通过返回 `human_required`。
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

## 执行与持久化约定

调用前事务性预留预算并计数；返回 usage 后结算，未知用量保留预留金额。
模型报错后的换模型尝试也占用总调用额度。实际用量超过预留时先记账再报错。
预算是调用准入限制，无法阻止供应商报告超出约定的实际费用。
输入估算使用序列化消息字节数加 framing 余量，保守估算可能排除实际可用模型。

Rust 调用方通过 `CancellationToken` 取消，并继续等待 `run` 返回，以完成结算与任务状态写入。
直接丢弃 future 或进程崩溃可能留下 running 记录；通过 `Store::records` 检查，
使用有证据的 `Store::reconcile` 对未知费用对账。当前不自动重放恢复任务。
Python 包装器将 asyncio 取消传递给 Rust，并等待本次清理；请在所有任务退出后 `close`。
执行截止时间保留 `finalization_ms` 用于结束写入，但数据库写入没有硬实时完成保证。

## 当前边界

已打通 Rust 执行闭环、SQLite 账本、Python 扩展和离线验收测试。
工具声明目前在执行前明确拒绝，待加入宿主工具执行接口；不会隐式执行模型提出的工具调用。
事件传输模块已有实现，Engine 当前未公开流式订阅。健康统计目前仅在单任务内使用。
自动恢复、后台优雅关闭、持久化路由指标和宿主工具回调仍待后续实现；
`close_grace_ms`、`cleanup_timeout_ms` 暂未参与调度。
真实供应商联调尚未验证。
