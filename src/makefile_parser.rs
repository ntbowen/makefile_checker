use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ParsedMakefile {
    pub path: PathBuf,
    pub pkg_name: String,
    pub pkg_version: String,
    pub pkg_release: Option<String>,
    /// Primary (first) resolved URL
    pub source_url: Option<String>,
    /// All mirror URLs from PKG_SOURCE_URL (space/newline separated)
    pub source_urls: Vec<String>,
    pub source_file: Option<String>,
    pub pkg_hash: Option<String>,
    pub pkg_source_date: Option<String>,
    pub pkg_source_version: Option<String>,
    pub source_type: SourceType,
    pub raw_vars: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum SourceType {
    GitHubRelease {
        owner: String,
        repo: String,
        tag_template: TagTemplate,
    },
    GitHubCommit {
        owner: String,
        repo: String,
        commit: String,
    },
    GitHubTagPath {
        owner: String,
        repo: String,
        tag_path: String,
    },
    GitLab {
        host: String,
        owner: String,
        repo: String,
        tag_template: TagTemplate,
    },
    BitBucket {
        owner: String,
        repo: String,
        tag_template: TagTemplate,
    },
    Gitea {
        host: String,
        owner: String,
        repo: String,
        tag_template: TagTemplate,
    },
    SourceForge {
        project: String,
    },
    PyPI {
        package: String,
    },
    CratesIo {
        package: String,
    },
    Npm {
        package: String,
    },
    RubyGems {
        gem: String,
    },
    Hackage {
        package: String,
    },
    Cpan {
        module: String,
    },
    KernelOrg {
        package: String,
    },
    Cgit {
        /// Base URL of the cgit instance, e.g. https://git.kernel.org/pub/scm/utils/dtc/dtc.git
        repo_url: String,
    },
    Maven {
        group_id: String,
        artifact_id: String,
    },
    GoModule {
        module_path: String,
    },
    /// Fallback: fetch a URL and extract version with regex
    UrlRegex {
        url: String,
        regex: String,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TagTemplate {
    WithV,        // v${VERSION}
    Plain,        // ${VERSION}
    Custom(String), // e.g. release-${VERSION}, liburing-${VERSION}
}

pub fn parse_makefile(path: &Path) -> Result<Option<ParsedMakefile>> {
    let content = std::fs::read_to_string(path)?;
    
    let mut vars: HashMap<String, String> = HashMap::new();

    // Pass 1: collect raw variable definitions (simple assignment only)
    for line in content.lines() {
        let line = line.trim();
        // Skip comments and empty lines
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        // Match: VAR:=value or VAR=value (not +=)
        if let Some(caps) = RE_VAR_ASSIGN.captures(line) {
            let key = caps[1].to_string();
            let val = caps[2].trim().to_string();
            vars.insert(key, val);
        }
    }

    // Must have PKG_NAME
    let pkg_name = match vars.get("PKG_NAME") {
        Some(v) if !v.is_empty() => v.clone(),
        _ => return Ok(None),
    };

    let pkg_version = vars.get("PKG_VERSION").cloned().unwrap_or_default();
    if pkg_version.is_empty() {
        return Ok(None);
    }

    let pkg_release = vars.get("PKG_RELEASE").cloned();
    let pkg_hash = vars.get("PKG_HASH").cloned();
    let pkg_source_date = vars.get("PKG_SOURCE_DATE").cloned();
    let pkg_source_version = vars.get("PKG_SOURCE_VERSION").cloned();

    // Resolve PKG_SOURCE_URL — may be multiple space/newline-separated mirror URLs
    let raw_source_url = vars.get("PKG_SOURCE_URL").cloned().unwrap_or_default();
    let source_urls: Vec<String> = if raw_source_url.is_empty() {
        vec![]
    } else {
        raw_source_url
            .split_whitespace()
            .map(|u| expand_vars(u, &vars))
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .collect()
    };
    let source_url = source_urls.first().cloned();

    let raw_source = vars.get("PKG_SOURCE").cloned().unwrap_or_default();
    let source_file = if raw_source.is_empty() {
        None
    } else {
        Some(expand_vars(&raw_source, &vars))
    };

    // Try each URL until we get a recognised source type
    let source_type = source_urls
        .iter()
        .map(|u| detect_source_type(u, &pkg_version, &pkg_name, &vars))
        .find(|t| !matches!(t, SourceType::Unknown))
        .unwrap_or(SourceType::Unknown);

    Ok(Some(ParsedMakefile {
        path: path.to_path_buf(),
        pkg_name,
        pkg_version,
        pkg_release,
        source_url,
        source_urls,
        source_file,
        pkg_hash,
        pkg_source_date,
        pkg_source_version,
        source_type,
        raw_vars: vars,
    }))
}

/// Expand $(VAR) and ${VAR} references in a string using the vars map.
/// Also handles the pattern $(VAR_NAME:=value) or simple $(VAR).
fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();

    // Replace $(VAR) and ${VAR} patterns
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 10 {
        changed = false;
        iterations += 1;
        let new = RE_VAR_REF.replace_all(&result, |caps: &regex::Captures| {
            let varname = &caps[1];
            if let Some(val) = vars.get(varname) {
                changed = true;
                val.clone()
            } else {
                caps[0].to_string()
            }
        });
        result = new.to_string();
    }

    // Remove trailing ?  (used in codeload URLs as query string separator)
    if result.ends_with('?') {
        result.pop();
    }

    result
}

/// Detect source type from the resolved URL.
fn detect_source_type(
    url: &str,
    pkg_version: &str,
    pkg_name: &str,
    vars: &HashMap<String, String>,
) -> SourceType {
    // codeload.github.com/<owner>/<repo>/tar.gz/<ref>
    if let Some(caps) = RE_CODELOAD.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        let ref_part = caps[3].to_string();

        // Check for commit hash (40 hex chars)
        if RE_COMMIT_HASH.is_match(&ref_part) {
            return SourceType::GitHubCommit {
                owner,
                repo,
                commit: ref_part,
            };
        }

        // Check for PKG_SOURCE_VERSION (commit-based)
        if let Some(src_ver) = vars.get("PKG_SOURCE_VERSION") {
            if RE_COMMIT_HASH.is_match(src_ver) {
                return SourceType::GitHubCommit {
                    owner,
                    repo,
                    commit: src_ver.clone(),
                };
            }
        }

        // refs/tags/<path> pattern (e.g. refs/tags/wrapper/26018)
        if ref_part.starts_with("refs/tags/") {
            let tag_path = ref_part.trim_start_matches("refs/tags/").to_string();
            // If the path ends with the version number
            if tag_path.ends_with(pkg_version) {
                let prefix = &tag_path[..tag_path.len() - pkg_version.len()];
                let prefix = prefix.trim_end_matches('/');
                if prefix.is_empty() {
                    return SourceType::GitHubRelease {
                        owner,
                        repo,
                        tag_template: TagTemplate::Plain,
                    };
                } else {
                    return SourceType::GitHubTagPath {
                        owner,
                        repo,
                        tag_path: format!("{}/", prefix),
                    };
                }
            }
            return SourceType::GitHubTagPath {
                owner,
                repo,
                tag_path,
            };
        }

        // Direct tag reference: v<version>, <version>, or <prefix>-<version>
        let tag_template = detect_tag_template(&ref_part, pkg_version);
        return SourceType::GitHubRelease {
            owner,
            repo,
            tag_template,
        };
    }

    // github.com/<owner>/<repo>/archive/<ref>.tar.gz
    if let Some(caps) = RE_GITHUB_ARCHIVE.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        let ref_part = caps[3].to_string();
        let tag_template = detect_tag_template(&ref_part, pkg_version);
        return SourceType::GitHubRelease {
            owner,
            repo,
            tag_template,
        };
    }

    // gitlab.com or self-hosted GitLab: gitlab.*/owner/repo/-/archive/
    if let Some(caps) = RE_GITLAB.captures(url) {
        let host = caps[1].to_string();
        let owner = caps[2].to_string();
        let repo = caps[3].to_string();
        let ref_part = caps[4].to_string();
        let tag_template = detect_tag_template(&ref_part, pkg_version);
        return SourceType::GitLab { host, owner, repo, tag_template };
    }

    // BitBucket: bitbucket.org/<owner>/<repo>/get/<ref>.tar.gz
    if let Some(caps) = RE_BITBUCKET.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        let ref_part = caps[3].to_string();
        let tag_template = detect_tag_template(&ref_part, pkg_version);
        return SourceType::BitBucket { owner, repo, tag_template };
    }

    // Gitea / Forgejo: <host>/<owner>/<repo>/archive/<ref>.tar.gz
    // Exclude known platforms already handled above
    if let Some(caps) = RE_GITEA.captures(url) {
        let host = caps[1].to_string();
        let is_known = host == "github.com"
            || host == "codeload.github.com"
            || host.starts_with("gitlab.")
            || host == "bitbucket.org";
        if !is_known {
            let owner = caps[2].to_string();
            let repo = caps[3].to_string();
            let ref_part = caps[4].to_string();
            let tag_template = detect_tag_template(&ref_part, pkg_version);
            return SourceType::Gitea { host, owner, repo, tag_template };
        }
    }

    // SourceForge: downloads.sourceforge.net/project/<proj>/
    if let Some(caps) = RE_SOURCEFORGE.captures(url) {
        return SourceType::SourceForge { project: caps[1].to_string() };
    }

    // PyPI: files.pythonhosted.org or pypi.org
    if let Some(caps) = RE_PYPI.captures(url) {
        let pkg = caps.get(1).or_else(|| caps.get(2))
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| pkg_name.to_string());
        return SourceType::PyPI { package: pkg };
    }

    // crates.io
    if let Some(caps) = RE_CRATESIO.captures(url) {
        return SourceType::CratesIo { package: caps[1].to_string() };
    }

    // npm / registry.npmjs.org
    if let Some(caps) = RE_NPM.captures(url) {
        return SourceType::Npm { package: caps[1].to_string() };
    }

    // RubyGems
    if let Some(caps) = RE_RUBYGEMS.captures(url) {
        return SourceType::RubyGems { gem: caps[1].to_string() };
    }

    // Hackage (Haskell)
    if let Some(caps) = RE_HACKAGE.captures(url) {
        return SourceType::Hackage { package: caps[1].to_string() };
    }

    // CPAN (Perl)
    if let Some(caps) = RE_CPAN.captures(url) {
        return SourceType::Cpan { module: caps[1].to_string() };
    }

    // kernel.org: cdn.kernel.org or www.kernel.org/pub/
    if let Some(caps) = RE_KERNELORG.captures(url) {
        return SourceType::KernelOrg { package: caps[1].to_string() };
    }

    // cgit: git.kernel.org, git.savannah.gnu.org, etc.
    if let Some(caps) = RE_CGIT.captures(url) {
        return SourceType::Cgit { repo_url: caps[1].to_string() };
    }

    // Maven Central: repo1.maven.org or search.maven.org
    if let Some(caps) = RE_MAVEN.captures(url) {
        let group_id = caps[1].replace('/', ".");
        let artifact_id = caps[2].to_string();
        return SourceType::Maven { group_id, artifact_id };
    }

    // Go module proxy: proxy.golang.org or sum.golang.org
    if let Some(caps) = RE_GOMODULE.captures(url) {
        return SourceType::GoModule { module_path: caps[1].to_string() };
    }

    SourceType::Unknown
}

