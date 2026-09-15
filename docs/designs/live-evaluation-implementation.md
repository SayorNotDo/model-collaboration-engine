# 策略评测实施计划

目标：按已确认的 live-evaluation-design 实现独立示例，先模拟验收，再由宿主提供实际配置与预算开展真实联调。

架构：examples/evaluate.py 负责命令行；examples/evaluation/inputs.py 负责准入与任务构造；runner.py 持有引擎和运行生命周期；reporting.py 负责版本化业务验收与报告。所有账务操作复用 Engine，费用汇总复用 compare_metrics.summarize。

技术：Python 3.12、现有原生扩展、pytest、本地 HTTP/SSE 模拟。

- [x] 在 test_usage_evidence.py 验证两种端点收到 usage 后实际超时，断言调用一次、费用 15、预留零、timed_out。
- [x] 新增 test_evaluation.py，先验证入口缺失失败，再覆盖相同输入配对、延后反馈、类型严格验收、未知费用停止、额度预拒绝、不覆盖及取消保留。
- [x] inputs.py 校验 schema 1 配置、凭证变量、模型版本与价格版本、固定任务集和所有任务预算之和；构造明确类型、关闭规划、无工具的提交。
- [x] runner.py 新建独占输出目录和三个数据库（联调、single、cascade）；逐任务交替顺序，保存每次输出，取消后继续等待清理并保留结果。
- [x] reporting.py 从 evaluations 选择最终保存候选，所有调用结束后写反馈；导出分组 metrics、任务状态、未执行原因和端到端耗时，保留验收版本。
- [x] 提供 12 个合成任务与 CLI；同步 README、设计状态、代码入口与评测技术图。
- [x] 执行 Python 全量测试、Ruff、文档引用和空白检查；图的结构、浏览器、视觉分别验收。未修改 Rust 时复用本轮先前的原生构建。

真实调用不属于模拟验收：需要宿主提供实际模型配置与 microcredits 总准入额度。此次不提交或推送工作区。

## 验收结果

2026-09-16：新增超时回归 2 项和评测回归 12 项通过；Python 全量 96 项通过（63.12 秒），Ruff 与 CLI --help 通过。此次无 Rust 生产代码变更，沿用前轮已重建的扩展。评测图结构 9/9、浏览器通过，已查看 1440×900 深色及 2048×1320 浅色截图，视觉通过；哈希与源码映射记录在图目录。真实供应商联调未执行。
