# 配置指南

正式文件入口为 [engine.json](engine.json)，UTF-8 JSON，`schema_version` 必须为 1。这个版本是配置格式版本，与 SQLite schema 2、任务 schema_version 和画像 schema_version 分别管理。

## 加载与生效

```python
from model_collaboration_engine import Engine, load_config

config = load_config("config/engine.json")
# 在宿主 async 函数中：
# async with await Engine.open(config) as engine:
#     result = await engine.run(task)
```

加载同步读取最多 1 MiB JSON，在 Rust 内完成反序列化、引用解析与校验，返回只读 `Configuration`。不访问网络、不读取凭证值、不创建数据库或目录。可读取 `config.model_ids` 和 `config.database_path`，不提供配置修改方法；重新配置需重新加载并打开引擎。

- `load_config(path)`：数据库相对路径以配置文件所在目录为基准，绝对路径保持绝对。
- `parse_config(document, base_dir=".")`：解析宿主构造的分层字典，相对路径以指定 base_dir 为基准。输入字典之后的修改不影响已解析对象。
- `Engine.open(document)`：含 schema_version 的分层字典经同一解析器处理，默认路径基准为当前工作目录。为避免混淆，文件使用 load_config。
- 无默认配置搜索、文件覆盖合并、环境变量插值、热更新或自动 dotenv 加载。错误类型为 configuration，`details.field` 定位字段或配置组；不回显无效字段的值。
- Rust 对应 `configuration::load(path)` / `configuration::parse(json, base_dir)`，得到 `ResolvedConfig`，只读检查使用 as_config，移交 Engine::open 使用 into_config。

原 `contracts::Config` 和 Python 平铺字典保留为低层程序化/组件注入入口，沿用原有字段及路径规则。[旧样例](../examples/config.json) 供现有组件测试使用，不作为新配置文件格式；load_config 不接受它。新旧路径都进入同一执行内核，配置结构变化没有新增执行算法或数据库迁移。

## 供应商接入点与模型

[services.example.json](services.example.json) 展示官方站点的两个模型及中转站的模型别名，所有 URL、模型名和价格都是占位值。

providers 中每项拥有唯一非空 id，kind 只允许 direct/relay。kind 是宿主对接入方式的声明，不触发供应商专属 SDK、鉴权或上游身份验证。base_url 使用 HTTP(S) 且禁止 URL 内凭证、查询串与 fragment；路径保留，由适配器追加 chat/completions 或 responses。

`auth` 只接受 `{ "type": "bearer", "env": "MODEL_API_KEY" }`。env 是环境变量名（字母或下划线开头，其余为字母、数字、下划线），不是密钥。变量在实际调用时读取，发送 Authorization: Bearer；配置加载成功不代表凭证有效。没有无鉴权或自定义请求头模式。

endpoints 为非空且不重复的 chat_completions/responses 数组。模型的 endpoint 必须出现在所引用站点的声明中；协议和能力都是宿主声明，不执行自动探测。region、local 描述实际接入站点，不推断中转的上游数据地域。

每个 provider 下直接填写非空 `models` 数组，模型继承该站点的地址、凭证、地域和本地声明，不再填写 `provider_id`。模型 `id` 在全部站点之间必须唯一，供路由和任务约束引用；`model` 是服务端请求名，允许站点特有别名。同一上游模型经不同站点访问时使用不同候选 ID。version、capabilities、context_tokens、input_price、output_price、price_version、acceptance、reliability、latency_ms、uncertainty 仍属于模型候选，完整字段见示例。

配置阅读顺序如下：先填一个服务的连接信息，再填该服务提供的模型。

```text
providers
├── 官方服务：id、base_url、auth、endpoints、region、local
│   └── models：模型 A、模型 B
└── 中转服务：id、base_url、auth、endpoints、region、local
    └── models：模型 C
```

开发期配置 schema_version 仍为 1，旧的顶层 `models` 和模型 `provider_id` 会被拒绝；将原模型移到对应 provider 的 `models` 中并删除 `provider_id` 即可。该调整不修改 SQLite schema，也不改变低层组件配置。

价格按每 token 的整数 microcredits，所有预算必须使用相同单位。容量、质量先验和价格不自动从供应商读取。模型身份、版本与画像关联保持原有语义；修改渠道或模型行为后由宿主管理候选版本，不能假设不同候选就是不同底层模型。

任务 constraints.allowed_providers 匹配 provider.id（实际服务站），allowed_models 匹配 model.id；接入远端时 local_only 应与站点 local 声明相符。一个站点可以同时服务多个模型、不同协议；地址和凭证只配置一次。

## 路由、运行与存储

routing.weights 使用原评分权重，planner_models 使用模型候选 ID（可省略为空池），profiles 使用原质量画像对象（可省略或 null）。首版不支持画像文件路径字段；需要独立文件时由宿主显式读取：

```python
import json
from pathlib import Path
from model_collaboration_engine import parse_config

base = Path("config")
document = json.loads((base / "engine.json").read_text(encoding="utf-8"))
document["routing"]["profiles"] = json.loads(
    (base / "routing-profiles.json").read_text(encoding="utf-8")
)
config = parse_config(document, base_dir=base)
```

runtime 必须提供并发、缓冲和关闭参数，沿用引擎已有范围约束。storage.database_path 指定文件，拒绝空值和 :memory:；父目录应由宿主准备。数据库版本不匹配时按[开发期规则](../README.md#开发期数据库规则)处理。

加载会拒绝未知字段、未知枚举、重复接入点/模型 ID、悬空引用、端点不匹配和无效资源/画像配置。错误不保证定位到每个语义约束的叶字段；跨字段规则定位到模型或 routing/runtime 组。

## 凭证准备

[.env.example](../.env.example) 只列出变量名。将凭证注入实际运行 Python 的进程环境；在其他终端设置的临时变量不会自动影响当前宿主。普通配置文件与只读对象不保存密钥值，加载不自动验证变量存在。

本次配置功能以本地模拟 HTTP/SSE 验证，不代表占位服务可用或真实供应商已联调。模块关系见[技术图索引](../docs/diagrams/README.md)。