fn detect_tag_template(ref_part: &str, pkg_version: &str) -> TagTemplate {
    if ref_part == format!("v{}", pkg_version) {
        TagTemplate::WithV
    } else if ref_part == pkg_version {
        TagTemplate::Plain
    } else {
        // Custom: e.g. "release-1.2.3" or "liburing-2.14" or "app/v1.2.3"
        TagTemplate::Custom(ref_part.replace(pkg_version, "${VERSION}"))
    }
}

// ──────────────────────────── compiled regexes ────────────────────────────

use std::sync::LazyLock;

static RE_VAR_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)(?::=|=)\s*(.*)$").unwrap()
});

static RE_VAR_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$[({]([A-Za-z_][A-Za-z0-9_]*)[)}]").unwrap()
});

static RE_CODELOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://codeload\.github\.com/([^/]+)/([^/]+)/tar\.gz/(.+)").unwrap()
});

static RE_GITHUB_ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://github\.com/([^/]+)/([^/]+)/archive/(.+?)\.tar\.gz").unwrap()
});

static RE_COMMIT_HASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{40}$").unwrap()
});

static RE_GITLAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://(gitlab\.[^/]+)/([^/]+)/([^/]+)/-/archive/([^/]+)/").unwrap()
});

static RE_BITBUCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://bitbucket\.org/([^/]+)/([^/]+)/get/([^/]+)\.tar").unwrap()
});

