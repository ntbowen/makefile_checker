# makefile_checker

> **[中文文档](README.zh.md)**

OpenWrt Makefile upstream version checker — interactive CLI built with Rust.

Batch-checks all packages under an OpenWrt feed/tree against their upstream sources and reports which ones are outdated, along with optional `PKG_HASH` integrity verification.

---

## Features

- **Recursive scan** — walks directory trees and finds every `Makefile` automatically
- **Full variable substitution** — expands `$(PKG_VERSION)`, `$(PKG_NAME)`, etc. before parsing URLs
- **14 upstream backends** — auto-detected from `PKG_SOURCE_URL`:

| Backend | Detected from |
| --- | --- |
| GitHub Release / Tag / Commit | `codeload.github.com`, `github.com` |
| GitLab (hosted & self-hosted) | `gitlab.com`, `gitlab.*` |
| BitBucket | `bitbucket.org` |
| Gitea / Forgejo / Codeberg | any host with `/archive/` path |
| SourceForge | `downloads.sourceforge.net` |
| PyPI | `files.pythonhosted.org`, `pypi.org` |
| crates.io | `static.crates.io` |
| npm | `registry.npmjs.org` |
| RubyGems | `rubygems.org` |
| Hackage (Haskell) | `hackage.haskell.org` |
| CPAN (Perl) | `cpan.org`, `metacpan.org` |
| kernel.org | `www.kernel.org`, `cdn.kernel.org` |
| cgit / gitweb | `git.kernel.org`, any `.git/snapshot` |
| Go module proxy | `proxy.golang.org` |
| URL regex (custom) | per-package rule override |
| Repology + Anitya fallback | all unknown sources |

- **PKG_HASH verification** — downloads the source tarball and compares its SHA-256 against `PKG_HASH`; mismatches are highlighted in red
- **Retry with exponential backoff** — configurable retry count for transient HTTP errors / rate limits
- **Per-package rules** — ignore specific versions, clamp version range, strip tag prefixes/suffixes, allow pre-releases, or override the backend entirely with a URL regex
- **Skip packages** — exclude specific package names from checks
- **Multi-URL support** — tries each URL in `PKG_SOURCE_URL` until a backend is detected
- **Snapshot delta** — compares results with the previous run and only shows what changed
- **Interactive TUI** — configure all options without editing files; settings persist across runs
- **Color-coded XLSX export** — outdated rows in red, up-to-date in green, hash mismatches highlighted
- **CSV export** — machine-readable output for CI pipelines
- **Bilingual UI** — English / 中文, switchable at runtime

---

## Build

```bash
cargo build --release
# binary: target/release/makefile_checker
```

Requires Rust 1.75+ (uses `LazyLock`).

---

## Usage

```bash
./makefile_checker
```

On first run you will be prompted to configure search paths, concurrency, etc. Settings are saved to `~/.config/makefile_checker/config.toml` and reloaded automatically.

### GitHub Token (recommended)

Without a token the GitHub API is limited to **60 requests/hour**. With a token: **5 000/hour**.

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
./makefile_checker
```

The token can also be set in the interactive Configure menu or directly in the config file.

---

## Configuration File

Location: `~/.config/makefile_checker/config.toml`

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
lang          = "en"            # en | zh

# Per-package overrides
[pkg_rules.openssl]
ignore_regex       = ["^1\\..*"]   # ignore 1.x branch
min_version        = "3.0.0"

[pkg_rules.nginx]
strip_prefix       = "release-"
include_prerelease = false

[pkg_rules.some-obscure-pkg]
url_regex_url     = "https://example.com/downloads/"
url_regex_pattern = 'href="v?([0-9]+\.[0-9]+\.[0-9]+)\.tar'
```

### `pkg_rules` Fields

| Field | Type | Description |
| --- | --- | --- |
| `ignore_regex` | `[String]` | Skip versions matching any of these regex patterns |
| `min_version` | String | Only accept versions ≥ this |
| `max_version` | String | Only accept versions ≤ this |
| `strip_prefix` | String | Remove prefix from tag before comparing (e.g. `"release-"`) |
| `strip_suffix` | String | Remove suffix from tag before comparing (e.g. `"-stable"`) |
| `include_prerelease` | bool | Also consider alpha/beta/rc versions (default: `false`) |
| `url_regex_url` | String | Fetch this URL and extract the version with `url_regex_pattern` |
| `url_regex_pattern` | String | Regex with capture group 1 for version extraction |

---

## Output

### Terminal table

```text
Package          Current    Latest     Status     Tag/Commit   Backend   Hash
─────────────────────────────────────────────────────────────────────────────
tailscale        1.94.2     1.96.4     OUTDATED   v1.96.4      github    ✓
openssl          3.3.0      3.3.2      OUTDATED              github    ✗ MISMATCH
curl             8.7.1      8.7.1      OK                       github    ✓
```

### XLSX / CSV

Exported to the configured `output_path`. XLSX columns:

`PKG_NAME` · `Directory` · `Current` · `Latest` · `Status` · `Tag/Commit` · `Backend` · `Hash Status` · `Commit SHA` · `Upstream URL` · `Note` · `Path`

---

## Architecture

```text
src/
├── main.rs           # entry point
├── config.rs         # Config + PkgRule structs, TOML load/save
├── makefile_parser.rs# parse Makefile vars, detect SourceType
├── upstream.rs       # all backend check functions, apply_rule, verify_hash
├── reporter.rs       # terminal table, print_summary, XLSX/CSV export
├── interactive.rs    # TUI menus (dialoguer), run_check orchestration
├── snapshot.rs       # delta comparison with previous run
└── i18n.rs           # bilingual string constants
```

---

## Tests

```bash
cargo test
```

35 unit tests covering: all 14 backend URL patterns, multi-URL fallback, variable expansion, version comparison, pre-release detection, and all `apply_rule` behaviours.
