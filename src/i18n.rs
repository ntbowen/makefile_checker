use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    En,
    Zh,
}

impl Default for Lang {
    fn default() -> Self {
        Lang::En
    }
}

impl std::fmt::Display for Lang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Lang::En => write!(f, "English"),
            Lang::Zh => write!(f, "中文"),
        }
    }
}

/// A bilingual string: (English, Chinese)
pub struct T(pub &'static str, pub &'static str);

impl T {
    pub fn get(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.0,
            Lang::Zh => self.1,
        }
    }
}

// ─────────────────────────── string constants ────────────────────────────

pub const BANNER_SUBTITLE: T = T(
    "OpenWrt Makefile Upstream Version Checker",
    "OpenWrt Makefile 上游版本检查工具",
);

// Main menu
pub const MENU_PROMPT: T = T("What would you like to do?", "请选择操作");
pub const MENU_RUN: T = T("Run check now", "立即运行检查");
pub const MENU_CONFIGURE: T = T("Configure settings", "修改配置");
pub const MENU_QUIT: T = T("Quit", "退出");

// Config labels
pub const CFG_TITLE: T = T("─── Configure Settings ───", "─── 配置设置 ───");
pub const CFG_PATHS: T = T(
    "Search paths (comma-separated)",
    "搜索路径（逗号分隔）",
);
pub const CFG_JOBS: T = T("Parallel jobs", "并行任务数");
pub const CFG_TIMEOUT: T = T("HTTP timeout (seconds)", "HTTP 超时（秒）");
pub const CFG_TOKEN: T = T(
    "GitHub API token (leave empty to keep / use GITHUB_TOKEN env)",
    "GitHub API Token（留空保持不变 / 或设置 GITHUB_TOKEN 环境变量）",
);
pub const CFG_FORMAT: T = T("Output format", "输出格式");
pub const CFG_OUTDIR: T = T("Output directory path", "输出目录路径");
pub const CFG_SKIP: T = T(
    "Skip directory patterns (comma-separated)",
    "跳过目录模式（逗号分隔）",
);
pub const CFG_SAVED: T = T("configuration saved.", "配置已保存。");

// Status labels (shown in header)
pub const STATUS_PATHS: T = T("Search paths:", "搜索路径:");
pub const STATUS_JOBS: T = T("Jobs:", "并行数:");
pub const STATUS_TIMEOUT: T = T("Timeout:", "超时:");
pub const STATUS_OUTPUT: T = T("Output:", "输出路径:");
pub const STATUS_NONE: T = T("(none)", "（未设置）");

// Scan section
pub const SCAN_TITLE: T = T("─── Scanning for Makefiles ───", "─── 扫描 Makefile ───");
pub const SCAN_SEARCHING: T = T("Searching for Makefiles...", "正在搜索 Makefile...");
pub const SCAN_FOUND: T = T("Found", "找到");
pub const SCAN_MAKEFILE_S: T = T("Makefile(s)", "个 Makefile");
pub const SCAN_NONE: T = T(
    "No Makefiles found in the specified paths.",
    "在指定路径中未找到 Makefile。",
);
pub const SCAN_PARSED: T = T("Parsed", "已解析");
pub const SCAN_VALID: T = T(
    "valid OpenWrt package Makefile(s)",
    "个有效的 OpenWrt 包 Makefile",
);
pub const SCAN_NONE_VALID: T = T(
    "No valid OpenWrt package Makefiles found.",
    "未找到有效的 OpenWrt 包 Makefile。",
);
pub const SCAN_PARSING: T = T("parsing...", "解析中...");

// Check section
pub const CHECK_ALL_PROMPT: T = T(
    "Check all packages? (No = select subset)",
    "检查全部包？（选否可手动筛选）",
);
pub const CHECK_SELECT_PROMPT: T = T(
    "Select packages to check (space to toggle, enter to confirm)",
    "选择要检查的包（空格切换，回车确认）",
);
pub const CHECK_PROGRESS: T = T(
    "parallel jobs...",
    "并行任务...",
);
pub const CHECK_CHECKING: T = T("checking", "正在检查");
pub const CHECK_UPSTREAM_TITLE: T = T(
    "Checking package(s) upstream with",
    "正在检查包的上游版本，并行数",
);

// Table headers
pub const TBL_PACKAGE: T = T("Package", "包名");
pub const TBL_CURRENT: T = T("Current", "当前版本");
pub const TBL_LATEST: T = T("Latest", "最新版本");
pub const TBL_STATUS: T = T("Status", "状态");
pub const TBL_TAG_COMMIT: T = T("Latest Tag / Commit", "最新 Tag / Commit");
pub const TBL_BACKEND: T = T("Backend", "来源");
pub const TBL_NOTE: T = T("Note", "备注");

// Status values
pub const STATUS_OUTDATED: T = T("OUTDATED", "有更新");
pub const STATUS_OK: T = T("OK", "最新");
pub const STATUS_UNKNOWN: T = T("?", "?");
pub const STATUS_FORMAT_MISMATCH: T = T("FORMAT?", "格式不一致");