static RE_GITEA: LazyLock<Regex> = LazyLock::new(|| {
    // Generic /owner/repo/archive/ref.tar.gz — filter known platforms in code
    // ref can contain dots (e.g. v1.2.3), so allow any non-slash chars
    Regex::new(r"https://([^/]+)/([^/]+)/([^/]+)/archive/([^/]+)\.tar").unwrap()
});

static RE_SOURCEFORGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:downloads\.sourceforge\.net|sourceforge\.net/projects?)/([^/]+)").unwrap()
});

static RE_PYPI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:files\.pythonhosted\.org/packages/[^/]+/[^/]+/([^/]+)/|pypi\.org/packages/[^/]+/[^/]+/([^/]+)/)").unwrap()
});

static RE_CRATESIO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:static\.crates\.io|crates\.io)/crates/([^/]+)/").unwrap()
});

static RE_NPM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://registry\.npmjs\.org/([^/]+)/-/").unwrap()
});

static RE_RUBYGEMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:rubygems\.org/gems/|rubygems\.org/downloads/)([^/\-]+)").unwrap()
});

static RE_HACKAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://hackage\.haskell\.org/package/([^/]+)/").unwrap()
});

static RE_CPAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:www\.cpan\.org|cpan\.metacpan\.org|search\.cpan\.org)/[^/]+/[^/]*/([A-Za-z][A-Za-z0-9:_-]+)-").unwrap()
});

