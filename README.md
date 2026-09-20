# Model Collaboration Engine

**让 Python 异步代理在明确的预算和权限内协调多个模型，并留下可核查的执行证据。**

Rust 执行内核 · Python asyncio 接口 · SQLite 账本

[快速上手](#快速上手) · [配置指南](config/README.md) · [使用指南](docs/usage.md) · [当前进度](docs/designs/current-status.md) · [技术图](docs/diagrams/README.md) · [联调记录](docs/designs/live-evaluation-results.md)

## 为什么使用它

当应用需要选择模型、按验收结果升级或评审，并追踪每一次调用的费用时，Model Collaboration Engine 提供统一的执行边界：

- **有界协作**：`single`、`cascade`、`generator_critic` 共用预算、调用次数和截止时间。
- **可追溯账务**：调用前预留，收到 usage 后结算；费用未知时保留预留，支持有证据的对账。
- **宿主管理权限**：工具通过显式注册的异步回调执行，由宿主授权和校验参数。
- **可解释路由**：先筛选能力、地域和预算约束，再评分；版本化质量画像在任务内固定。
- **任务需求评估**：schema 3 可提交结构化任务事实，规则和类型化决策信号共同确定最低执行档位；评估结果随计划和路由快照固定。
- **完整生命周期**：流式事件、取消清理、恢复查询和分阶段关闭，最终结果以终态和账本为准。

Python 使用 JSON 可序列化字典；Rust 可以注入自定义适配器、存储和工具执行器。内置协议适配支持 `chat_completions` 与 `responses`。

**项目状态：开发中（0.1.0）。** 公共接口仍可能调整。已验证离线测试、本地模拟服务，以及限定范围的真实 DeepSeek Flash/Pro 协作调用；自动任务恢复与重放尚未实现。完整范围见[验证与当前边界](#验证与当前边界)。

## 快速上手

### 1. 安装

需要 **Python 3.12+、Rust 1.88+** 及平台 C/C++ 编译工具。默认编译内置 SQLite。当前按源码安装：

```sh
git clone https://github.com/SayorNotDo/model-collaboration-engine.git
cd model-collaboration-engine
python -m venv .venv
```

Windows PowerShell：

```powershell
.venv/Scripts/python.exe -m pip install maturin
.venv/Scripts/python.exe -m maturin develop
```

Linux / macOS：

```sh
.venv/bin/python -m pip install maturin
.venv/bin/python -m maturin develop
```

修改 Rust 或原生绑定后，需要重新执行 `maturin develop`。

### 2. 配置服务和密钥

编辑 [config/engine.json](config/engine.json)。每个服务下面直接填写它提供的模型：

```text
providers[]
├── id、base_url、auth：服务身份、地址与凭证变量
├── endpoints、region、local：协议与数据约束
└── models[]：模型名称、版本、能力、价格与质量先验
```

模型 `id` 在所有服务中唯一，路由和任务通过它选择候选；`model` 是发送给服务端的模型名称。多服务示例见 [services.example.json](config/services.example.json)。示例价格及质量先验不代表实际计费或实测质量，运行前应核实。

**`auth.env` 填环境变量名，密钥放在对应变量中。** 例如：

```json
{"type": "bearer", "env": "DEEPSEEK_API_KEY"}
```

在启动 Python 的同一终端中设置：

```powershell
# Windows PowerShell
$env:DEEPSEEK_API_KEY = "<你的服务密钥>"
```

```sh
# Linux / macOS
export DEEPSEEK_API_KEY="<你的服务密钥>"
```

每个 provider 可以引用不同变量，例如 `DEEPSEEK_API_KEY`、`RELAY_API_KEY`。**引擎不会自动读取 `.env`**；使用该文件时，由宿主启动程序显式载入。终端临时变量不会自动传递给其他已运行的终端或应用。

### 3. 执行第一个任务

将下面代码保存为仓库根目录的 `quickstart.py`：

```python
import asyncio
import json
import time
from pathlib import Path
from uuid import uuid4

from model_collaboration_engine import Engine, load_config


async def main():
    config = load_config("config/engine.json")
    task = json.loads(Path("examples/task.json").read_text(encoding="utf-8"))
    task["task_id"] = str(uuid4())
    task["deadline_ms"] = int(time.time() * 1000) + 60_000
    # 本示例允许请求已配置的远端服务；仅限本地数据时保持 True。
    task["constraints"]["local_only"] = False

    async with await Engine.open(config) as engine:
        result = await engine.run(task)
        print("执行状态:", result["status"])
        print("已结算费用 (microcredits):", result["settled_cost"])
        if result.get("artifact"):
            print(result["artifact"]["text"])


if __name__ == "__main__":
    asyncio.run(main())
```

运行 `.venv/Scripts/python.exe quickstart.py`（Linux/macOS 使用 `.venv/bin/python`）。

成功时会打印执行状态、费用和候选文本；模型输出并不固定。每次任务使用唯一 ID，`deadline_ms` 为绝对 Unix 毫秒时间。预算使用整数 microcredits，所有模型价格与任务预算必须采用同一单位。`completed` 表示执行检查通过，业务正确性仍需宿主验收。

## 选择协作策略

| 策略 | 适用场景 | 执行方式 |
| --- | --- | --- |
| `single` | 一次生成即可完成的任务 | 生成并进行确定性检查；未通过时返回 `human_required` |
| `cascade` | 希望在检查失败时升级模型 | 从未尝试且 Q 至少提高 `min_upgrade_gain` 的候选中继续选择 |
| `generator_critic` | 需要独立评审与修订 | 生成、确定性检查、JSON 评审，在轮数上限内修订 |

模型调用、工具执行和续写共享 `max_calls`；策略不会获得额外预算。级联要求下一候选的 Q 至少提高 `selection.min_upgrade_gain`；没有满足改善、预算和数据约束的候选时保留当前产物并转人工处理。独立评审可通过 `different_critic` 要求不同候选 ID，但不同 ID 不保证底层模型不同。

每个任务必须提供 `selection`：`min_quality` 是候选准入下限，`target_quality` 与 `above_target_factor` 控制达到目标后继续提高质量的边际价值，`cost_reference` 和 `latency_reference_ms` 是偏好尺度，`min_upgrade_gain` 是级联最小改善。预算仍只负责准入；单纯提高预算不会降低成本惩罚或改变已可用候选的价值顺序。完整字段见[任务适配选模](docs/usage.md#任务适配选模)。

需要引擎建议任务类型和策略时，可启用[按需规划](docs/usage.md#按需规划)；建议不能覆盖宿主明确选择、放宽约束或新增工具权限。

### 任务需求评估

schema 3 提交可携带 `task_facts` 和 `minimum_execution_class`。规则只读取宿主提供的结构化事实，不能从 goal 或 evidence 文本推断事实；命中规则、宿主下限和决策策略只能提高最低执行档位，不能放宽权限、预算或候选约束。

Rust 组件可通过 `Engine::with_components_and_decision` 注入供应商无关的 `DecisionModel`。决策调用使用任务共享的 `max_calls` 和预算，并保留规范化概率、模型身份及实际费用证据。当前 Python 公共接口尚未提供 DecisionModel 注入；配置了决策模型但未注入时会在 Engine 构造阶段拒绝。

## 接入与进阶用法

| 接口 | 用途 |
| --- | --- |
| `load_config(path)` / `parse_config(document, base_dir=...)` | 加载并校验只读配置，不联网或打开数据库 |
| `load_rankings(path, manifest_path)` | 导入 JSON/CSV 榜单与显式映射，返回规范榜单配置及导入报告 |
| `await Engine.open(config)` | 打开执行引擎与账本 |
| `await engine.run(task, tools=..., on_event=...)` | 执行任务，返回最终结果 |
| `engine.stream(task, tools=...)` | 通过异步上下文订阅事件并获取结果 |
| `await engine.record_feedback(feedback)` | 对保存的产物提交幂等、版本化的宿主反馈 |
| `await engine.evaluations(task_id)` / `await engine.metrics()` | 查询候选评价与聚合指标 |
| `await engine.recovery_records()` | 查询需关注的任务和调用证据，不重放任务 |
| `await engine.close()` | 停止准入并等待任务及回调清理 |

按需要继续阅读：

- [规划与流式示例](examples/submit.py) · [规划规则](docs/usage.md#按需规划)
- [类型与角色画像](docs/usage.md#类型角色画像) · [反馈与指标](docs/usage.md#验收反馈与持久化指标)
- [外部模型榜单](docs/rankings.md)：JSON/CSV 导入、版本映射、有效期及独立路由权重
- [宿主工具](docs/usage.md#宿主工具) · [流式事件](docs/usage.md#事件订阅)
- [执行与结算](docs/usage.md#执行与持久化约定) · [恢复检查](docs/usage.md#恢复检查) · [优雅关闭](docs/usage.md#优雅关闭)
- [配置字段](config/README.md) · [领域术语](CONTEXTS.md) · [架构及流程图](docs/diagrams/README.md)

## 运行策略评测

[评测脚本](examples/evaluate.py) 对同一组 [12 个合成抽取任务](examples/evaluation-cases.json) 运行 single 与 cascade，使用独立数据库，并在全部调用结束后提交业务反馈。

```powershell
.venv/Scripts/python.exe examples/evaluate.py --config config/engine.json --output evaluation-run-001 --total-budget 2500000
```

需要至少两个非占位模型候选和已配置的实际价格版本。默认预算覆盖一次最小联调及两组任务，共 25 个任务；`--total-budget` 是 microcredits 准入额度，不是货币金额。输出目录必须不存在，可通过 `--suite` 指定同格式任务集。

报告区分执行状态、正确产出、无产物、未执行、费用和任务耗时。费用未知或超额时停止后续派发，取消时保留已有证据。宿主业务评分发生在运行后，不会触发本轮 cascade 升级。详见[评测设计](docs/designs/live-evaluation-design.md)。

需要验收协作路径时使用[复杂协作探针](docs/designs/collaboration-probe.md)，分别检查级联、评审修订、规划和工具。计费明细核对见[离线账务审计](docs/designs/billing-precision.md)；已有总 token 证据不能自动补齐缓存与峰谷明细。

## 验证与当前边界

| 范围 | 已有证据 | 解读边界 |
| --- | --- | --- |
| Rust / Python | [基础实现记录](docs/designs/live-evaluation-implementation.md)及[任务适配路由记录](docs/designs/task-fit-routing-implementation.md)覆盖预算、取消、反馈、配置、持久化和动态升级；新增 Linux/Windows CI 定义 | 工作流须在 GitHub 实际执行后才能宣称 CI 通过 |
| 协议适配 | 两种端点均有本地 HTTP/SSE 模拟测试 | 不能据此宣称所有兼容服务均已验证 |
| 真实服务 | Flash 抽取任务两组各 12/12；[复杂协作四场景](docs/designs/collaboration-live-results.md)验证 Flash→Pro 级联、评审修订、规划与工具续写 | 合成场景不证明自然质量收益；真实 Responses 尚未验证 |
| 费用 | 保留请求 ID、usage、终态与账本证据 | 当前不区分缓存及峰谷价格；真实联调费用为上界估算，未核对供应商扣费 |
| 任务需求评估 | Rust 规则、DecisionModel 契约、执行档位过滤和固定快照已有本地模拟测试 | 尚未接入 Python DecisionModel；尚未进行真实决策模型校准、shadow 评测或自然任务收益评测 |

[真实联调报告](docs/designs/live-evaluation-results.md)保留了首轮数据编码问题、修正后的结果及两轮费用。小样本和未升级的级联结果不能证明策略收益。

当前不提供自动恢复、任务重放或跨任务 exactly-once 保证。未知费用不能视为零；真实调用产生的证据应保留到对账完成。事件增量与流结束也不等于任务成功，最终以终态和账本为准。

### 开发期数据库规则

当前仅接受 SQLite schema 2。版本不匹配时拒绝打开，不自动迁移或清库；备份真实费用证据后再决定是否重建。详细步骤见[数据库规则](docs/usage.md#开发期数据库规则)。配置、任务、画像与数据库版本分别管理。

## 开发与贡献

欢迎通过 [Issues](https://github.com/SayorNotDo/model-collaboration-engine/issues) 报告问题，或提交 Pull Request。问题报告请包含复现步骤、版本、脱敏配置和预期/实际结果；不要附带密钥或未经脱敏的真实数据。

1. 从[代码参考](.agent/code-map.md)定位模块，阅读[仓库协作指南](AGENTS.md)和[代码实现规范](.agent/coding-standards.md)。
2. 为行为变化提供可观察结果、调用次数或账本状态的回归验证。
3. 按[本地验证流程](.agent/README.md)执行 Rust、Python 和文档检查；Cargo 与 maturin 共享构建目录时按顺序运行。
4. 同步受影响示例和文档，通过 PR 合入受保护的 `master`。

[CI 工作流](.github/workflows/ci.yml)执行离线检查，不需要供应商密钥。[发布准备](docs/releasing.md)说明手动构建、产物验证及尚未发布的边界。[指标规模基线](docs/designs/current-status.md#指标规模基线)提供可复现的离线测量方法。

## 许可证

本项目采用 [MIT 许可证](LICENSE)。
