# 外部模型榜单接入（JSON / CSV）

日期：2026-09-16。用户确认外部榜单作为路由参考，首版通用 JSON/CSV 导入、数据源可替换。

## 决策

Python 在任务外导入本地榜单，显式映射外部模型及版本到本地候选；Rust 验证配置并将参考值固定到任务路由快照。执行期间不联网刷新榜单。此轮不采集外部 API、不修改供应商价格或业务质量样本。

榜单名次不填入质量先验：名次不是验收概率，既有质量 Q 和 cascade 门槛保持不变。task-fit-v1 在硬约束和主评分之后，仅用 `weight * normalized_rank` 裁决主评分完全相同的候选。权重范围 0..1，显式配置；缺失参考为 0，但证据状态必须注明未知/未匹配，不把其质量解释为零。schema 1..3 保留此前直接加分的历史复算语义。

`normalized_rank = (population - rank) / (population - 1)`；population=1 时为 1。population 是完整榜单的规模，由导入规格显式声明，不能用本地候选数量替代。并列名次可具有相同参考值；不同数据源或类别不混合排名。该线性映射只是排序偏好，不是统计置信度或概率。

## Rust 配置契约

配置文件 `routing.rankings`（可省略）；底层 Config 对应 `rankings`。

```json
{
  "schema_version": 1,
  "version": "synthetic-ranking-v1",
  "source": "synthetic fixture",
  "source_version": "benchmark-v1",
  "category": "synthetic reasoning",
  "published_at_ms": 1789516800000,
  "expires_at_ms": 1792108800000,
  "population": 10,
  "weight": 0.2,
  "entries": [{
    "model_id": "local-candidate",
    "model_version": "local-version",
    "source_model": "external-model",
    "source_model_version": "external-version",
    "task_type": "reasoning",
    "role": "invoke",
    "rank": 2
  }]
}
```

版本、来源、榜单类别与身份均非空，source 最多 2048 字节，其余身份字段最多 256 字节；population 为 1..1,000,000，rank 为 1..population，entries 最多 100,000。时间为非负整数 Unix 毫秒且不超过 i64::MAX，expires_at_ms 必须晚于 published_at_ms；权重有限且在 0..1。拒绝未知字段、未知本地候选 ID 与重复候选/版本/类型/角色键。相同外部模型版本的名次必须一致。

只按模型 ID、精确模型版本、任务类型及角色匹配；不自动跨类型继承榜单，不隐式将生成榜单用于 critic；宿主显式角色映射按有效画像匹配。未生效、过期、模型版本不符或无匹配数据时不给贡献，并保留解析状态。规划阶段继续使用既有全局先验，榜单只影响执行节点。

快照保存来源、日期、版本、配置摘要、原始名次、population、解析状态及实际评分贡献；保存之后不再受配置更新或时间变化影响。扩展路由快照版本，不改变 SQLite 表结构及 schema；已有真实账务证据不重建、不迁移。

## Python 导入契约

JSON 为对象数组；CSV 使用表头 `model,version,rank`，UTF-8（可含 BOM）。所有行都验证，重复外部模型版本、重复 CSV 表头、非法名次或超出总体规模均拒绝。

导入规格 JSON 包含上面元数据（不含 entries），增加 `task_type`、`roles` 和 `mappings`。映射项使用 `source_model`、`source_model_version`、`model_id`、`model_version`；不通过模糊名称猜测版本。同一外部模型可映射到多个服务上的显式候选。源榜单无需每行都有本地映射，但规格中的映射必须找到源行；源行 rank 缺失不赋零。

导入生成可写入 `routing.rankings` 的规范对象，并报告忽略/缺失名次。示例 CLI 写入新的 JSON 文件，不修改原配置、数据库或原始榜单。公共 Python 接口独立于榜单服务 SDK；运行加载通过现有 `parse_config` 校验。

## 实施与验收

- [x] Rust 排名契约、配置解析、固定快照与 task-fit-v1 平分裁决；测试硬约束、质量门槛、版本/类别不匹配、过期、缺失及新旧算法复算。
- [x] Python JSON/CSV 导入、显式映射、导入报告和示例；先失败测试再实现，覆盖畸形输入、未知值及重复行。
- [x] README、配置指南、领域说明、架构/反馈图及源码证据同步；图形结构、浏览器与视觉分别检查。
- [x] Rust 全量/Clippy/fmt、重建原生扩展、Python 全量/Ruff；真实供应商调用不是本轮所需验收。

本轮工作在既有未提交变更基础上增量进行，不覆盖上一轮联调与工程准备成果。

实际检查与未验证项见[实施验收](external-ranking-implementation.md)。
