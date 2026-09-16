# 构建与发布准备

当前版本仍处于开发期，安装入口见[根目录 README](../README.md)。仓库提供持续集成与手动分发包构建；构建工作流只保存 GitHub Actions artifacts，不上传 PyPI、不创建 GitHub Release，也不推送标签。

## 持续集成范围

[CI 工作流](../.github/workflows/ci.yml) 在 `master` 推送、Pull Request 和手动运行时检查 Ubuntu 24.04、Windows Server 2022 的 x86_64 环境，使用 Python 3.12 与 Rust stable。每个任务最多 45 分钟，同一分支的新运行取消旧运行。

每个平台依次执行 Rust 格式、全特性测试、Clippy、Python 包安装、Ruff 与 Python 测试。Rust 与原生扩展构建顺序执行，避免共享构建目录竞争。测试使用本地模拟服务，不提供供应商密钥，不执行真实付费联调。

工作流文件存在不代表远端检查已通过，以目标提交的 Actions 结果为准。当前矩阵不验证 macOS、ARM、全部 Python 版本或 Rust 最低版本；`abi3-py312` 声明不等于每种解释器组合都已实测。

## 手动构建可下载的分发包

1. 确认目标提交的 CI 全部通过，并检查 [Cargo.toml](../Cargo.toml)、[pyproject.toml](../pyproject.toml) 的版本一致、[LICENSE](../LICENSE) 与包元数据一致。
2. 在 GitHub Actions 选择 **Release artifacts (manual)**，选择待验证的分支或标签后运行；记录实际提交 SHA。
3. 等待两个平台任务成功。工作流构建 release wheel，Ubuntu 额外构建 sdist 并从中重建 wheel；检查分发包元数据，再安装本次 wheel 并执行 Python 测试（Ubuntu 测试重建的 wheel）。
4. 下载名称含平台与提交 SHA 的 artifacts（保留 14 天），检查版本、wheel 标签、许可证和源包内容，保存本次日志与结果。

[构建工作流](../.github/workflows/release-artifacts.yml) 的 Linux wheel 使用 `--compatibility off`，是 Ubuntu 构建机对应的原生 wheel，**不承诺 manylinux 或旧 Linux 发行版兼容**。Windows wheel 对应 x86_64。面向公开分发时，需另行确定支持矩阵，并使用合适的 manylinux 构建环境完成安装验证；参见 [maturin 分发文档](https://www.maturin.rs/distribution)。

sdist 重建使用当前任务已安装的 maturin，共享 Cargo 编译缓存。工作流检查关键源文件、锁文件和许可证存在，并拒绝包含 `artifacts/`、`.env` 凭证文件或 `.db` 的源包；`pyproject.toml` 排除真实调用归档目录。公开发布前仍需审查完整包清单。

## 公开发布前的完成条件

- 目标提交的 CI 与手动 artifact 构建全部通过，确认实际支持平台上的安装和导入。
- 版本、变更说明、用户文档、数据库 schema 边界与包内容一致。
- 明确平台兼容范围，完成 sdist 重建及目标 wheel 的干净环境验收。
- 单独确认发布目标、账号权限与发布授权，再配置发布步骤。现有工作流只需仓库只读权限，无发布凭证。

本地检查命令统一见[本地验证流程](../.agent/README.md)。离线测试结果与真实供应商联调结果分别记录。