// Summary
pub const SUMMARY_ALL_OK: T = T("All packages are up-to-date!", "所有包均为最新版本！");
pub const SUMMARY_OUTDATED: T = T("Outdated packages:", "有更新的包：");
pub const SUMMARY_CHECKED: T = T("packages checked:", "个包已检查：");
pub const SUMMARY_OUTDATED_CNT: T = T("outdated", "个有更新");
pub const SUMMARY_OK_CNT: T = T("up-to-date", "个已最新");
pub const SUMMARY_UNKNOWN_CNT: T = T("unknown", "个未知");
pub const SUMMARY_FORMAT_MISMATCH_CNT: T = T("format-mismatch", "个格式不一致");

// Save
pub const SAVE_XLSX: T = T("Saved XLSX:", "已保存 XLSX：");
pub const SAVE_CSV: T = T("Saved CSV:", "已保存 CSV：");
pub const SAVE_PROMPT: T = T("Save report to file?", "是否将报告保存到文件？");
pub const SAVE_DIR_PROMPT: T = T("Output directory", "输出目录");

// Spreadsheet sheet name
pub const SHEET_NAME: T = T("Version Report", "版本报告");

// Spreadsheet column headers
pub const HDR_PKG_NAME: T = T("Package Name", "包名");
pub const HDR_DIRECTORY: T = T("Directory", "目录");
pub const HDR_CURRENT: T = T("Current Version", "当前版本");
pub const HDR_LATEST: T = T("Latest Version", "最新版本");
pub const HDR_STATUS: T = T("Status", "状态");
pub const HDR_TAG_COMMIT: T = T("Latest Tag / Commit", "最新 Tag / Commit");
pub const HDR_BACKEND: T = T("Backend", "来源");
pub const HDR_COMMIT_SHA: T = T("Latest Commit SHA", "最新 Commit SHA");
pub const HDR_UPSTREAM_URL: T = T("Upstream URL", "上游 URL");
pub const HDR_NOTE: T = T("Note / Error", "备注 / 错误");
pub const HDR_PATH: T = T("Makefile Path", "Makefile 路径");

// Snapshot / changed-only
pub const SNAP_CHANGED_ONLY_PROMPT: T = T(
    "Show changed packages only (compare with last run snapshot)?",
    "只显示有变化的包（与上次快照对比）？",
);
pub const SNAP_NO_CHANGES: T = T(
    "No changes detected since last snapshot.",
    "与上次快照相比没有变化。",
);
pub const SNAP_NEW_RUN: T = T(
    "(First run, no snapshot yet — showing all results)",
    "（首次运行，尚无快照，显示全部结果）",
);
pub const SNAP_SAVED: T = T("Snapshot updated.", "快照已更新。");
pub const SNAP_CHANGED_COUNT: T = T("changed since last run:", "相比上次有变化：");

// Hash mismatch
pub const TBL_HASH: T = T("Hash", "哈希");
pub const HDR_HASH_STATUS: T = T("Hash Status", "哈希状态");
pub const HASH_MISMATCH: T = T("⚠ MISMATCH", "⚠ 哈希不符");
pub const HASH_OK: T = T("✓", "✓");
pub const HASH_UNCHECKED: T = T("-", "-");
pub const SUMMARY_HASH_MISMATCH: T = T(
    "packages with PKG_HASH mismatch (possible tampering!):",
    "个包 PKG_HASH 不匹配（可能被篡改！）：",
);

// Configure extras
pub const CFG_RETRY: T = T("Max retries on HTTP errors", "HTTP 错误最大重试次数");
pub const CFG_SKIP_PKGS: T = T(
    "Skip package names (comma-separated, exact match)",
    "跳过的包名（逗号分隔，精确匹配）",
);
pub const CFG_PKG_RULES_TITLE: T = T(
    "─── Per-Package Rules ───",
    "─── 包级规则 ───",
);
pub const CFG_PKG_RULES_LIST: T = T(
    "Currently configured pkg_rules (edit config.toml directly for full control):",
    "已配置的包级规则（完整编辑请直接修改 config.toml）：",
);
pub const CFG_PKG_RULES_NONE: T = T(
    "(none configured)",
    "（暂无配置）",
);
pub const CFG_PKG_RULES_HINT: T = T(
    "Tip: add rules in config.toml under [pkg_rules.<pkg_name>]\n  Available fields: ignore_regex, min_version, max_version, strip_prefix, strip_suffix, include_prerelease, url_regex_url, url_regex_pattern",
    "提示：在 config.toml 的 [pkg_rules.<包名>] 下添加规则\n  可用字段：ignore_regex, min_version, max_version, strip_prefix, strip_suffix, include_prerelease, url_regex_url, url_regex_pattern",
);

// Pre-release global toggle
pub const CFG_PRERELEASE: T = T(
    "Include pre-release versions globally (alpha/beta/rc/dev)",
    "全局包含预发布版本（alpha/beta/rc/dev）",
);
pub const CFG_PRERELEASE_NOTE: T = T(
    "  (per-package override: set include_prerelease=true under [pkg_rules.<name>] in config.toml)",
    "  （单包覆盖：在 config.toml 的 [pkg_rules.<包名>] 下设置 include_prerelease=true）",
);

