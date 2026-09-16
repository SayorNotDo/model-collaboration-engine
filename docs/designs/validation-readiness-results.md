# 协作联调与工程准备验收记录

日期：2026-09-16。分支：`codex/collaboration-validation`，基于 `0d2e339`。此记录描述本轮实际检查，不代表 GitHub CI 已执行或公开发布。

## 本地检查

- `cargo test --locked --all-features`：69 项通过。
- `cargo clippy --locked --all-features --all-targets -- -D warnings`：通过。
- `cargo fmt --check`：通过。
- 最终 `python -m pytest -q`：147 项通过（56.40 秒）；`python -m ruff check .` 通过。探针其中 18 项覆盖路径证据、调用预算、存储失败和重复取消收尾。
- 计费审计与指标基准的独立测试：33 项通过；分别检查 malformed/缺失证据、精确小数、聚合计数费用和临时库清理。
- Windows release wheel 构建、Twine 元数据检查通过；独立 Python 3.13 环境当时的 111 项测试通过。
- sdist 初次验收 155 个条目，从中重建 wheel、安装与导入通过；补齐探针后重新打包为 160 个条目并复核必需源码、锁文件、README、LICENSE 及三个新示例均在，`artifacts/` 已排除。
- actionlint 1.7.12 检查两个工作流通过。Linux 构建、Python 3.12 矩阵和 GitHub Actions 的实际执行尚未验证。
- 修改文档的本地文件链接、`git diff --check`、新增文本空白检查通过；待交付文件未发现 `.env` 中的凭证值。

## 独立证据与限制

[复杂协作真实联调](collaboration-live-results.md)：四个场景路径和最终答案均通过，10 次模型调用、1 次只读工具执行，账本合计 60,612 microcredits 配置价上界估算，未知费用和未结算预留为零。与离线测试分开核验。

[指标基线](../../artifacts/metrics-baseline-20260916.json)只覆盖一个模型分组、无质量反馈的合成数据。不是生产 SLA。

[费用审计](../../artifacts/billing-audit-20260916.json)只复核配置价估算和缺失证据，不修改已有账本；供应商实扣金额未知。

本轮没有修改 Rust 内核、数据库 schema 或已有技术图的职责关系，因此没有重新生成技术图。关闭图既有视觉问题仍保留在[图形验收索引](../diagrams/README.md)。
