# Jev 与“Jev-like”决策模型调研

> 调研日期：2026-09-20
> 结论依据：优先采用 TypeSafe AI 官方文档、官方发布文章、官方代码仓库和官方评测；第三方资料只用于确认公开信息是否存在，不作为产品能力依据。

## 结论

**Jev** 是 TypeSafe AI 于 2026-09-15 发布、当时处于 early access 的专有决策模型。TypeSafe 将其称为首个 **System One Model**：输入一份文本或结构化 `state`，以及预先定义的若干封闭问题；模型一次返回类型固定的选择、评分或布尔概率，而不生成自由文本。[官方发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)称该产品由 TypeSafe 创始人 Diogo Almeida 发布，名称取自 William Stanley Jevons。

**“Jev-like”不是官方模型名、标准或已建立的学术类别。** 在本项目中，这个词最多只能表示一种受 Jev 启发的接口和职责：低延迟地回答封闭、原子、语义性的判断问题，并输出概率；代码继续掌握约束、计算、权限和副作用。若设计要求指向 TypeSafe 的具体产品，应直接称为 **Jev** 或 **TypeSafe System One API**；若供应商可替换，建议称为 **typed probabilistic decision model（类型化概率决策模型）** 或 **System-One-style decision model**，并明确这是项目自己的抽象。

Jev 的形状适合本项目中“规则无法可靠覆盖的模糊语义信号”，例如任务种类、需求是否含糊、是否需要多步推理、是否需要澄清。它不适合作为唯一的复杂度判定器，也不应直接决定模型、预算、工具权限或执行动作。文件数、跨服务范围、migration、预算和权限等确定性事实应由代码计算；最终路由应由本地策略把规则结果、Jev 概率和风险阈值组合起来。

## 已确认的产品事实

### 提出者、名称与定位