static RE_KERNELORG: LazyLock<Regex> = LazyLock::new(|| {
    // cdn.kernel.org/pub/linux/utils/<pkg>/ or www.kernel.org/pub/linux/<pkg>/
    Regex::new(r"https?://(?:cdn|www)\.kernel\.org/pub/linux/(?:[^/]+/)?([^/]+)/").unwrap()
});

static RE_CGIT: LazyLock<Regex> = LazyLock::new(|| {
    // cgit/gitweb repos: git.*/pub/scm/**/*.git or cgit.*/... ending in .git archive
    // Capture the repo base URL (up to and including .git)
    Regex::new(r"(https?://git\.[^/]+/(?:pub/scm/|cgit/|r/)?[^?]+\.git)(?:/(?:snapshot|archive)/|\?)").unwrap()
});

static RE_MAVEN: LazyLock<Regex> = LazyLock::new(|| {
    // https://repo1.maven.org/maven2/<group/path>/<artifact>/<version>/<artifact>-<version>.*
    // No backreference: just match the structure up to the artifact directory
    Regex::new(r"https?://(?:repo1\.maven\.org/maven2|search\.maven\.org/remotecontent[^?]*\?)/([a-z][^/]+(?:/[^/]+)*)/([^/]+)/[^/]+/").unwrap()
});

