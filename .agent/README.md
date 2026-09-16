# 本地验证流程

仓库协作规则见 [AGENTS.md](../AGENTS.md)。以下命令在项目根目录执行，适用于 Windows PowerShell；其他平台使用虚拟环境对应的 Python 路径。

任务分派由 [task-router.md](task-router.md) 选择对应工作流；进入 [验证工作流](workflows/verify.md) 后，按变更范围使用下列命令。代码入口与回归场景见 [code-map.md](code-map.md)。

## Rust 行为变更

Clippy 规则在 `Cargo.toml` 的 `[lints.clippy]` 中维护：默认规则组加同步锁和 RefCell 借用跨等待检查。下列命令将所有警告视为失败；不全量启用 pedantic/restriction，必要例外在最小范围说明原因。

```powershell
cargo test --all-features
cargo clippy --all-features --all-targets -- -D warnings
cargo fmt --check
```

## Python 静态检查

Ruff 的固定版本、Python 目标版本、文件范围和规则统一维护在 `pyproject.toml`。安装检查依赖后，在仓库根目录运行：

```powershell
.venv/Scripts/python.exe -m pip install ".[lint]"
.venv/Scripts/python.exe -m ruff check .
```

检查覆盖 `python/`、`examples/` 与 `tests/python/` 中的 Python 文件，启用 E4/E7/E9、F、I、B，分别检查基本语法与语句问题、未定义或未使用名称、导入顺序及常见易错写法。显式重导出的公共接口和 pytest 共享夹具使用 `name as name` 标注，避免全局忽略未使用导入检查。

行宽目标为 100，当前未启用 E501 或 Python 全量格式门禁；长行仍按代码实现规范审查。Ruff 不替代静态类型检查、取消安全审查或行为测试；当前未配置类型检查器。[CI 工作流](../.github/workflows/ci.yml) 执行 Linux/Windows 检查，范围与手动分发包构建见[发布准备](../docs/releasing.md)。自动修复后检查实际 diff，确认修改仅属本次范围。

## Python 接口与原生扩展变更

首次使用时，按根目录 [README.md](../README.md) 的要求准备 Rust、Python 和 C/C++ 编译工具，并创建测试环境：

```powershell
python -m venv .venv
.venv/Scripts/python.exe -m pip install maturin
.venv/Scripts/python.exe -m pip install -e ".[test]"
```

每次修改影响 Python 的 Rust 代码、绑定或包装器后，先重建扩展，再运行测试，避免加载旧二进制：

```powershell
.venv/Scripts/python.exe -m maturin develop
.venv/Scripts/python.exe -m pytest
```

协议测试优先沿用 `tests/python/test_engine.py` 的本地 HTTP 模拟方式；工具回传、流式事件与回调取消参考 `tests/python/test_tools_stream.py`。测试通过的完成标准是命令退出码为零且相关行为用例通过；缺少工具链或测试依赖时，报告具体阻塞与未执行的检查。

本次 Linux/Rust 1.98.1 环境在增量扩展构建时出现内部符号链接错误，使用 `CARGO_INCREMENTAL=0` 重建后通过；在同一环境复现时沿用该设置。Windows 先用 `$env:CARGO_INCREMENTAL = "0"` 设置环境变量，Linux 可在命令前添加 `CARGO_INCREMENTAL=0`。

共享同一 `target/` 时，Cargo 检查、测试与 maturin 构建按顺序运行。maturin 会切换 PyO3 构建配置，与 Cargo 测试同时运行可能使文档测试引用失效的构建产物。

## 技术图检查

先按当前安装的 archify 技能定位其根目录，将下面的占位路径、图型和文件名替换为实际值。命令成功后保存 JSON 输出作为对应回执；生成结果与截图的视觉检查要求以该技能为准。

```powershell
$archifyRoot = '<当前 archify 技能根目录>'
node "$archifyRoot/bin/archify.mjs" validate architecture docs/diagrams/architecture.json --quality showcase --json
node "$archifyRoot/bin/archify.mjs" deliver architecture docs/diagrams/architecture.json docs/diagrams/architecture.html --quality showcase --json
node "$archifyRoot/bin/archify.mjs" visual-check docs/diagrams/architecture.html --json
```

关闭图使用 `lifecycle` 类型及 `shutdown.json` / `shutdown.html`。仅修改说明且图及源码证据未变时，核对已有回执与文件哈希即可；结构通过不代表视觉通过。详细完成条件见 [技术图工作流](workflows/diagram.md)。

## 文档与提交检查

核对修改涉及的源码事实和 Markdown 相对文件链接。已有文件运行 `git diff --check`；新增文件还需直接检查内容，暂存后可用 `git diff --cached --check` 检查。通过 `git status --short` 和实际 diff 确认交付范围。