- Jev 由 **TypeSafe AI** 发布；发布文章署名为创始人 **Diogo Almeida**。官方称其为 TypeSafe 的首个公开 System One model，并将 System One 的命名与 Daniel Kahneman 的快思考概念联系起来。[发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)
- “Jev”取名自经济学家 **William Stanley Jevons**，对应 Jevons paradox 所表达的效率提高可能扩大需求。[发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)
- 官方将 Jev 定位为软件内的决策原语，而非聊天或内容生成模型；其核心契约是“unstructured state in, typed probabilistic decisions out”。[发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)、[官方介绍](https://docs.typesafe.ai/introduction)

### 输入、问题与输出

API 入口为 `POST https://api.typesafe.ai/v1/systemone`。请求包含：

- `state`：字符串、JSON 对象或数组；
- `model`：例如 `jev-latest` 或固定版本 `jev-1.13.0`；
- `questions`：由调用方命名的问题映射。

官方定义三种问题类型：[API reference](https://docs.typesafe.ai/api)、[Primitives](https://docs.typesafe.ai/primitives)

| 类型 | 用途 | 返回值 |
| --- | --- | --- |
| `Choice` | 从调用方给出的封闭选项中选择 | 最高概率选项、所有选项的概率分布、`confidence` |
| `Score` | 按 2–10 个有序语义等级评分 | 概率加权分数、等级概率分布、`confidence` |
| `Noul` | 判断一个是/否命题 | “是”的概率，范围为 0–1；没有单独的 `confidence` 字段 |

一次请求可以混合多种问题。官方文档称各问题共享同一份 `state`，彼此独立并行评估；问题 ID 只用于关联请求和响应，不参与模型推理。[官方介绍](https://docs.typesafe.ai/introduction)、[API reference](https://docs.typesafe.ai/api)

这里的 `confidence` 和选中项的概率不是同一概念。对于 `Choice` 和 `Score`，`confidence` 是从完整概率分布的集中程度推导出的统计量；分布越平坦，置信度越低。`Noul` 直接返回命题为真的概率。[Confidence](https://docs.typesafe.ai/confidence)

### 当前公开版本与运行边界

截至调研日，官方模型页列出：[Models](https://docs.typesafe.ai/models)

- 稳定版本：`jev-1.13.0`；`jev-latest` 当时指向该版本；
- 输入：纯文本语义，`state` 可用字符串或 JSON 组织，不支持原生图像、音频或视频；
- 上下文：每请求总计 64k tokens，同时要求 `state` 加最长单个问题不超过 32k tokens；
- `Choice` 最多 255 个选项；`Score` 最多 10 个等级；
- 标价：输入 `$0.042 / MTok`，输出不计费；
- 官方公布的默认限流当时为 250,000 tokens/s 和 1,200 requests/min，并明确称限流可能动态变化；
- 英语是主要训练语言，CJK 等其他语言可处理但准确度不等同，官方要求在目标数据上自行验证；
- 模型别名会随版本移动。若阈值是针对某版本校准的，应固定版本并记录响应中的实际模型 ID。

这些是 2026-09-20 的产品状态，不应固化为本项目长期不变的事实。

## 训练、架构和推理成本：公开到什么程度

### 已确认

TypeSafe 将训练方法命名为 **Reinforcement Learning for Calibrated Decisions（RLCD）**，声称其目标是让模型输出决策及校准概率：在足够多的样本上，给出约 0.8 概率的一组预测应约有 80% 正确。校准是群体统计性质，不保证任一单次 0.8 预测正确。[AI primer](https://docs.typesafe.ai/introduction/machine-learning-primer)

官方还声称 Jev 使用“new model architecture”和并行 sampler，所有结果并行产生，而非自回归地逐 token 生成文本。[发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)

### 无法确认

截至调研日，没有找到 TypeSafe 发布的 Jev 技术论文、模型卡式技术报告、权重、训练代码、训练数据说明或可复现的 RLCD 算法。以下信息均未公开，不能由“System One”或“并行 sampler”反推出答案：

- backbone 类型、是否为 Transformer encoder/diffusion/其他结构；
- 参数量、层数、embedding 和 decision head 结构；
- RLCD 的 reward、loss、采样策略、校准方法和训练阶段；
- 训练集来源、规模、标签流程、污染控制和分布覆盖；
- 推理硬件、batching、量化和服务拓扑；
- 概率在跨领域、跨语言和分布漂移后的校准稳定性。

因此，不能把社区对“BERT-like encoder”或“非自回归单次 forward pass”的猜测写成 Jev 的实现事实。“Jev-like 模型”若要在本项目中自行实现，也不能以公开资料为基础复现 Jev，只能实现相同或相似的外部契约。

### 成本与延迟证据的边界

官方发布文章给出的 TypeSafe 端到端延迟区间为 **70–500 ms**，并称在其 workflow eval 上最高达到 **193.6× 更快、444.6× 更便宜**。[发布文章](https://typesafe.ai/blog/introducing-system-one-models-and-jev)

这些数字只能视为**供应商测量**：

- 官方承认延迟通常从美国西海岸的员工笔记本测量；
- “193.6× / 444.6×”被官方称为现实收益的高端值；
- workflow 由 TypeSafe 模型能力团队成员制作，官方承认可能存在偏差；
- 参考答案不是独立人工真值，而是 GPT-6 Astra 与 Claude Fable 5.1 高推理设置的平均结果；
- 比较模型通过 TypeSafe 自己的 System One LLM wrapper 适配为相同接口。

[官方 workflow eval](https://evals.typesafe.ai/)公开了四种工作流的查询和比较方式，因而可用于理解产品形态；它仍不是独立评测，也不能替代本项目流量上的准确率、选择性风险、延迟和成本测量。

## 官方已披露的限制

`jev-1.13` 的官方 jaggedness 文档给出了对本项目非常关键的失败模式：[Jev 1.13 jaggedness](https://docs.typesafe.ai/model-jaggedness/jev-1.13)

1. **字面理解。** 隐含条件、否定和边界容易被按字面解释，问题及每个选项需要写清精确定义。
2. **数字和计算较弱。** 不应让模型计数、做算术、比较日期或重建精确数值；这些工作应在代码中完成。
3. **多跳和间接推理较弱。** 属性的属性、复杂间接关系和多层推理会降低准确率。
4. **无关上下文导致 context rot。** 状态越长且无关内容越多，准确率越差；应先由代码检索和过滤。
5. **对抗性内容可改变判断。** Jev 1.13 默认不会把 `state` 当作不可信数据，prompt injection 或带倾向性的文本可能操纵结果。
6. **问题之间没有逻辑恒等保证。** 同一语义换成 `Choice`、`Noul` 或否定问法时，概率不必满足算术一致性；阈值也不能跨问题类型直接复用。
7. **不能生成文本。** 封闭选项之外的提取应先由代码或生成模型产生候选，再由 Jev 选择。
8. **类型正确不等于判断正确。** 固定输出空间可以保证不会返回 schema 外的字符串，却不能保证所选类别、概率或风险判断正确。官方“can't hallucinate/zero hallucinations”的说法应严格解释为输出类型和候选空间受到约束，而不是事实错误率为零。

## 是否适合任务分类与策略决策

### 适合承担的职责

在经过本地评测后，Jev 可以作为一个**受限语义传感器**，并行产生下列信号：

- `task_kind`：从项目定义的任务类型中选择；
- `ambiguity`：按明确、部分含糊、关键缺失等语义等级评分；
- `reasoning_need`：判断任务是否需要跨证据、多步设计或复杂权衡；
- `needs_clarification`：判断当前输入是否缺少继续执行所需的信息；
- `risk_signal`：对无法通过硬规则识别的语义风险给出概率。

这与 TypeSafe 官方的 intent routing 模式一致：模型进行快速分类，代码根据答案和置信阈值选择确定性处理器、专业 LLM 或人工路径。[Intent routing](https://docs.typesafe.ai/patterns/intent-routing)

Jev 也可在运行后对**单一、明确、可从现有证据直接判断的属性**做辅助检查，例如“结果是否回答了用户请求”或“失败是否表现为缺少上下文”。它不能替代编译、测试、账本检查和其他确定性验收，也不宜单独对复杂产物做总体 `accept/retry/escalate` 判决。

### 不适合承担的职责

- 计算文件数、token 数、费用、截止时间、重试次数或 migration 是否存在；
- 从完整仓库和长历史中自动找出所有相关上下文；
- 执行复杂规划、数学、日期比较或多跳因果分析；
- 发放工具权限、扩大预算、解除地域/供应商/能力约束；
- 直接触发有副作用的工具；
- 作为高风险工作流唯一的安全分类器或最终验收者；
- 在没有本地标签和校准评测的情况下，把供应商 `confidence` 当作可移植的正确率保证。

### 对本项目架构的建议

建议将“Jev-like”落实为**协议与职责**，而不是绑定到一个神秘模型架构：

```text
TaskFacts（代码提取的确定事实）
  + 精简后的语义状态
  + 一组原子、封闭、版本化的问题
        ↓
DecisionModelAdapter（Jev 或兼容实现）
        ↓
TypedDecisionSignals（值、概率分布、置信度、模型版本）
        ↓
Deterministic Policy（硬约束、风险阈值、预算和候选资格）
        ↓
执行档位 / 澄清 / 保守回退
```

具体规则：

1. **先规则、后语义。** 代码先提取文件数、服务边界、migration、工具和预算等事实；能确定的路径不调用模型。
2. **问题原子化。** 不向模型询问“这个任务总体难不难”；分别询问任务类型、含糊度、推理需求和风险，再由代码组合。
3. **模型只提议信号。** 模型不接收供应商价格表，也不直接返回供应商模型 ID；策略层根据约束与信号选择执行等级。
4. **上下文最小化。** 只发送完成这些判断所需的提交内容与结构化事实，不把完整历史、仓库或工具输出直接塞入 `state`。
5. **不可信输入隔离。** 用户文本和仓库内容标记为数据；敏感硬规则在模型调用前执行，模型输出后再次校验。
6. **置信度门控和保守失败。** 缺失概率、低置信度、超时、限流、未知选项或版本漂移时，进入项目定义的保守档位或澄清路径，不让模型失败扩大权限。
7. **固定版本与证据。** 记录问题集版本、模型实际版本、输入摘要、全部概率、阈值、最终策略覆盖和后续真实结果。
8. **用真实结果校准。** 按任务类型和问题分别测量 accuracy、coverage、selective risk、ECE/Brier score、升级率、重试成本和最终成功率；阈值来自本项目数据，不抄官方示例的 `0.5` 或 `0.8`。

## 最小验证计划

在把 Jev 纳入路由前，应先进行 shadow evaluation，不让其输出影响真实执行：

1. 从历史任务构造冻结数据集，避免同一任务及近重复样本跨 train/tune/test 泄漏。
2. 由独立证据标注每个原子信号，以及最终实际需要的执行档位；不要用当前 router 的决定直接当真值。
3. 同时比较规则基线、小型生成式 LLM 结构化输出、Jev 和“规则 + Jev”的组合。
4. 分别报告简单任务的降级误路由率、困难任务的漏升级率、校准曲线、选择性风险、p50/p95 延迟和包含重试后的总成本。
5. 对长上下文、中文、否定、跨文件间接关系、prompt injection、未知类别和供应商不可用进行切片测试。
6. 仅当组合方案在固定质量约束下减少了成本或延迟，且困难任务漏升级率满足上限，才启用有限流量；运行时继续保留结果监控与升级机制。

## 事实分级

| 判断 | 分级 | 说明 |
| --- | --- | --- |
| Jev 是 TypeSafe 的专有 System One 决策模型 | 已确认 | 官方发布与文档一致 |
| 接口为 `state + typed questions → typed probabilistic answers` | 已确认 | API、SDK 与官方代码仓库可核对 |
| 输出采用 Choice、Score、Noul 三种原语 | 已确认 | 官方 API 契约 |
| Jev 1.13 的价格、上下文和限流 | 已确认但易变 | 仅代表调研日状态 |
| 使用 RLCD、新架构和并行 sampler | 供应商声明 | 官方声明存在，但实现未公开，无法独立复现 |
| 概率在任意本项目任务上都已校准 | 无法确认 | 必须在本项目分布上验证 |
| Jev 是 BERT-like encoder 或单次 forward-pass classifier | 无法确认 | 官方未披露，社区推测不能作为事实 |
| Jev 能取代复杂度规则或 runtime monitor | 不成立 | 官方限制和本项目约束都要求代码掌握确定性逻辑与回退 |
| Jev 可作为模糊语义信号源，辅助模型路由 | 合理推断 | 契合官方 intent-routing 模式，但收益须由本地评测证明 |

## 主要一手资料

- [TypeSafe：Introducing System One Models & Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev)
- [TypeSafe docs：Introduction](https://docs.typesafe.ai/introduction)
- [TypeSafe docs：API reference](https://docs.typesafe.ai/api)
- [TypeSafe docs：Models](https://docs.typesafe.ai/models)
- [TypeSafe docs：AI primer / RLCD](https://docs.typesafe.ai/introduction/machine-learning-primer)
- [TypeSafe docs：Confidence](https://docs.typesafe.ai/confidence)
- [TypeSafe docs：Jev 1.13 jaggedness](https://docs.typesafe.ai/model-jaggedness/jev-1.13)
- [TypeSafe docs：How to build with System One](https://docs.typesafe.ai/concepts/how-to-build-with-system-one)
- [TypeSafe docs：Intent routing](https://docs.typesafe.ai/patterns/intent-routing)
- [TypeSafe workflow evals](https://evals.typesafe.ai/)
- [TypeSafe 官方 Python SDK](https://github.com/typesafe-ai/typesafe-sdk-python)
- [TypeSafe 官方 JavaScript SDK](https://github.com/typesafe-ai/typesafe-sdk-js)
- [TypeSafe 官方 System One LLM adapter](https://github.com/typesafe-ai/system-one-adapter-python)
