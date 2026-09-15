# 分层供应商配置实施计划

用户已确认接入点、模型、路由、运行时、存储和任务分离。本次在当前工作区连续实现并验证，保留此前未提交文档。

## 配置边界

新文件只接受 schema_version=1，固定 storage/providers/routing/runtime 四组。providers 表达实际接收请求的站点（direct/relay），包含 Bearer 凭证变量、支持端点、地域及本地声明；模型嵌套在所属 provider.models 中，继承连接信息，继续配置模型名、版本、能力、价格及质量先验。allowed_providers 对应站点 ID，不把中转宣称的上游身份用于授权。

文件加载以配置文件目录解析数据库相对路径；内存配置以显式 base_dir 或当前目录解析。无合并、插值、热更新或自动 dotenv。凭证保持调用时读取，加载不联网、不打开数据库。有效配置对象不提供修改接口；底层 Config 保留给 Rust 组件注入及既有 Python 字典调用，所有路径进入同一 Engine。

## 实施顺序与验收

1. 在 tests/configuration.rs 写失败测试：同站多模型与中转别名解析、站点权限映射、相对路径、重复/无效引用、协议不匹配、无效版本和未知字段拒绝。运行 `cargo test --test configuration`，确认缺少 configuration 模块导致失败。
2. 新增 src/configuration.rs（加载及只读有效配置）、src/configuration/document.rs（文件契约与关系校验）。示例调用为 `configuration::load(path)?.into_config()`；文件错误使用 configuration 类别和字段路径，避免回显配置值。复用现有 Config 校验约束，不改路由和结算算法。
3. 新增 src/bindings/configuration.rs，导出 frozen Configuration；Python 增加 load_config(path)、parse_config(dict, base_dir=...)，Engine.open 支持只读对象。现有平铺字典保留为低层程序化入口，不作为文件格式。新增 Python 测试验证读取对象不可变、修改原字典不影响已解析配置、未知字段拒绝、真实本地 HTTP 站点模型映射。
4. 新增 config/engine.json、config/routing-profiles.json 和 .env.example；完整模型字段仍由宿主填写，质量画像首版内联在 routing.profiles，独立画像文件由宿主显式读取后传给 parse_config。示例读取统一入口，纯信息读取不启用规划。
5. 更新 README、CONTEXTS、代码参考与 archify 配置关系图。核对路径、引用、当前实现与拟议评测边界；原图未变部分复核源码依据。
6. 顺序运行 `cargo test --all-features --locked`、Clippy、fmt；重建扩展再运行 pytest。已有规划超时用例并行失败需如实复核，不把单测通过代替全量通过。

## 验收接口

```python
from model_collaboration_engine import Engine, load_config
config = load_config("config/engine.json")
# 在宿主 async 函数中：
# async with await Engine.open(config) as engine:
#     result = await engine.run(task)
```

首版不添加供应商专属协议、动态模型发现、自定义认证头或自动价格抓取。模型版本/画像键保持候选隔离；数据库 schema 不变。完成后报告验证结果，不自动提交或推送当前工作区。

## 嵌套模型配置（2026-09-16）

用户确认以服务为阅读单位：providers[].models 替代顶层 models/provider_id，加载后展开为现有 contracts::Config。全局模型 ID 唯一，空服务模型池、跨服务重复 ID 和旧字段均拒绝；报错路径指向 providers[i].models[j]。配置示例、Python 字典调用及评测准入遍历同步迁移；低层组件接口与数据库不变。
