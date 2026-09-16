# 外部模型榜单

通过本地 JSON/CSV 为执行路由提供可追溯的排序参考。首版不绑定榜单服务，不访问外部 API，也不自动更新文件。数据源变更时保留相同输入格式和显式模型映射即可。

## 1. 准备榜单与映射

榜单 CSV 使用三个表头，或提供同字段的 JSON 对象数组：

```csv
model,version,rank
external-model-a,release-a,1
external-model-b,release-b,2
```

```json
[
  {"model": "external-model-a", "version": "release-a", "rank": 1},
  {"model": "external-model-b", "version": "release-b", "rank": 2}
]
```

文件为 UTF-8，可含 BOM。名次从 1 开始，越小越优，可并列；缺失名次保持未知，不填成 0。不要把不同类别、日期或评测版本拼成一张榜单。榜单无需只包含已配置模型。

映射规格见[合成示例](../examples/rankings/manifest.json)，它包含：

| 字段 | 含义 |
| --- | --- |
| `version` | 本次导入快照的宿主版本 |
| `source`、`source_version`、`category` | 榜单来源、评测方法/数据版本、原始类别 |
| `published_at_ms`、`expires_at_ms` | 生效与到期时刻，Unix 毫秒 |
| `population` | 完整榜单的模型数量，不能换成已配置候选数或最大并列名次 |
| `weight` | 独立榜单参考权重，0..1；0 表示关闭贡献 |
| `task_type`、`roles` | 本地任务类型和适用角色，必须显式选择 |
| `mappings` | 外部模型/版本到本地模型 ID/版本的精确映射 |

每条映射含 `source_model`、`source_model_version`、`model_id`、`model_version`。同一外部模型可以对应不同 provider 下的候选，但必须逐个列出；不自动猜测别名，也不把“最新版本”与某个具体发布版本视作相同。Rust 配置校验会拒绝不存在的本地模型 ID。

## 2. 导入并加载

在项目根目录先用合成数据验证格式：

```powershell
.venv/Scripts/python.exe examples/import_rankings.py --input examples/rankings/models.csv --manifest examples/rankings/manifest.json --output rankings.json
```

输出为可放入配置 `routing.rankings` 的规范 JSON。命令拒绝覆盖已有输出。示例仅演示格式和本地候选映射，不代表真实模型排名；使用自己的榜单时同步修改映射与有效期。

宿主也可以直接加载输入并查看导入报告：

```python
import json
from pathlib import Path
from model_collaboration_engine import load_rankings, parse_config

base = Path("config")
document = json.loads((base / "engine.json").read_text(encoding="utf-8"))
imported = load_rankings("my-model-ranks.csv", "my-rank-manifest.json")
document["routing"]["rankings"] = imported["rankings"]
print(imported["report"])
config = parse_config(document, base_dir=base)
# 将 config 传给 Engine.open；任务设置与映射匹配的 task_type。
```

已有规范 JSON 则直接读取并赋给 `document["routing"]["rankings"]`。配置不支持隐式文件引用或热更新；加载时仍校验整个配置的 1 MiB 上限。原始榜单与映射文件也各自有 1 MiB 上限；只映射实际需要的候选。

## 3. 路由如何使用名次

先执行能力、上下文、候选、provider、地域、预算和质量门槛等硬约束，再计算任务适配主评分。榜单只裁决主评分完全相同的候选：

```text
参考值 = (population - rank) / (population - 1)
榜单参考 = weight × 参考值
排序 = 主评分降序 → 榜单参考降序 → 预估费用升序 → 模型 ID 升序
```

只有一个模型的榜单参考值为 1。参考值是相对排序偏好，**不是业务验收通过率**。例如完整榜单有 101 个模型，第 1 名参考为 weight，第 51 名为 weight/2，第 101 名为 0；未上榜不改变原有质量估计。榜单不能让主评分较低的模型超过主评分较高的模型。

按候选 ID、模型版本、有效角色画像的任务类型和角色精确匹配。不会像质量画像那样自动继承父类型或 general 榜单。若宿主显式配置了角色画像映射，则按有效映射匹配；这不改变工具权限或反馈归因。

未配置、权重为零、尚未生效、已过期、缺失或版本/类型/角色不匹配时，榜单贡献为 0，并记录具体原因。名次与 `QualityProfile` 的 Q 分开；不改变 cascade 的 Q 门槛，不生成业务质量样本，也不更新价格、可靠性和延迟先验。规划阶段继续用宿主规划池和全局先验。

## 4. 查看与复算

任务计划中的 `routing_snapshot` 保存原始榜单配置、摘要和每个候选的解析结果；调用的 `route.ranking` 与 `breakdown.ranking` 保存选中候选的参考证据。当前路由快照版本为 4，数据库表结构仍为 schema 2。schema 1..3 保留当时的直接加分复算语义。

任务开始时固定榜单和有效性判断。更新文件或跨过到期时刻不会改变运行中任务；后续重新加载配置并打开的引擎使用新榜单。已保存快照可通过现有纯路由复算接口核对，不会重放供应商请求。

## 验证范围

导入测试使用合成数据，路由集成使用本地 HTTP/SSE 服务；它们验证解析、映射、约束与快照行为，不证明外部榜单的真实性或自然任务收益。后续接具体来源时，应保留其数据版本、时间、类别和来源说明，另行校准业务效果。

实现边界及取舍见[设计记录](designs/external-ranking-design.md)。
