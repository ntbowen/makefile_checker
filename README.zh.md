# makefile_checker

> **[English Documentation](README.md)**

OpenWrt Makefile 上游版本检查工具 —— 基于 Rust 的交互式命令行程序。

批量检查 OpenWrt feed/软件树下的所有软件包，与上游源对比版本，报告哪些包已过期，并可选地验证 `PKG_HASH` 完整性。

---

## 功能特性

- **递归扫描** — 自动遍历目录树，找出所有 `Makefile`
- **完整变量展开** — 解析 URL 前先展开 `$(PKG_VERSION)`、`$(PKG_NAME)` 等变量
- **14 种上游后端** — 从 `PKG_SOURCE_URL` 自动识别：

| 后端 | 识别依据 |
| --- | --- |
| GitHub Release / Tag / Commit | `codeload.github.com`、`github.com` |
| GitLab（官方 & 自托管） | `gitlab.com`、`gitlab.*` |
| BitBucket | `bitbucket.org` |
| Gitea / Forgejo / Codeberg | 任意含 `/archive/` 路径的主机 |
| SourceForge | `downloads.sourceforge.net` |
| PyPI | `files.pythonhosted.org`、`pypi.org` |
| crates.io | `static.crates.io` |
| npm | `registry.npmjs.org` |
| RubyGems | `rubygems.org` |
| Hackage（Haskell） | `hackage.haskell.org` |
| CPAN（Perl） | `cpan.org`、`metacpan.org` |
| kernel.org | `www.kernel.org`、`cdn.kernel.org` |
| cgit / gitweb | `git.kernel.org`、任意 `.git/snapshot` |
| Go module proxy | `proxy.golang.org` |
| URL 正则（自定义） | 通过 per-package 规则覆盖 |
| Repology + Anitya 兜底 | 所有无法识别的来源 |

- **PKG_HASH 验证** — 下载源码压缩包，将 SHA-256 与 `PKG_HASH` 对比，不匹配时以红色警告标注
- **重试 + 指数退避** — 对 HTTP 瞬时错误 / 速率限制自动重试，次数可配置
- **Per-package 规则** — 按包名忽略特定版本、限制版本范围、去除 tag 前缀/后缀、允许预发布版本、或用 URL 正则完全覆盖后端
- **跳过指定包** — 将特定包名排除在检查之外
- **多 URL 支持** — 逐一尝试 `PKG_SOURCE_URL` 中的每个 URL，直到识别出后端
- **快照差量** — 与上次运行结果对比，只展示发生变化的条目
- **交互式 TUI** — 无需手动编辑文件即可配置所有选项，配置跨会话持久化
- **彩色 XLSX 导出** — 过期行红色、最新行绿色、哈希不匹配高亮标注
- **CSV 导出** — 供 CI 流水线使用的机器可读格式
- **中英双语界面** — 运行时可切换

---

## 编译

```bash
cargo build --release
# 产物：target/release/makefile_checker
```

需要 Rust 1.75+（使用了 `LazyLock`）。

---

## 使用

```bash
./makefile_checker
```

首次运行时会引导配置扫描路径、并发数等参数，配置保存至 `~/.config/makefile_checker/config.toml`，后续自动加载。

### GitHub Token（推荐）

不使用 token 时 GitHub API 限制为 **60 次请求/小时**；使用 token 后提升至 **5000 次/小时**。

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
./makefile_checker
```

Token 也可在交互式配置菜单中设置，或直接写入配置文件。

---

## 配置文件

位置：`~/.config/makefile_checker/config.toml`

```toml
search_paths  = ["/home/user/openwrt/feeds/packages"]
parallel_jobs = 16
timeout_secs  = 20
retry_times   = 3
output_path   = "."
output_format = "xlsx"          # xlsx | csv | both | none
github_token  = "ghp_..."
skip_patterns = ["host/", "toolchain/"]
skip_packages = ["linux", "llvm-bpf"]
lang          = "zh"            # en | zh

# Per-package 覆盖规则
[pkg_rules.openssl]
ignore_regex = ["^1\\..*"]     # 忽略 1.x 分支
min_version  = "3.0.0"

[pkg_rules.nginx]
strip_prefix       = "release-"
include_prerelease = false

[pkg_rules.some-obscure-pkg]
url_regex_url     = "https://example.com/downloads/"
url_regex_pattern = 'href="v?([0-9]+\.[0-9]+\.[0-9]+)\.tar'
```

### `pkg_rules` 字段说明

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `ignore_regex` | `[String]` | 忽略匹配任意正则的版本号 |
| `min_version` | String | 只接受 ≥ 此版本的结果 |
| `max_version` | String | 只接受 ≤ 此版本的结果 |
| `strip_prefix` | String | 比较前去除 tag 前缀（如 `"release-"`） |
| `strip_suffix` | String | 比较前去除 tag 后缀（如 `"-stable"`） |
| `include_prerelease` | bool | 是否包含 alpha/beta/rc 预发布版本（默认 `false`） |
| `url_regex_url` | String | 抓取该 URL，用 `url_regex_pattern` 提取版本号 |
| `url_regex_pattern` | String | 含捕获组 1 的正则，用于版本提取 |

---

## 输出示例

### 终端表格

```text
软件包           当前版本   最新版本   状态      Tag/Commit   后端     哈希
──────────────────────────────────────────────────────────────────────────
tailscale        1.94.2     1.96.4     过期      v1.96.4      github   ✓
openssl          3.3.0      3.3.2      过期                   github   ✗ 不匹配
curl             8.7.1      8.7.1      最新                   github   ✓
```

底部汇总行示例：

```text
已检查 342 个包  23 个过期  315 个最新  4 个未知  2 个 PKG_HASH 不匹配！
```

### XLSX / CSV

导出到配置的 `output_path`。XLSX 列顺序：

`PKG_NAME` · `目录` · `当前版本` · `最新版本` · `状态` · `Tag/Commit` · `后端` · `哈希状态` · `Commit SHA` · `上游 URL` · `备注` · `路径`

---

## 项目结构

```text
src/
├── main.rs            # 程序入口
├── config.rs          # Config + PkgRule 结构体，TOML 读写
├── makefile_parser.rs # 解析 Makefile 变量，识别 SourceType
├── upstream.rs        # 所有后端检查函数、apply_rule、verify_hash
├── reporter.rs        # 终端表格、print_summary、XLSX/CSV 导出
├── interactive.rs     # TUI 菜单（dialoguer），run_check 编排
├── snapshot.rs        # 与上次运行结果的差量对比
└── i18n.rs            # 中英双语字符串常量
```

---

## 单元测试

```bash
cargo test
```

共 35 个单元测试，覆盖：全部 14 种后端 URL 模式、多 URL 回退逻辑、变量展开、版本比较、预发布版本识别，以及 `apply_rule` 的所有分支行为。
