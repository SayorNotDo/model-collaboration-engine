# 技术图索引

本次使用 archify 2.17 更新工作区技术资料，代码基线为 `0d2e339` 及工作区变更。2026-09-17 同步架构与反馈图中的任务价值评分、严格质量升级、离线榜单平分参考及固定快照语义。图形验收只针对图产物，不代表代码测试或真实供应商联调通过。

| 技术图 | 范围与状态 | 验收 |
| --- | --- | --- |
| [架构总览](architecture.html) · architecture | 当前实现：统一入口、规划校验、任务价值路由、动态升级、工具、事件和账本 | 结构 9/9、浏览器通过、视觉通过 |
| [反馈与后续路由](feedback-routing.html) · architecture | 当前实现：宿主反馈、持久化指标、离线榜单平分参考、新任务固定路由快照与严格质量升级 | 结构 9/9、浏览器通过、视觉通过 |
| [关闭与重试](shutdown.html) · lifecycle | 当前实现：停止准入、排空、清理超时和释放数据库 | 结构 9/9、浏览器通过；视觉未通过，布局仍偏左 |
| [策略对比评测](evaluation-plan.html) · architecture | 当前实现：固定任务集、两组独立数据库、延后反馈与对比报告 | 结构 9/9、浏览器通过、视觉通过；限定范围真实结果见[联调记录](../designs/live-evaluation-results.md) |

## 阅读边界

架构连线表示模块协作，不代表每次调用的严格时序。规划、执行、工具和续写共用资源限制；宿主保有工具权限。反馈图按数据依赖展开，指标是 SQLite 的一致性查询，不是独立数据库或后台服务；质量回退顺序为精确类型/角色、父类型链、general、全局先验。新反馈不刷新运行中任务的快照。新任务固定质量 Q、选择偏好、算法版本与榜单证据；成本和延迟偏好尺度独立于预算准入。外部榜单由 Python 在任务外导入 JSON/CSV 并显式映射模型版本，只在任务价值主评分相同时辅助排序，不改变质量 Q 或级联门槛。规划阶段继续使用全局先验，执行期不联网刷新榜单。

关闭图将宽限等待、取消和清理合并为等待排空。规划也属于活动任务；费用未知不等于任务仍在执行。数据库释放和不配合的宿主回调没有硬性总时限。结果泳道已补齐，原空 Outcomes 泳道问题已消除；压缩宽度的候选造成浏览器溢出，已恢复无溢出版本，版面偏左仍待改进。

评测图对应[已确认设计](../designs/live-evaluation-design.md)和独立 Python 示例，已提供合成任务集与逐任务报告。图中展示两组配对流程；前置联调使用额外 smoke 数据库，费用单列。现有 Engine、反馈和指标接口可复用；开始真实调用前仍需模型配置和费用额度。草案记录的规划超时测试已拆为派发后与派发前场景，并通过 Rust 全量回归。图中的参考答案供宿主验收，不发给模型。

## 编辑源与检查证据

每张图均有同名 JSON 编辑源、HTML 产物、`.delivery.json` 交付回执、`.visual-check.json` 浏览器回执和 `.visual-check.html` 截图索引：

- 架构：[JSON](architecture.json) · [交付](architecture.delivery.json) · [浏览器](architecture.visual-check.json) · [截图](architecture.visual-check.html)
- 反馈：[JSON](feedback-routing.json) · [交付](feedback-routing.delivery.json) · [浏览器](feedback-routing.visual-check.json) · [截图](feedback-routing.visual-check.html)
- 关闭：[JSON](shutdown.json) · [交付](shutdown.delivery.json) · [浏览器](shutdown.visual-check.json) · [截图](shutdown.visual-check.html)
- 评测：[JSON](evaluation-plan.json) · [交付](evaluation-plan.delivery.json) · [浏览器](evaluation-plan.visual-check.json) · [截图](evaluation-plan.visual-check.html)

[源码与设计映射](source-map.json) 区分当前实现和待实现节点，保存文件 SHA-256；[视觉审查](review.json) 绑定最终规格、HTML 和实际查看的截图。浏览器检查覆盖 1440×900、1600×1000、1920×1080、2048×1320，明暗截图覆盖最小和最大尺寸。自动回执中的 visualReview=pending 由独立审查记录补充，不修改自动回执。

此前“archify 不可用、HTML 待生成”的记录已由本次交付取代；历史图形及验收状态可查 Git 历史。结构、浏览器和视觉结论分别记录，任一通过不替代其他检查。

## 维护入口

遵循[技术图工作流](../../.agent/workflows/diagram.md)，命令见[本地验证流程](../../.agent/README.md#技术图检查)。只修改 JSON，通过生成器更新 HTML；图源变化后重新收集验收证据。领域术语见 [CONTEXTS.md](../../CONTEXTS.md)，开发期数据库操作以 [README](../../README.md#开发期数据库规则) 为准。