static RE_GOMODULE: LazyLock<Regex> = LazyLock::new(|| {
    // https://proxy.golang.org/github.com/foo/bar/@v/v1.2.3.zip
    Regex::new(r"https?://proxy\.golang\.org/([^/@]+(?:/[^/@]+)+)/@").unwrap()
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn detect(url: &str) -> SourceType {
        detect_source_type(url, "1.2.3", "mypkg", &HashMap::new())
    }

    // ── detect_source_type ────────────────────────────────────────────────

    #[test]
    fn test_github_release_v_prefix() {
        let st = detect("https://codeload.github.com/nicowillis/foo/tar.gz/v1.2.3");
        assert!(matches!(st, SourceType::GitHubRelease { .. }));
    }

    #[test]
    fn test_github_release_plain() {
        let st = detect("https://codeload.github.com/nicowillis/foo/tar.gz/1.2.3");
        assert!(matches!(st, SourceType::GitHubRelease { .. }));
    }

    #[test]
    fn test_github_commit() {
        let st = detect(
            "https://codeload.github.com/owner/repo/tar.gz/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        );
        assert!(matches!(st, SourceType::GitHubCommit { .. }));
    }

    #[test]
    fn test_gitlab() {
        let st = detect("https://gitlab.com/user/project/-/archive/v1.2.3/project-v1.2.3.tar.gz");
        assert!(matches!(st, SourceType::GitLab { .. }));
    }

    #[test]
    fn test_bitbucket() {
        let st = detect("https://bitbucket.org/user/repo/get/v1.2.3.tar.gz");
        assert!(matches!(st, SourceType::BitBucket { .. }));
    }

    #[test]
    fn test_gitea() {
        // Gitea archive URL: host must not be github/gitlab/bitbucket
        let st = detect("https://codeberg.org/owner/repo/archive/v1.2.3.tar.gz");
        assert!(matches!(st, SourceType::Gitea { .. }));
    }

    #[test]
    fn test_sourceforge() {
        let st = detect("https://downloads.sourceforge.net/project/myproject/myproject-1.2.3.tar.gz");
        assert!(matches!(st, SourceType::SourceForge { .. }));
    }

    #[test]
    fn test_pypi() {
        // Real pythonhosted URL structure: /packages/<py_ver>/<initial>/<pkg_name>/<file>
        let st = detect("https://files.pythonhosted.org/packages/source/r/requests/requests-1.2.3.tar.gz");
        assert!(matches!(st, SourceType::PyPI { .. }));
    }

    #[test]
    fn test_cratesio() {
        let st = detect("https://static.crates.io/crates/serde/serde-1.2.3.crate");
        assert!(matches!(st, SourceType::CratesIo { .. }));
    }

    #[test]
    fn test_npm() {
        let st = detect("https://registry.npmjs.org/lodash/-/lodash-1.2.3.tgz");
        assert!(matches!(st, SourceType::Npm { .. }));
    }

    #[test]
    fn test_rubygems() {
        let st = detect("https://rubygems.org/gems/rails-1.2.3.gem");
        assert!(matches!(st, SourceType::RubyGems { .. }));
    }

    #[test]
    fn test_hackage() {
        let st = detect("https://hackage.haskell.org/package/aeson-1.2.3/aeson-1.2.3.tar.gz");
        assert!(matches!(st, SourceType::Hackage { .. }));
    }

    #[test]
    fn test_kernelorg() {
        let st = detect("https://www.kernel.org/pub/linux/utils/util-linux/v2.38/util-linux-2.38.tar.gz");
        assert!(matches!(st, SourceType::KernelOrg { .. }));
    }

    #[test]
    fn test_cgit() {
        let st = detect("https://git.kernel.org/pub/scm/utils/dtc/dtc.git/snapshot/dtc-1.6.1.tar.gz");
        assert!(matches!(st, SourceType::Cgit { .. }));
    }

    #[test]
    fn test_gomodule() {
        let st = detect("https://proxy.golang.org/github.com/foo/bar/@v/v1.2.3.zip");
        assert!(matches!(st, SourceType::GoModule { .. }));
    }

    #[test]
    fn test_unknown() {
        let st = detect("https://example.com/some/random/url/file.tar.gz");
        assert!(matches!(st, SourceType::Unknown));
    }

    // ── multi-URL parsing ─────────────────────────────────────────────────

    #[test]
    fn test_multi_url_first_wins() {
        // First URL is GitHub, second is SourceForge
        let url = "https://codeload.github.com/owner/repo/tar.gz/v1.2.3 https://downloads.sourceforge.net/project/foo/foo-1.2.3.tar.gz";
        let vars: HashMap<String, String> = [("PKG_SOURCE_URL".to_string(), url.to_string())].into();
        // Simulate what parse_makefile does
        let source_urls: Vec<String> = url
            .split_whitespace()
            .map(|u| u.to_string())
            .collect();
        let source_type = source_urls
            .iter()
            .map(|u| detect_source_type(u, "1.2.3", "repo", &vars))
            .find(|t| !matches!(t, SourceType::Unknown))
            .unwrap_or(SourceType::Unknown);
        assert!(matches!(source_type, SourceType::GitHubRelease { .. }));
    }

    #[test]
    fn test_multi_url_fallback_to_second() {
        // First URL is unrecognised, second is PyPI with correct path structure
        let url = "https://example.com/random.tar.gz https://files.pythonhosted.org/packages/source/r/requests/requests-1.2.3.tar.gz";
        let vars: HashMap<String, String> = HashMap::new();
        let source_urls: Vec<String> = url.split_whitespace().map(|u| u.to_string()).collect();
        let source_type = source_urls
            .iter()
            .map(|u| detect_source_type(u, "1.2.3", "requests", &vars))
            .find(|t| !matches!(t, SourceType::Unknown))
            .unwrap_or(SourceType::Unknown);
        assert!(matches!(source_type, SourceType::PyPI { .. }));
    }

    // ── expand_vars ───────────────────────────────────────────────────────

    #[test]
    fn test_expand_vars_simple() {
        let mut vars = HashMap::new();
        vars.insert("PKG_VERSION".to_string(), "1.2.3".to_string());
        let result = expand_vars("https://example.com/pkg-$(PKG_VERSION).tar.gz", &vars);
        assert_eq!(result, "https://example.com/pkg-1.2.3.tar.gz");
    }

    #[test]
    fn test_expand_vars_curly() {
        let mut vars = HashMap::new();
        vars.insert("PKG_NAME".to_string(), "myapp".to_string());
        let result = expand_vars("https://example.com/${PKG_NAME}/", &vars);
        assert_eq!(result, "https://example.com/myapp/");
    }

    #[test]
    fn test_expand_vars_missing() {
        let vars = HashMap::new();
        let result = expand_vars("https://example.com/$(MISSING)/file.tar.gz", &vars);
        assert_eq!(result, "https://example.com/$(MISSING)/file.tar.gz");
    }
}

pub fn find_makefiles(search_paths: &[String], skip_patterns: &[String]) -> Vec<PathBuf> {
    use walkdir::WalkDir;
    let mut results = Vec::new();

    for search_path in search_paths {
        let walker = WalkDir::new(search_path)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                // Skip hidden dirs and skip_patterns
                let name = e.file_name().to_string_lossy();
                if name.starts_with('.') {
                    return false;
                }
                if e.file_type().is_dir() {
                    let path_str = e.path().to_string_lossy();
                    for pat in skip_patterns {
                        if path_str.contains(pat.as_str()) {
                            return false;
                        }
                    }
                }
                true
            });

        for entry in walker.flatten() {
            if entry.file_type().is_file()
                && entry.file_name().to_string_lossy() == "Makefile"
            {
                results.push(entry.path().to_path_buf());
            }
        }
    }

    results
}