// Misc
pub const LANG_SELECT_PROMPT: &str = "Language / 语言";
pub const SECONDS_SUFFIX: T = T("s", "秒");

// Configure: dl_path and hash fetch
pub const CFG_DL_PATH: T = T(
    "Source download directory (e.g. /path/to/openwrt/dl, leave empty to disable save)",
    "源码下载保存路径（如 /path/to/openwrt/dl，留空不保存）",
);
pub const CFG_FETCH_HASH: T = T(
    "Fetch upstream tarball SHA-256 hash for outdated packages?",
    "对有更新的包下载上游源码并计算 SHA-256？",
);

// Post-check: format-mismatch notice
pub const FORMAT_MISMATCH_HEADER: T = T(
    "─── Version format mismatch (skipped from auto-update) ───",
    "─── 版本格式不一致（已跳过自动更新） ───",
);
pub const FORMAT_MISMATCH_NOTE: T = T(
    "current and latest use incompatible versioning schemes — manual review required",
    "当前版本与最新版本格式不兼容，需人工检查后再更新",
);

// Post-check: outdated packages action menu
pub const OUTDATED_HEADER: T = T(
    "─── Outdated packages ───",
    "─── 有更新的包 ───",
);
pub const OUTDATED_SELECT_ALL_PROMPT: T = T(
    "Select all outdated packages? (No = choose manually)",
    "选择全部有更新的包？（否 = 手动勾选）",
);
pub const OUTDATED_SELECT_MANUAL_PROMPT: T = T(
    "Select packages  [space = toggle, a = select all, enter = confirm]",
    "勾选要操作的包  [空格切换, a 全选, 回车确认]",
);
pub const OUTDATED_ACTION_PROMPT: T = T(
    "What to do with the selected packages?",
    "对已选包执行什么操作？",
);
pub const OUTDATED_ACTION_HASH: T = T(
    "① Fetch upstream commit & SHA-256 only  (no file changes)",
    "① 仅获取上游 commit 和 SHA-256  （不修改文件）",
);
pub const OUTDATED_ACTION_UPDATE: T = T(
    "② Update Makefile only  (PKG_VERSION; skip hash if not yet fetched)",
    "② 仅更新 Makefile  （只写 PKG_VERSION；未获取哈希则跳过 PKG_HASH）",
);
pub const OUTDATED_ACTION_BOTH: T = T(
    "③ Fetch hash THEN update Makefile  (PKG_VERSION + PKG_HASH)",
    "③ 先获取哈希再更新 Makefile  （同时写入 PKG_VERSION 和 PKG_HASH）",
);
pub const OUTDATED_ACTION_SKIP: T = T(
    "④ Skip — go back",
    "④ 跳过，返回",
);

// Hash fetch progress
pub const HASH_FETCH_TITLE: T = T(
    "─── Step 1 / 2 : Fetching upstream commit & SHA-256 ───",
    "─── 第 1 步 / 共 2 步：获取上游 commit 和 SHA-256 ───",
);
pub const HASH_FETCH_TITLE_ONLY: T = T(
    "─── Fetching upstream commit & SHA-256 ───",
    "─── 获取上游 commit 和 SHA-256 ───",
);
pub const HASH_FETCH_NO_URL: T = T(
    "no source URL — skipped",
    "无源码 URL，跳过",
);
pub const HASH_FETCH_NO_FILE: T = T(
    "no source filename — skipped",
    "无源码文件名，跳过",
);
pub const HASH_FETCH_OK: T = T("hash fetched:", "哈希获取成功:");
pub const HASH_FETCH_ERR: T = T("hash error:", "哈希获取失败:");
pub const COMMIT_FETCH_OK: T = T("latest commit:", "最新 commit:");
pub const COMMIT_FETCH_ERR: T = T("commit error:", "commit 获取失败:");

// Makefile update
pub const UPDATE_TITLE: T = T(
    "─── Step 2 / 2 : Updating Makefiles ───",
    "─── 第 2 步 / 共 2 步：更新 Makefile ───",
);
pub const UPDATE_TITLE_ONLY: T = T(
    "─── Updating Makefiles ───",
    "─── 更新 Makefile ───",
);
pub const UPDATE_CONFIRM: T = T(
    "Proceed to update the selected Makefile(s)?",
    "确认更新以上选中的 Makefile？",
);
pub const UPDATE_OK: T = T("updated, changed:", "已更新，修改字段:");
pub const UPDATE_ERR: T = T("update error:", "更新失败:");
pub const UPDATE_BAK: T = T("backup written to", "备份已写入");
pub const UPDATE_NOTHING: T = T("nothing to update (no new version/hash available)", "无可更新内容（缺少版本或哈希）");

// Spreadsheet new columns
pub const HDR_UPSTREAM_COMMIT: T = T("Upstream Commit", "上游 Commit");
pub const HDR_UPSTREAM_HASH: T = T("Upstream SHA-256", "上游 SHA-256");
