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
    Pecl {
        package: String,
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
    /// golang official release tarballs from go.dev/dl/ (e.g. golang1.26, golang-bootstrap)
    GoLang,
    /// Android / Google source git repos (googlesource.com)
    GoogleSource {
        repo_url: String,
    },
    /// freedesktop.org tarball download (gstreamer, libsoup, etc.)
    Freedesktop {
        project: String,
    },
    /// No PKG_SOURCE_URL: package is maintained directly in the feed (scripts/configs only)
    NoSource,
    /// PKG_SOURCE_URL uses @OPENWRT or @IMMORTALWRT private mirror (source not publicly accessible)
    OpenWrtMirror,
    /// PKG_SOURCE_URL uses @GNU / @GNOME / @APACHE / @SAVANNAH / @KERNEL mirror macro
    GnuMirror {
        /// Mirror name, e.g. "GNU", "GNOME", "APACHE", "SAVANNAH", "KERNEL"
        mirror: String,
        /// Package/project name extracted from the macro argument
        package: String,
    },
    /// PKG_SOURCE_URL is a plain HTTP download directory not matching any known forge/registry
    CustomUrl {
        url: String,
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

    // Pass 1: collect variable definitions (:= = ?= +=)
    // Also handle line continuations (trailing backslash).
    //
    // Conditional blocks (ifeq / ifdef / ifneq / ifndef … else … endif) are
    // NOT evaluated — we cannot know which branch will be taken at build time.
    // To get a stable, predictable result we only record assignments that are
    // at the top level (depth == 0).  Assignments inside any conditional block
    // are silently ignored; the top-level value is considered authoritative.
    let mut logical_line = String::new();
    let mut cond_depth: usize = 0;  // nesting depth of ifeq/ifdef/ifneq/ifndef blocks
    for raw in content.lines() {
        if raw.ends_with('\\') {
            logical_line.push_str(raw.trim_end_matches('\\'));
            logical_line.push(' ');
            continue;
        } else {
            logical_line.push_str(raw);
        }
        let line = logical_line.trim().to_string();
        logical_line.clear();

        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        // Track conditional block depth
        let lw = line.split_whitespace().next().unwrap_or("");
        if matches!(lw, "ifeq" | "ifneq" | "ifdef" | "ifndef") {
            cond_depth += 1;
            continue;
        }
        if lw == "endif" {
            cond_depth = cond_depth.saturating_sub(1);
            continue;
        }
        if lw == "else" {
            // else at the same depth — do not change depth
            continue;
        }

        // Only process variable assignments at top level
        if cond_depth > 0 {
            continue;
        }

        if let Some(caps) = RE_VAR_ASSIGN.captures(&line) {
            let key = caps[1].to_string();
            let op  = caps[2].to_string();
            let val = caps[3].trim().to_string();
            match op.as_str() {
                "?=" => { vars.entry(key).or_insert(val); }
                "+="  => {
                    let entry = vars.entry(key).or_default();
                    if !entry.is_empty() { entry.push(' '); }
                    entry.push_str(&val);
                }
                _ => { vars.insert(key, val); }
            }
        }
    }

    // Inject defaults from well-known OpenWrt include files.
    // The parser does not expand `include` directives, so variables defined
    // only in included .mk files are missing.  We detect the include by
    // scanning the raw content and insert the known defaults as ?= (only if
    // the key is not already set by the Makefile itself).
    if content.contains("$(INCLUDE_DIR)/u-boot.mk") || content.contains("include/u-boot.mk") {
        // From include/u-boot.mk:
        //   PKG_SOURCE = $(PKG_NAME)-$(PKG_VERSION).tar.bz2
        //   PKG_SOURCE_URL = https://ftp.denx.de/pub/u-boot
        vars.entry("PKG_SOURCE_URL".to_string())
            .or_insert_with(|| "https://ftp.denx.de/pub/u-boot".to_string());
        vars.entry("PKG_SOURCE".to_string())
            .or_insert_with(|| "$(PKG_NAME)-$(PKG_VERSION).tar.bz2".to_string());
    }

    // Must have PKG_NAME (expand in case it references other vars)
    let pkg_name_raw = vars.get("PKG_NAME").cloned().unwrap_or_default();
    let pkg_name = expand_vars(&pkg_name_raw, &vars);
    if pkg_name.is_empty() || pkg_name.contains("$(") {
        return Ok(None);
    }

    // Expand PKG_VERSION — handles $(subst ...) / $(word ...) / multi-var etc.
    let pkg_version_raw = vars.get("PKG_VERSION").cloned().unwrap_or_default();
    let pkg_version = expand_vars(&pkg_version_raw, &vars);
    // Skip if still unexpanded (contains unresolved Make expressions)
    if pkg_version.is_empty() || pkg_version.contains("$(") {
        return Ok(None);
    }

    let pkg_release = vars.get("PKG_RELEASE")
        .map(|v| expand_vars(v, &vars));
    let pkg_hash = vars.get("PKG_HASH")
        .map(|v| expand_vars(v, &vars));
    let pkg_source_date = vars.get("PKG_SOURCE_DATE")
        .map(|v| expand_vars(v, &vars));
    let pkg_source_version = vars.get("PKG_SOURCE_VERSION")
        .map(|v| expand_vars(v, &vars));

    // Resolve PKG_SOURCE_URL: first expand the whole value (handles += concat),
    // then split on whitespace to get individual mirror URLs.
    let raw_source_url = vars.get("PKG_SOURCE_URL").cloned().unwrap_or_default();
    let expanded_source_url = expand_vars(&raw_source_url, &vars);
    // Detect @MIRROR macros before http filtering — these are OpenWrt shorthand
    // that won't pass the http:// filter below.
    let mirror_macro_type: Option<SourceType> = expanded_source_url
        .split_whitespace()
        .find_map(|token| {
            if let Some(rest) = token.strip_prefix("@SF/") {
                // @SF/projectname[/path] → SourceForge
                // Some use @SF/project/<name>/ (with literal 'project' as first segment)
                let segments: Vec<&str> = rest.splitn(3, '/').collect();
                let project = if segments.first().copied() == Some("project") {
                    segments.get(1).copied().unwrap_or(rest)
                } else {
                    segments.first().copied().unwrap_or(rest)
                };
                let project = expand_vars(project, &vars);
                if !project.is_empty() && !project.contains("$(") {
                    return Some(SourceType::SourceForge { project });
                }
            } else if token.starts_with("@OPENWRT") || token.starts_with("@IMMORTALWRT") {
                return Some(SourceType::OpenWrtMirror);
            } else if let Some((mirror, rest)) = [
                ("GNU", token.strip_prefix("@GNU/")),
                ("GNOME", token.strip_prefix("@GNOME/")),
                ("APACHE", token.strip_prefix("@APACHE/")),
                ("SAVANNAH", token.strip_prefix("@SAVANNAH/")),
                ("KERNEL", token.strip_prefix("@KERNEL/")),
            ].iter().find_map(|(m, r)| r.map(|r| (*m, r))) {
                // Extract the first path segment as package name
                let pkg = rest.split('/').next().unwrap_or(rest);
                let pkg = expand_vars(pkg, &vars);
                let pkg = if pkg.is_empty() || pkg.contains("$(") {
                    pkg_name.clone()
                } else {
                    pkg
                };
                return Some(SourceType::GnuMirror {
                    mirror: mirror.to_string(),
                    package: pkg,
                });
            }
            None
        });

    let source_urls: Vec<String> = if expanded_source_url.is_empty() {
        vec![]
    } else {
        expanded_source_url
            .split_whitespace()
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .map(|u| u.trim_end_matches('?').to_string())
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
    let source_type_from_url = source_urls
        .iter()
        .map(|u| detect_source_type(u, &pkg_version, &pkg_name, &vars))
        .find(|t| !matches!(t, SourceType::Unknown));

    // Fallback: infer source type from ecosystem-specific variables when URL
    // detection fails (common in perl/python/php/node packages).
    let source_type = source_type_from_url
        .or(mirror_macro_type)
        .unwrap_or_else(|| {
        // 0. GitHub bare-git URL: github.com/<owner>/<repo>.git
        //    Combined with PKG_SOURCE_VERSION to decide commit vs tag.
        for url in &source_urls {
            if let Some(caps) = RE_GITHUB_GIT.captures(url) {
                let owner = caps[1].to_string();
                let repo  = caps[2].to_string();
                if let Some(ver) = &pkg_source_version {
                    if RE_COMMIT_HASH.is_match(ver) {
                        return SourceType::GitHubCommit {
                            owner,
                            repo,
                            commit: ver.clone(),
                        };
                    }
                    // Path-based tag: e.g. client_release/8.0/8.0.4 (BOINC style)
                    if let Some(tag_path) = detect_path_tag_prefix(ver, &pkg_version) {
                        return SourceType::GitHubTagPath { owner, repo, tag_path };
                    }
                    {
                        // Tag-like version string (e.g. mc_release_10.39.0)
                        let tag_template = detect_tag_template(ver, &pkg_version);
                        return SourceType::GitHubRelease { owner, repo, tag_template };
                    }
                }
                // No PKG_SOURCE_VERSION — treat as GitHubRelease with plain tag
                return SourceType::GitHubRelease {
                    owner,
                    repo,
                    tag_template: TagTemplate::WithV,
                };
            }
        }

        // 0b. CPAN directory URL: extract module name from PKG_SOURCE_NAME, then
        //     PKG_SOURCE (strip -VERSION.ext), finally derive from pkg_name.
        //     PKG_SOURCE_URL = https://www.cpan.org/authors/id/P/PE/PETDANCE/
        //     PKG_SOURCE_URL = https://search.cpan.org/CPAN/authors/id/C/CB/CBARRATT/
        for url in &source_urls {
            if RE_CPAN_DIR.is_match(url) {
                let module = vars.get("PKG_SOURCE_NAME")
                    .map(|v| expand_vars(v, &vars))
                    .filter(|v| !v.is_empty() && !v.contains("$("))
                    // Try PKG_SOURCE: strip trailing -<version>.<ext>
                    .or_else(|| {
                        vars.get("PKG_SOURCE").map(|v| {
                            let s = expand_vars(v, &vars);
                            // Strip -(v?)<version>.<ext> suffix
                            let s = s.trim_end_matches(".tar.gz")
                                      .trim_end_matches(".tar.bz2")
                                      .trim_end_matches(".tgz");
                            // Remove trailing hyphen + version (may start with 'v')
                            let pkg_ver = &pkg_version;
                            let stripped = if let Some(pos) = s.rfind(&format!("-{}", pkg_ver)) {
                                &s[..pos]
                            } else if let Some(pos) = s.rfind(&format!("-v{}", pkg_ver)) {
                                &s[..pos]
                            } else {
                                s
                            };
                            stripped.to_string()
                        })
                        .filter(|v| !v.is_empty() && !v.contains("$("))
                    })
                    .unwrap_or_else(|| {
                        // Last resort: capitalise pkg_name after stripping perl- prefix
                        pkg_name.trim_start_matches("perl-")
                            .split('-')
                            .map(|w| {
                                let mut c = w.chars();
                                match c.next() {
                                    None => String::new(),
                                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                                }
                            })
                            .collect::<Vec<_>>().join("-")
                    });
                if !module.is_empty() {
                    return SourceType::Cpan { module };
                }
            }
        }

        // 0c. GitLab bare .git URL: gitlab.<host>/<owner>[/subgroup]/<repo>.git
        //     cap[1]=host, cap[2]=owner path (may include subgroups), cap[3]=repo
        //     Combined with PKG_SOURCE_VERSION to decide commit vs tag.
        for url in &source_urls {
            if let Some(caps) = RE_GITLAB_GIT.captures(url) {
                let host = caps[1].to_string();
                let owner = caps[2].to_string();
                let repo  = caps[3].to_string();
                if let Some(ver) = &pkg_source_version {
                    if RE_COMMIT_HASH.is_match(ver) {
                        return SourceType::GitLab {
                            host, owner, repo,
                            tag_template: TagTemplate::Plain,
                        };
                    } else {
                        let tag_template = detect_tag_template(ver, &pkg_version);
                        return SourceType::GitLab { host, owner, repo, tag_template };
                    }
                }
                return SourceType::GitLab {
                    host, owner, repo,
                    tag_template: TagTemplate::WithV,
                };
            }
        }

        // 0d. GitHub bare repo URL: github.com/<owner>/<repo> (no .git, no sub-path)
        //     Combined with PKG_SOURCE_VERSION to decide commit vs tag (same as .git case)
        for url in &source_urls {
            if let Some(caps) = RE_GITHUB_BARE.captures(url) {
                let owner = caps[1].to_string();
                let repo  = caps[2].to_string();
                if let Some(ver) = &pkg_source_version {
                    if RE_COMMIT_HASH.is_match(ver) {
                        return SourceType::GitHubCommit { owner, repo, commit: ver.clone() };
                    }
                    // Path-based tag: e.g. client_release/8.0/8.0.4 (BOINC style)
                    if let Some(tag_path) = detect_path_tag_prefix(ver, &pkg_version) {
                        return SourceType::GitHubTagPath { owner, repo, tag_path };
                    }
                    let tag_template = detect_tag_template(ver, &pkg_version);
                    return SourceType::GitHubRelease { owner, repo, tag_template };
                }
                return SourceType::GitHubRelease {
                    owner,
                    repo,
                    tag_template: TagTemplate::WithV,
                };
            }
        }

        // 1. PyPI: PYPI_NAME variable (python-* packages often omit PKG_SOURCE_URL)
        if let Some(name) = vars.get("PYPI_NAME") {
            let name = expand_vars(name, &vars);
            if !name.is_empty() && !name.contains("$(") {
                return SourceType::PyPI { package: name };
            }
        }
        // 2. CPAN (MetaCPAN): METACPAN_NAME + optional METACPAN_AUTHOR
        if let Some(name) = vars.get("METACPAN_NAME") {
            let name = expand_vars(name, &vars);
            if !name.is_empty() && !name.contains("$(") {
                return SourceType::Cpan { module: name };
            }
        }
        // 3. PECL (PHP extensions): PECL_NAME variable
        if let Some(name) = vars.get("PECL_NAME") {
            let name = expand_vars(name, &vars);
            if !name.is_empty() && !name.contains("$(") {
                return SourceType::Pecl { package: name };
            }
        }
        // 4. Scoped npm: PKG_NPM_SCOPE + PKG_NPM_NAME (e.g. @azure/event-hubs)
        if let (Some(scope), Some(npm_name)) = (vars.get("PKG_NPM_SCOPE"), vars.get("PKG_NPM_NAME")) {
            let scope = expand_vars(scope, &vars);
            let npm_name = expand_vars(npm_name, &vars);
            if !scope.is_empty() && !npm_name.is_empty()
                && !scope.contains("$(") && !npm_name.contains("$(")
            {
                return SourceType::Npm { package: format!("@{}/{}", scope, npm_name) };
            }
        }
        // 5. Unscoped npm fallback: PKG_NPM_NAME alone
        if let Some(npm_name) = vars.get("PKG_NPM_NAME") {
            let npm_name = expand_vars(npm_name, &vars);
            if !npm_name.is_empty() && !npm_name.contains("$(") {
                return SourceType::Npm { package: npm_name };
            }
        }
        // 6. Plain HTTP URL that matched no known forge/registry → CustomUrl
        if let Some(url) = source_urls.first() {
            return SourceType::CustomUrl { url: url.clone() };
        }
        // 7. No PKG_SOURCE_URL at all → package is a local feed / script bundle
        if raw_source_url.is_empty() {
            return SourceType::NoSource;
        }
        SourceType::Unknown
    });

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

impl ParsedMakefile {
    /// Returns the version that should be used for upstream comparison.
    ///
    /// Rule: use PKG_VERSION unless pkg_source_version is a *plain* version
    /// that differs from pkg_version (e.g. lua-bit32 uses PKG_SRC_VERSION=0.3
    /// while PKG_VERSION is an OpenWrt meta-version 5.3.0).
    ///
    /// We deliberately do NOT return the raw pkg_source_version when it is a
    /// decorated git tag like "hidapi-0.15.0", "rel-20200726", "v2.2.2",
    /// "mc_release_10.39.0", etc. — those are tag strings, not version numbers.
    /// For those cases the caller should use pkg_version directly and let the
    /// tag-template machinery handle prefix/suffix stripping.
    pub fn effective_version(&self) -> &str {
        if let Some(sv) = &self.pkg_source_version {
            let sv = sv.trim();
            if sv.is_empty() { return &self.pkg_version; }

            // Skip git commit hashes (hex strings 12-40 chars)
            let is_commit_hash = sv.len() >= 12
                && sv.len() <= 40
                && sv.chars().all(|c| c.is_ascii_hexdigit());
            if is_commit_hash { return &self.pkg_version; }

            // Skip decorated tags: if after stripping a leading v/V the value
            // equals pkg_version, it is just "v<version>" — already the same.
            let sv_stripped = sv.trim_start_matches(|c| c == 'v' || c == 'V');
            if sv_stripped == self.pkg_version.as_str() {
                return &self.pkg_version;
            }

            // Skip any tag that contains a non-version prefix separator, i.e.
            // the pkg_version appears INSIDE the string with a prefix or suffix
            // (e.g. "hidapi-0.15.0", "rel-20200726", "mc_release_10.39.0",
            //  "version_0.2.0", "lf-6.12.20-2.0.0", "release-0.1").
            // Heuristic: if pkg_version is a substring and is not the whole
            // stripped value, it is a decorated tag — skip it.
            if sv_stripped != self.pkg_version.as_str()
                && sv.contains(self.pkg_version.as_str())
            {
                return &self.pkg_version;
            }

            // A "plain" version different from pkg_version — use it.
            // Typical case: lua-bit32 PKG_SRC_VERSION=0.3, PKG_VERSION=5.3.0
            // Only accept if the value starts with a digit (after optional v).
            if sv_stripped.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                // Also skip date-prefixed formats from PKG_SOURCE_VERSION that
                // do not match pkg_version at all (these are exotic custom tags)
                if !sv.contains('_') || sv_stripped == self.pkg_version.as_str() {
                    return sv;
                }
            }
        }
        &self.pkg_version
    }
}

/// Fully expand a Make expression: variable refs + built-in function calls.
/// Supports up to 10 recursive passes; unknown $(shell ...) expressions are
/// left unexpanded (returned as-is) rather than causing a panic.
pub(crate) fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    expand_vars_depth(input, vars, 0)
}

fn expand_vars_depth(input: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    if depth > 32 {
        return input.to_string();
    }
    let mut result = input.to_string();
    for _ in 0..10 {
        let expanded = expand_once_depth(&result, vars, depth);
        if expanded == result {
            break;
        }
        result = expanded;
    }
    // Remove trailing ? (codeload URL separator artefact)
    if result.ends_with('?') {
        result.pop();
    }
    result
}

/// One pass: find the innermost $(...) / ${...}, evaluate it, replace in string.
fn expand_once_depth(input: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let open = bytes[i + 1];
            if open == b'(' || open == b'{' {
                let close = if open == b'(' { b')' } else { b'}' };
                // Find matching close bracket (bracket_depth-aware)
                let start = i + 2;
                let mut bracket_depth = 1usize;
                let mut j = start;
                while j < bytes.len() && bracket_depth > 0 {
                    if bytes[j] == open  { bracket_depth += 1; }
                    if bytes[j] == close { bracket_depth -= 1; }
                    if bracket_depth > 0 { j += 1; }
                }
                if bracket_depth == 0 {
                    let inner = &input[start..j];
                    let replacement = eval_make_expr_depth(inner, vars, depth + 1);
                    out.push_str(&replacement);
                    i = j + 1;
                    continue;
                }
            } else if open == b'$' {
                // $$ → literal $
                out.push('$');
                i += 2;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Evaluate the content of $(…): either a function call or a variable name.
fn eval_make_expr_depth(inner: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let inner = inner.trim();

    // Detect function call: starts with a known function name followed by space/tab
    // Split only on the FIRST whitespace after the function name
    if let Some(sp) = inner.find(|c: char| c == ' ' || c == '\t') {
        let func = inner[..sp].trim();
        let rest = inner[sp..].trim();
        match func {
            "subst" => return make_subst(rest, vars, depth),
            "patsubst" => return make_patsubst(rest, vars, depth),
            "strip" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            "findstring" => return make_findstring(rest, vars, depth),
            "filter" => return make_filter(rest, vars, false, depth),
            "filter-out" => return make_filter(rest, vars, true, depth),
            "sort" => {
                let t = expand_vars_depth(rest, vars, depth);
                let mut words: Vec<&str> = t.split_whitespace().collect();
                words.sort_unstable();
                words.dedup();
                return words.join(" ");
            }
            "word" => return make_word(rest, vars, depth),
            "words" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace().count().to_string();
            }
            "wordlist" => return make_wordlist(rest, vars, depth),
            "firstword" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace().next().unwrap_or("").to_string();
            }
            "lastword" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace().last().unwrap_or("").to_string();
            }
            "dir" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace()
                    .map(|w| match w.rfind('/') {
                        Some(p) => w[..=p].to_string(),
                        None => "./".to_string(),
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "notdir" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace()
                    .map(|w| match w.rfind('/') {
                        Some(p) => &w[p+1..],
                        None => w,
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "suffix" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace()
                    .filter_map(|w| w.rfind('.').map(|p| &w[p..]))
                    .collect::<Vec<_>>().join(" ");
            }
            "basename" => {
                let t = expand_vars_depth(rest, vars, depth);
                return t.split_whitespace()
                    .map(|w| match w.rfind('.') {
                        Some(p) => &w[..p],
                        None => w,
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "addsuffix" => return make_addsuffix(rest, vars, depth),
            "addprefix" => return make_addprefix(rest, vars, depth),
            "join" => return make_join(rest, vars, depth),
            "if" => return make_if(rest, vars, depth),
            "or" => return make_or(rest, vars, depth),
            "and" => return make_and(rest, vars, depth),
            "shell" => {
                // Do not execute — return empty string as safe fallback
                return String::new();
            }
            "call" => {
                // Basic $(call var,arg1,arg2) — expand the variable only
                let parts: Vec<&str> = rest.splitn(2, ',').collect();
                let fname = expand_vars_depth(parts[0].trim(), vars, depth);
                if let Some(v) = vars.get(fname.trim()) {
                    return expand_vars_depth(v, vars, depth);
                }
                return String::new();
            }
            "value" => {
                let vname = expand_vars_depth(rest, vars, depth);
                return vars.get(vname.trim()).cloned().unwrap_or_default();
            }
            "origin" | "flavor" | "info" | "warning" | "error" => {
                return String::new();
            }
            _ => {
                // Unknown function — return unexpanded to avoid data loss
                return format!("$({} {})", func, rest);
            }
        }
    }

    // No function call — plain variable name (possibly with :=... modifier)
    // Handle $(VAR:suffix=replacement) substitution reference
    if let Some(colon) = inner.find(':') {
        let varname = &inner[..colon];
        let subst_spec = &inner[colon+1..];
        if let Some(eq) = subst_spec.find('=') {
            let pat = &subst_spec[..eq];
            let rep = &subst_spec[eq+1..];
            if let Some(val) = vars.get(varname.trim()) {
                let expanded = expand_vars_depth(val, vars, depth);
                return expanded.split_whitespace()
                    .map(|w| if w.ends_with(pat) {
                        format!("{}{}", &w[..w.len()-pat.len()], rep)
                    } else {
                        w.to_string()
                    })
                    .collect::<Vec<_>>().join(" ");
            }
        }
    }

    // Plain variable lookup
    vars.get(inner)
        .map(|v| expand_vars_depth(v, vars, depth))
        .unwrap_or_else(|| format!("$({inner})"))
}

// ───────────────────────── Make function helpers ──────────────────────────

/// Split function args on the first comma (args may contain nested $(...))
fn split_args(s: &str, n: usize) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in s.chars() {
        match ch {
            '(' | '{' => { depth += 1; current.push(ch); }
            ')' | '}' => { depth -= 1; current.push(ch); }
            ',' if depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
                if args.len() + 1 == n { }
            }
            _ => { current.push(ch); }
        }
        if args.len() + 1 == n && ch == ',' {
            // last chunk: everything remaining
            break;
        }
    }
    args.push(current.trim().to_string());
    args
}

/// Split on first comma only, rest goes into second element.
/// Does NOT trim values — spaces in args (e.g. $(subst -, ,...)) must be preserved.
fn split2(s: &str) -> (String, String) {
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '{' => depth += 1,
            ')' | '}' => depth -= 1,
            ',' if depth == 0 => {
                return (s[..i].to_string(), s[i+1..].to_string());
            }
            _ => {}
        }
    }
    (s.to_string(), String::new())
}

/// Split on first two commas, rest goes into third element
fn split3(s: &str) -> (String, String, String) {
    let (a, rest) = split2(s);
    let (b, c)   = split2(&rest);
    (a, b, c)
}

fn make_subst(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (from, to, text) = split3(args);
    let from = expand_vars_depth(from.trim(), vars, depth);
    let to   = expand_vars_depth(&to,   vars, depth);  // intentionally keep internal spaces
    let text = expand_vars_depth(text.trim(), vars, depth);
    text.replace(from.as_str(), to.as_str())
}

fn make_patsubst(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (pat, rep, text) = split3(args);
    let pat  = expand_vars_depth(pat.trim(),  vars, depth);
    let rep  = expand_vars_depth(rep.trim(),  vars, depth);
    let text = expand_vars_depth(text.trim(), vars, depth);
    text.split_whitespace()
        .map(|w| patsubst_word(w, &pat, &rep))
        .collect::<Vec<_>>().join(" ")
}

fn patsubst_word(word: &str, pat: &str, rep: &str) -> String {
    if let Some(stem_pos) = pat.find('%') {
        let prefix = &pat[..stem_pos];
        let suffix = &pat[stem_pos+1..];
        if word.starts_with(prefix) && word.ends_with(suffix)
            && word.len() >= prefix.len() + suffix.len()
        {
            let stem = &word[prefix.len()..word.len()-suffix.len()];
            return rep.replace('%', stem);
        }
        word.to_string()
    } else {
        if word == pat { rep.to_string() } else { word.to_string() }
    }
}

fn make_findstring(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (find, text) = split2(args);
    let find = expand_vars_depth(find.trim(), vars, depth);
    let text = expand_vars_depth(text.trim(), vars, depth);
    if text.contains(find.as_str()) { find } else { String::new() }
}

fn make_filter(args: &str, vars: &HashMap<String, String>, invert: bool, depth: usize) -> String {
    let (pats_raw, text) = split2(args);
    let pats_expanded = expand_vars_depth(pats_raw.trim(), vars, depth);
    let pats: Vec<&str> = pats_expanded.split_whitespace().collect();
    let text = expand_vars_depth(text.trim(), vars, depth);
    text.split_whitespace()
        .filter(|w| {
            let matched = pats.iter().any(|p| word_matches_pattern(w, p));
            if invert { !matched } else { matched }
        })
        .collect::<Vec<_>>().join(" ")
}

/// Returns true if word matches the Make filter pattern (supports % wildcard)
fn word_matches_pattern(word: &str, pat: &str) -> bool {
    if let Some(stem_pos) = pat.find('%') {
        let prefix = &pat[..stem_pos];
        let suffix = &pat[stem_pos+1..];
        word.starts_with(prefix)
            && word.ends_with(suffix)
            && word.len() >= prefix.len() + suffix.len()
    } else {
        word == pat
    }
}

fn make_word(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (n_raw, text) = split2(args);
    let n_str = expand_vars_depth(n_raw.trim(), vars, depth);
    let text  = expand_vars_depth(text.trim(),  vars, depth);
    let n: usize = n_str.trim().parse().unwrap_or(0);
    text.split_whitespace().nth(n.saturating_sub(1)).unwrap_or("").to_string()
}

fn make_wordlist(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let parts = split_args(args, 3);
    let s = expand_vars_depth(parts.first().map(String::as_str).unwrap_or(""), vars, depth);
    let e = expand_vars_depth(parts.get(1).map(String::as_str).unwrap_or(""), vars, depth);
    let text = expand_vars_depth(parts.get(2).map(String::as_str).unwrap_or(""), vars, depth);
    let start: usize = s.trim().parse().unwrap_or(1);
    let end:   usize = e.trim().parse().unwrap_or(0);
    text.split_whitespace()
        .enumerate()
        .filter(|(i, _)| *i + 1 >= start && *i + 1 <= end)
        .map(|(_, w)| w)
        .collect::<Vec<_>>().join(" ")
}

fn make_addsuffix(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (suf, text) = split2(args);
    let suf  = expand_vars_depth(suf.trim(),  vars, depth);
    let text = expand_vars_depth(text.trim(), vars, depth);
    text.split_whitespace()
        .map(|w| format!("{}{}", w, suf))
        .collect::<Vec<_>>().join(" ")
}

fn make_addprefix(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (pre, text) = split2(args);
    let pre  = expand_vars_depth(pre.trim(),  vars, depth);
    let text = expand_vars_depth(text.trim(), vars, depth);
    text.split_whitespace()
        .map(|w| format!("{}{}", pre, w))
        .collect::<Vec<_>>().join(" ")
}

fn make_join(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (list1, list2) = split2(args);
    let l1 = expand_vars_depth(list1.trim(), vars, depth);
    let l2 = expand_vars_depth(list2.trim(), vars, depth);
    let w1: Vec<&str> = l1.split_whitespace().collect();
    let w2: Vec<&str> = l2.split_whitespace().collect();
    let len = w1.len().max(w2.len());
    (0..len)
        .map(|i| format!("{}{}", w1.get(i).unwrap_or(&""), w2.get(i).unwrap_or(&"")))
        .collect::<Vec<_>>().join(" ")
}

fn make_if(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    let (cond, rest) = split2(args);
    let (then_part, else_part) = split2(&rest);
    let cond_val = expand_vars_depth(&cond, vars, depth);
    if !cond_val.trim().is_empty() {
        expand_vars_depth(&then_part, vars, depth)
    } else {
        expand_vars_depth(&else_part, vars, depth)
    }
}

fn make_or(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    // $(or cond1,cond2,...) — return first non-empty
    let mut rest = args.to_string();
    loop {
        let (a, b) = split2(&rest);
        let v = expand_vars_depth(&a, vars, depth);
        if !v.trim().is_empty() { return v; }
        if b.is_empty() { return String::new(); }
        rest = b;
    }
}

fn make_and(args: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    // $(and cond1,cond2,...) — return last if all non-empty, else ""
    let mut rest = args.to_string();
    let mut last;
    loop {
        let (a, b) = split2(&rest);
        let v = expand_vars_depth(&a, vars, depth);
        if v.trim().is_empty() { return String::new(); }
        last = v;
        if b.is_empty() { return last; }
        rest = b;
    }
}

/// Detect source type from the resolved URL.
fn detect_source_type(
    url: &str,
    pkg_version: &str,
    pkg_name: &str,
    _vars: &HashMap<String, String>,
) -> SourceType {
    // codeload.github.com/<owner>/<repo>/tar.gz/<ref>
    if let Some(caps) = RE_CODELOAD.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        let ref_part = caps[3].to_string();

        // Check for commit hash (40 hex chars) directly in the URL ref
        if RE_COMMIT_HASH.is_match(&ref_part) {
            // URL ref is a commit hash — use it directly
            return SourceType::GitHubCommit {
                owner,
                repo,
                commit: ref_part,
            };
        }

        // URL ref is NOT a commit hash (it's a tag like "v0.19.0").
        // Do NOT let PKG_SOURCE_VERSION override this — the package has a proper
        // release tag even if PKG_SOURCE_VERSION happens to hold a commit hash
        // (e.g. tini: URL uses v$(PKG_VERSION) tag but also sets PKG_SOURCE_VERSION).
        // Fall through to tag / release detection below.

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

        // Path-based tag: e.g. client_release/8.0/8.0.4 (BOINC style)
        if let Some(tag_path) = detect_path_tag_prefix(&ref_part, pkg_version) {
            return SourceType::GitHubTagPath { owner, repo, tag_path };
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

    // BitBucket downloads directory: bitbucket.org/<owner>/<repo>/downloads/
    if let Some(caps) = RE_BITBUCKET_DOWNLOADS.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        return SourceType::BitBucket { owner, repo, tag_template: TagTemplate::WithV };
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

    // github.com/<owner>/<repo>/releases/download/<tag>/
    // Used by packages that set PKG_SOURCE_URL to the download dir and
    // PKG_SOURCE to the filename (e.g. mac80211, pcapplusplus)
    if let Some(caps) = RE_GITHUB_RELEASES_DOWNLOAD.captures(url) {
        let owner = caps[1].to_string();
        let repo = caps[2].to_string();
        let tag_ref = caps[3].to_string();
        let tag_template = detect_tag_template(&tag_ref, pkg_version);
        return SourceType::GitHubRelease { owner, repo, tag_template };
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

    // golang official release tarballs: go.dev/dl/ golang.google.cn/dl/
    if RE_GOLANG_DL.is_match(url) {
        return SourceType::GoLang;
    }

    // freedesktop.org tarballs: gstreamer.freedesktop.org/src/<name>
    if let Some(caps) = RE_FREEDESKTOP.captures(url) {
        let name = caps[1].to_string();
        if !name.is_empty() {
            return SourceType::Freedesktop { project: name };
        }
    }

    // OpenWrt / ImmortalWrt source mirrors (https://sources.openwrt.org/ etc.)
    if RE_OPENWRT_SOURCES.is_match(url) {
        return SourceType::OpenWrtMirror;
    }

    // Google source repositories: *.googlesource.com/path
    if let Some(caps) = RE_GOOGLESOURCE.captures(url) {
        let repo_url = caps[1].to_string();
        return SourceType::GoogleSource { repo_url };
    }

    // ftp.denx.de/pub/u-boot: directory index with links like u-boot-2026.04.tar.bz2
    if url.contains("denx.de") && url.contains("u-boot") {
        return SourceType::UrlRegex {
            url: "https://ftp.denx.de/pub/u-boot/".to_string(),
            regex: r"u-boot-(\d{4}\.\d{2}(?:\.\d+)?)\.tar".to_string(),
        };
    }

    SourceType::Unknown
}

/// Detect whether `ref_part` is a hierarchical path tag like `client_release/8.0/8.0.4`
/// where the last segment equals `pkg_version` and the segment before it is version-dependent.
/// Returns `Some("client_release/")` (stable prefix with trailing slash) when detected,
/// or `None` when the ref is a plain tag.
fn detect_path_tag_prefix(ref_part: &str, pkg_version: &str) -> Option<String> {
    if !ref_part.contains('/') {
        return None;
    }
    let last_seg = ref_part.rsplit('/').next()?;
    if last_seg != pkg_version {
        return None;
    }
    let without_ver = ref_part[..ref_part.len() - last_seg.len()].trim_end_matches('/');
    // If the remaining path ends with a numeric-looking segment (e.g. "8.0"), strip it too
    // so we get a stable prefix like "client_release" instead of "client_release/8.0".
    let stable_prefix = if let Some(p) = without_ver.rfind('/') {
        let mid = &without_ver[p + 1..];
        if mid.chars().all(|c| c.is_ascii_digit() || c == '.') {
            &without_ver[..p]
        } else {
            without_ver
        }
    } else {
        without_ver
    };
    Some(format!("{}/", stable_prefix))
}

fn detect_tag_template(ref_part: &str, pkg_version: &str) -> TagTemplate {
    if ref_part == format!("v{}", pkg_version) {
        return TagTemplate::WithV;
    }
    if ref_part == pkg_version {
        return TagTemplate::Plain;
    }
    // Some packages use $(subst .,_,$(PKG_VERSION)) in their tag, e.g.:
    //   PKG_VERSION=2.7.4  -> tag ref R_2_7_4  (prefix "R_", dots->underscores)
    //   PKG_VERSION=8.9.0  -> tag ref CRYPTOPP_8_9_0
    //   PKG_VERSION=8.19.0 -> tag ref curl-8_19_0
    // Detect this by checking if the underscore-form of pkg_version appears in ref_part.
    let ver_underscored = pkg_version.replace('.', "_");
    if ref_part.contains(&ver_underscored) {
        // Build template by replacing the underscore form with a placeholder that
        // extract_version_from_tag will later convert back (via _ -> . normalisation).
        let tmpl = ref_part.replace(&ver_underscored, "${VERSION}");
        return TagTemplate::Custom(tmpl);
    }
    // Some packages use $(subst .,,$(PKG_VERSION)) — dots removed entirely, e.g.:
    //   PKG_VERSION=2025.04.30  -> PKG_VERSION_REAL = 1.20250430  (prefix "1.", dots stripped)
    // Detect: the dot-stripped form of pkg_version appears inside ref_part.
    let ver_dotless = pkg_version.replace('.', "");
    if ref_part.contains(&ver_dotless) && !ver_dotless.is_empty() {
        let tmpl = ref_part.replace(&ver_dotless, "${VERSION_NODOT}");
        return TagTemplate::Custom(tmpl);
    }

    // Packages like tfa-layerscape / uboot-layerscape use a text-prefixed tag whose
    // numeric components equal those in PKG_VERSION but with different separators.
    // e.g. PKG_VERSION=6.12.20.2.0.0  PKG_SOURCE_VERSION=lf-6.12.20-2.0.0
    // Detect: numeric segments of ref_part == numeric segments of pkg_version,
    // and ref_part starts with a non-digit prefix.
    // Build template Custom("<prefix>${VERSION}") so that other tags in the same
    // family (e.g. lf-6.18.2-1.0.0) are correctly matched and version-extracted.
    {
        let nums_ref: Vec<&str> = ref_part
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .collect();
        let nums_ver: Vec<&str> = pkg_version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .collect();
        if !nums_ref.is_empty()
            && nums_ref == nums_ver
            && ref_part.starts_with(|c: char| !c.is_ascii_digit())
        {
            if let Some(first_digit_pos) = ref_part.find(|c: char| c.is_ascii_digit()) {
                let prefix = &ref_part[..first_digit_pos];
                let tmpl = format!("{}${{VERSION}}", prefix);
                return TagTemplate::Custom(tmpl);
            }
        }
    }

    // Some packages like lua-openssl use PKG_VERSION=$(subst -,.,$(PKG_SOURCE_VERSION))
    // so the tag is the pkg_version with dots replaced by hyphens (or vice-versa).
    // e.g. PKG_VERSION=0.10.0.0, tag=0.10.0-0  (last dot -> hyphen)
    // These are plain tags; we can detect them by checking if the numeric segments match.
    {
        let tag_dots = ref_part.replace('-', ".");
        if tag_dots == pkg_version {
            return TagTemplate::Plain;
        }
        let ver_hyphens = pkg_version.replace('.', "-");
        if ref_part == ver_hyphens {
            return TagTemplate::Plain;
        }
    }

    // Generic custom template: e.g. "release-1.2.3" or "liburing-2.14" or "app/v1.2.3"
    // Only emit a Custom template when pkg_version actually appears inside ref_part so
    // the placeholder is meaningful; otherwise fall back to Plain.
    let replaced = ref_part.replace(pkg_version, "${VERSION}");
    if replaced.contains("${VERSION}") {
        TagTemplate::Custom(replaced)
    } else {
        TagTemplate::Plain
    }
}

// ──────────────────────────── compiled regexes ────────────────────────────

use std::sync::LazyLock;

static RE_VAR_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    // VAR := value  |  VAR = value  |  VAR ?= value  |  VAR += value
    Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*([:?+]?=)\s*(.*)$").unwrap()
});


static RE_CODELOAD: LazyLock<Regex> = LazyLock::new(|| {
    // Matches tar.gz and zip downloads: /tar.gz/<ref> or /zip/<ref>
    Regex::new(r"https://codeload\.github\.com/([^/]+)/([^/]+)/(?:tar\.gz|zip)/([^?]+)").unwrap()
});

static RE_GITHUB_ARCHIVE: LazyLock<Regex> = LazyLock::new(|| {
    // Matches /archive/<ref>.tar.gz AND /archive/<ref> (no extension, e.g. checksec)
    Regex::new(r"https://github\.com/([^/]+)/([^/]+)/archive/([^/?]+?)(?:\.tar\.gz|\.zip|$)").unwrap()
});

// github.com/<owner>/<repo>/releases/download/<tag>/ (trailing slash, file appended separately)
static RE_GITHUB_RELEASES_DOWNLOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://github\.com/([^/]+)/([^/]+)/releases/download/([^/]+)/?$").unwrap()
});

// github.com/<owner>/<repo>.git  (bare git clone URL)
static RE_GITHUB_GIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://github\.com/([^/]+)/([^/]+)\.git$").unwrap()
});

static RE_COMMIT_HASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9a-f]{40}$").unwrap()
});

// gitlab.<host>/owner[/subgroups]/<repo>/-/archive/<ref>/
// cap[1]=host, cap[2]=full owner path (may include subgroups), cap[3]=repo, cap[4]=ref
static RE_GITLAB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://(gitlab\.[^/]+)/(.+)/([^/]+)/-/archive/([^/]+)/").unwrap()
});

// gitlab.<host>/path/to/<repo>.git  (bare git clone URL, any gitlab instance)
// The host is captured; owner and repo are the last two path components.
static RE_GITLAB_GIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://(gitlab\.[^/]+|gitlab\.com)/(.+)/([^/]+)\.git$").unwrap()
});

static RE_BITBUCKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://bitbucket\.org/([^/]+)/([^/]+)/get/([^/]+)\.tar").unwrap()
});

// BitBucket downloads directory (no specific file/ref in URL)
static RE_BITBUCKET_DOWNLOADS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://bitbucket\.org/([^/]+)/([^/]+)/downloads/?$").unwrap()
});

static RE_GITEA: LazyLock<Regex> = LazyLock::new(|| {
    // Generic /owner/repo/archive/ref[.tar.gz] — filter known platforms in code
    // ref can contain dots (e.g. v1.2.3), so allow any non-slash chars
    // Optional .tar suffix to also match bare /archive/<ref> URLs (e.g. codeberg)
    Regex::new(r"https://([^/]+)/([^/]+)/([^/]+)/archive/([^/?]+?)(?:\.tar.*)?$").unwrap()
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
    // Match official registry and common CN mirrors that share the same URL structure:
    //   registry.npmjs.org/<pkg>/-/
    //   mirrors.tencent.com/npm/<pkg>/-/
    //   registry.npmmirror.com/<pkg>/-/   (formerly taobao)
    //   mirrors.huaweicloud.com/repository/npm/<pkg>/-/
    //   registry.npmjs.cf/<pkg>/-/
    Regex::new(
        r"https?://(?:registry\.npmjs\.org|mirrors\.tencent\.com/npm|registry\.npmmirror\.com|mirrors\.huaweicloud\.com/repository/npm|registry\.npmjs\.cf)/([^/]+)/-/"
    ).unwrap()
});

static RE_RUBYGEMS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:rubygems\.org/gems/|rubygems\.org/downloads/)([^/\-]+)").unwrap()
});

static RE_HACKAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://hackage\.haskell\.org/package/([^/]+)/").unwrap()
});

static RE_CPAN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches full path including package name:  cpan.org/.../Authen-SASL-2.16.tar.gz
    // Also matches author directory URLs:       cpan.org/authors/id/G/GB/GBARR/
    Regex::new(r"https?://(?:www\.cpan\.org|cpan\.metacpan\.org|search\.cpan\.org)/(?:[^/]+/)*([A-Za-z][A-Za-z0-9:_-]+)-").unwrap()
});

// Matches the cpan.org/authors/ directory URL (no package name embedded)
// search.cpan.org uses /CPAN/authors/id/... path prefix
static RE_CPAN_DIR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:www\.cpan\.org|cpan\.metacpan\.org|search\.cpan\.org)/(?:CPAN/)?authors/").unwrap()
});

// github.com/<owner>/<repo>  — bare repo URL without .git suffix or any sub-path
static RE_GITHUB_BARE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https://github\.com/([^/]+)/([^/]+)$").unwrap()
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

// go.dev/dl/ or golang.google.cn/dl/ or mirrors.*/golang/
static RE_GOLANG_DL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:go\.dev|golang\.google\.cn)/dl/").unwrap()
});

// OpenWrt / ImmortalWrt source tarballs hosting (not @MIRROR macro)
static RE_OPENWRT_SOURCES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:sources\.openwrt\.org|sources\.immortalwrt\.org)/").unwrap()
});

// Google source git: <host>.googlesource.com/path
static RE_GOOGLESOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(https?://[^/]+\.googlesource\.com/[^/?]+(?:/[^/?]+)*)").unwrap()
});

// freedesktop.org tarball downloads: gstreamer.freedesktop.org/src/<name>
// Captures the last path segment as the project name.
static RE_FREEDESKTOP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://(?:[^/]+\.)?freedesktop\.org/(?:[^/?]+/)*([^/?]+)/?$").unwrap()
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
    fn test_tini_tag_not_overridden_by_pkg_source_version() {
        // tini: URL ref is "v0.19.0" (a tag), but PKG_SOURCE_VERSION is a commit hash.
        // Must be detected as GitHubRelease, not GitHubCommit.
        let mut vars = HashMap::new();
        vars.insert(
            "PKG_SOURCE_VERSION".to_string(),
            "de40ad007797e0dcd8b7126f27bb87401d224240".to_string(),
        );
        let st = detect_source_type(
            "https://codeload.github.com/krallin/tini/tar.gz/v0.19.0",
            "0.19.0",
            "tini",
            &vars,
        );
        assert!(
            matches!(st, SourceType::GitHubRelease { .. }),
            "expected GitHubRelease but got {:?}", st
        );
        if let SourceType::GitHubRelease { owner, repo, tag_template } = st {
            assert_eq!(owner, "krallin");
            assert_eq!(repo, "tini");
            assert!(matches!(tag_template, TagTemplate::WithV));
        }
    }

    #[test]
    fn test_csshnpd_c_prefix_tag_template() {
        // csshnpd: PKG_SOURCE_URL=.../releases/download/c$(PKG_VERSION)
        // After expansion tag_ref = "c1.0.17", PKG_VERSION = "1.0.17"
        // Should be Custom("c${VERSION}") so that only c-prefixed releases are matched.
        let tmpl = detect_tag_template("c1.0.17", "1.0.17");
        assert!(
            matches!(&tmpl, TagTemplate::Custom(p) if p == "c${VERSION}"),
            "expected Custom(\"c${{VERSION}}\") but got {:?}", tmpl
        );
    }

    #[test]
    fn test_boinc_path_tag_detected_as_tag_path() {
        // boinc: PKG_SOURCE_URL=https://github.com/BOINC/boinc (bare, no .git)
        //        PKG_SOURCE_VERSION=client_release/8.0/8.0.4
        //        PKG_VERSION=8.0.4
        // Should be GitHubTagPath with tag_path="client_release/"
        // so that client_release/8.2/8.2.9 is found as latest.
        let content = "\
PKG_NAME:=boinc\n\
PKG_VERSION:=8.0.4\n\
PKG_RELEASE:=1\n\
PKG_SOURCE_PROTO:=git\n\
PKG_SOURCE_URL:=https://github.com/BOINC/boinc\n\
PKG_SOURCE_VERSION:=client_release/8.0/8.0.4\n\
PKG_MIRROR_HASH:=abc123\n";
        let tmp = std::env::temp_dir().join("boinc_test_Makefile");
        std::fs::write(&tmp, content).unwrap();
        let parsed = parse_makefile(&tmp).unwrap().unwrap();
        assert!(
            matches!(&parsed.source_type, SourceType::GitHubTagPath { owner, repo, tag_path }
                if owner == "BOINC" && repo == "boinc" && tag_path == "client_release/"),
            "expected GitHubTagPath with tag_path=client_release/, got {:?}", parsed.source_type
        );
    }

    #[test]
    fn test_lua_openssl_hyphen_version_tag() {
        // lua-openssl: PKG_SOURCE_VERSION=0.10.0-0, PKG_VERSION=$(subst -,.,$(PKG_SOURCE_VERSION))=0.10.0.0
        // Tag is "0.10.0-0" which is PKG_VERSION with last dot replaced by hyphen.
        // detect_tag_template must return Plain so find_best_tag matches "0.11.0-3" etc.
        let tmpl = detect_tag_template("0.10.0-0", "0.10.0.0");
        assert!(
            matches!(tmpl, TagTemplate::Plain),
            "expected Plain but got {:?}", tmpl
        );
    }

    #[test]
    fn test_detect_tag_template_no_placeholder_falls_back_to_plain() {
        // When pkg_version does not appear inside ref_part, the last-resort Custom()
        // would produce a template without ${VERSION} which is useless.  Should be Plain.
        let tmpl = detect_tag_template("some-unrelated-tag", "1.2.3");
        assert!(
            matches!(tmpl, TagTemplate::Plain),
            "expected Plain fallback but got {:?}", tmpl
        );
    }

    #[test]
    fn test_commit_hash_url_still_detected_as_commit() {
        // fft-eval / tac_plus pattern: URL ref IS the 40-char commit hash
        let mut vars = HashMap::new();
        vars.insert(
            "PKG_SOURCE_VERSION".to_string(),
            "4d3b6faee428e3bd9f44ab6a3d70585ec50484a1".to_string(),
        );
        let st = detect_source_type(
            "https://codeload.github.com/simonwunderlich/FFT_eval/tar.gz/4d3b6faee428e3bd9f44ab6a3d70585ec50484a1",
            "2019-11-27",
            "fft-eval",
            &vars,
        );
        assert!(
            matches!(st, SourceType::GitHubCommit { .. }),
            "expected GitHubCommit but got {:?}", st
        );
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
    fn test_npm_tencent_mirror() {
        // mirrors.tencent.com/npm/<pkg>/-/ is a common CN mirror used by OpenWrt node packages
        let st = detect("https://mirrors.tencent.com/npm/argon2/-/argon2-0.44.0.tgz");
        assert!(matches!(st, SourceType::Npm { package } if package == "argon2"),
            "Tencent npm mirror should be detected as Npm source type");
    }

    #[test]
    fn test_npm_npmmirror() {
        let st = detect("https://registry.npmmirror.com/alexa-app/-/alexa-app-4.2.3.tgz");
        assert!(matches!(st, SourceType::Npm { package } if package == "alexa-app"));
    }

    #[test]
    fn test_npm_multi_url_tencent_first() {
        // Simulates the actual node-argon2 pattern: tencent mirror first, npmjs second.
        // The parser iterates source_urls and should find Npm from the first URL.
        let url = "https://mirrors.tencent.com/npm/argon2/-/ https://registry.npmjs.org/argon2/-/";
        let source_urls: Vec<String> = url.split_whitespace()
            .filter(|u| u.starts_with("http"))
            .map(|u| u.to_string())
            .collect();
        let source_type = source_urls
            .iter()
            .map(|u| detect_source_type(u, "0.44.0", "node-argon2", &std::collections::HashMap::new()))
            .find(|t| !matches!(t, SourceType::Unknown))
            .unwrap_or(SourceType::Unknown);
        assert!(matches!(source_type, SourceType::Npm { package } if package == "argon2"),
            "Should detect Npm from tencent mirror (first URL) without needing npmjs.org");
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

    // ── Make function: subst ──────────────────────────────────────────────

    #[test]
    fn test_make_subst_basic() {
        let vars = HashMap::new();
        // $(subst ee,EE,feet on the street) -> fEEt on the strEEt
        let r = expand_vars("$(subst ee,EE,feet on the street)", &vars);
        assert_eq!(r, "fEEt on the strEEt");
    }

    #[test]
    fn test_make_subst_remove() {
        // $(subst linux,,3.2p4linux) -> 3.2p4   (hfsprogs pattern)
        let vars = HashMap::new();
        let r = expand_vars("$(subst linux,,3.2p4linux)", &vars);
        assert_eq!(r, "3.2p4");
    }

    #[test]
    fn test_make_subst_char_replace() {
        // $(subst p,.,3p2p4) -> 3.2.4  (owfs pattern)
        let vars = HashMap::new();
        let r = expand_vars("$(subst p,.,3p2p4)", &vars);
        assert_eq!(r, "3.2.4");
    }

    #[test]
    fn test_make_subst_with_var() {
        // $(subst p,.,$(PKG_REAL_VERSION))  ->  3.2.4
        let mut vars = HashMap::new();
        vars.insert("PKG_REAL_VERSION".to_string(), "3p2p4".to_string());
        let r = expand_vars("$(subst p,.,$(PKG_REAL_VERSION))", &vars);
        assert_eq!(r, "3.2.4");
    }

    // ── Make function: word ───────────────────────────────────────────────

    #[test]
    fn test_make_word() {
        let vars = HashMap::new();
        let r = expand_vars("$(word 2,foo bar baz)", &vars);
        assert_eq!(r, "bar");
    }

    #[test]
    fn test_make_word_with_subst() {
        // softethervpn pattern: $(word 1,$(subst -, ,v4.44-9807))
        let vars = HashMap::new();
        let r = expand_vars("$(word 1,$(subst -, ,v4.44-9807))", &vars);
        assert_eq!(r, "v4.44");
    }

    #[test]
    fn test_make_firstword() {
        let vars = HashMap::new();
        let r = expand_vars("$(firstword alpha beta gamma)", &vars);
        assert_eq!(r, "alpha");
    }

    #[test]
    fn test_make_lastword() {
        let vars = HashMap::new();
        let r = expand_vars("$(lastword alpha beta gamma)", &vars);
        assert_eq!(r, "gamma");
    }

    #[test]
    fn test_make_words_count() {
        let vars = HashMap::new();
        let r = expand_vars("$(words one two three)", &vars);
        assert_eq!(r, "3");
    }

    // ── Make function: patsubst ───────────────────────────────────────────

    #[test]
    fn test_make_patsubst_percent() {
        let vars = HashMap::new();
        // $(patsubst %.c,%.o,foo.c bar.c) -> foo.o bar.o
        let r = expand_vars("$(patsubst %.c,%.o,foo.c bar.c)", &vars);
        assert_eq!(r, "foo.o bar.o");
    }

    #[test]
    fn test_make_patsubst_no_match() {
        let vars = HashMap::new();
        let r = expand_vars("$(patsubst %.c,%.o,foo.h)", &vars);
        assert_eq!(r, "foo.h");
    }

    // ── Make function: strip ──────────────────────────────────────────────

    #[test]
    fn test_make_strip() {
        let vars = HashMap::new();
        let r = expand_vars("$(strip  a  b   c  )", &vars);
        assert_eq!(r, "a b c");
    }

    // ── Make function: filter / filter-out ───────────────────────────────

    #[test]
    fn test_make_filter() {
        let vars = HashMap::new();
        let r = expand_vars("$(filter %.c,foo.c bar.h baz.c)", &vars);
        assert_eq!(r, "foo.c baz.c");
    }

    #[test]
    fn test_make_filter_out() {
        let vars = HashMap::new();
        let r = expand_vars("$(filter-out %.c,foo.c bar.h baz.c)", &vars);
        assert_eq!(r, "bar.h");
    }

    // ── Make function: addsuffix / addprefix ─────────────────────────────

    #[test]
    fn test_make_addsuffix() {
        let vars = HashMap::new();
        let r = expand_vars("$(addsuffix .c,foo bar)", &vars);
        assert_eq!(r, "foo.c bar.c");
    }

    #[test]
    fn test_make_addprefix() {
        let vars = HashMap::new();
        let r = expand_vars("$(addprefix src/,foo.c bar.c)", &vars);
        assert_eq!(r, "src/foo.c src/bar.c");
    }

    // ── Substitution reference $(VAR:pat=rep) ─────────────────────────────

    #[test]
    fn test_subst_ref() {
        let mut vars = HashMap::new();
        vars.insert("SRCS".to_string(), "foo.c bar.c".to_string());
        let r = expand_vars("$(SRCS:.c=.o)", &vars);
        assert_eq!(r, "foo.o bar.o");
    }

    // ── ?= and += assignment ──────────────────────────────────────────────

    #[test]
    fn test_conditional_assign() {
        let mut vars: HashMap<String, String> = HashMap::new();
        // ?= should NOT overwrite existing value
        vars.insert("FOO".to_string(), "original".to_string());
        vars.entry("FOO".to_string()).or_insert("new".to_string());
        assert_eq!(vars["FOO"], "original");
    }

    #[test]
    fn test_append_assign() {
        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("LIST".to_string(), "a".to_string());
        let entry = vars.entry("LIST".to_string()).or_default();
        if !entry.is_empty() { entry.push(' '); }
        entry.push_str("b");
        assert_eq!(vars["LIST"], "a b");
    }

    // ── Real OpenWrt patterns ─────────────────────────────────────────────

    #[test]
    fn test_owfs_version() {
        // owfs: PKG_REAL_VERSION = 3p2p4
        //       PKG_VERSION = $(subst p,.,$(PKG_REAL_VERSION))  -> 3.2.4
        let mut vars = HashMap::new();
        vars.insert("PKG_REAL_VERSION".to_string(), "3p2p4".to_string());
        vars.insert("PKG_VERSION".to_string(), "$(subst p,.,$(PKG_REAL_VERSION))".to_string());
        let ver = expand_vars(vars.get("PKG_VERSION").unwrap(), &vars);
        assert_eq!(ver, "3.2.4");
    }

    #[test]
    fn test_hfsprogs_version() {
        // hfsprogs: PKG_REAL_VERSION = 627.40.1-linux
        //           PKG_VERSION = $(subst linux,,$(PKG_REAL_VERSION)) -> 627.40.1-
        let mut vars = HashMap::new();
        vars.insert("PKG_REAL_VERSION".to_string(), "627.40.1-linux".to_string());
        vars.insert("PKG_VERSION".to_string(), "$(subst linux,,$(PKG_REAL_VERSION))".to_string());
        let ver = expand_vars(vars.get("PKG_VERSION").unwrap(), &vars);
        assert_eq!(ver, "627.40.1-");
    }

    #[test]
    fn test_softethervpn_version() {
        // PKG_REALVERSION = v4.44-9807
        // PKG_VERSION = $(word 1,$(subst -, ,$(PKG_REALVERSION))) -> v4.44
        let mut vars = HashMap::new();
        vars.insert("PKG_REALVERSION".to_string(), "v4.44-9807".to_string());
        vars.insert("PKG_VERSION".to_string(),
            "$(word 1,$(subst -, ,$(PKG_REALVERSION)))".to_string());
        let ver = expand_vars(vars.get("PKG_VERSION").unwrap(), &vars);
        assert_eq!(ver, "v4.44");
    }

    #[test]
    fn test_multivar_version() {
        // qt6tools pattern: PKG_BASE = 6.11  PKG_BUGFIX = 0
        // PKG_VERSION = $(PKG_BASE).$(PKG_BUGFIX)  -> 6.11.0
        let mut vars = HashMap::new();
        vars.insert("PKG_BASE".to_string(), "6.11".to_string());
        vars.insert("PKG_BUGFIX".to_string(), "0".to_string());
        vars.insert("PKG_VERSION".to_string(), "$(PKG_BASE).$(PKG_BUGFIX)".to_string());
        let ver = expand_vars(vars.get("PKG_VERSION").unwrap(), &vars);
        assert_eq!(ver, "6.11.0");
    }

    #[test]
    fn test_modclean_version() {
        // node-modclean: PKG_REALVERSION = 3.0.0-beta.1
        // PKG_VERSION = $(subst -beta.,_beta,$(PKG_REALVERSION)) -> 3.0.0_beta1
        let mut vars = HashMap::new();
        vars.insert("PKG_NPM_NAME".to_string(), "modclean".to_string());
        vars.insert("PKG_NAME".to_string(), "node-$(PKG_NPM_NAME)".to_string());
        vars.insert("PKG_REALVERSION".to_string(), "3.0.0-beta.1".to_string());
        vars.insert("PKG_VERSION".to_string(),
            "$(subst -beta.,_beta,$(PKG_REALVERSION))".to_string());
        vars.insert("PKG_SOURCE".to_string(),
            "$(PKG_NPM_NAME)-$(PKG_REALVERSION).tgz".to_string());

        let name = expand_vars(vars.get("PKG_NAME").unwrap(), &vars);
        let ver  = expand_vars(vars.get("PKG_VERSION").unwrap(), &vars);
        let src  = expand_vars(vars.get("PKG_SOURCE").unwrap(), &vars);

        assert_eq!(name, "node-modclean");
        assert_eq!(ver,  "3.0.0_beta1");
        assert_eq!(src,  "modclean-3.0.0-beta.1.tgz");
    }

    // ── Variable-based fallback source type detection ─────────────────────

    fn parse_fake(extra_vars: &[(&str, &str)]) -> SourceType {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut content = String::from("PKG_NAME:=test-pkg\nPKG_VERSION:=1.0.0\n");
        for (k, v) in extra_vars {
            content.push_str(&format!("{}:={}\n", k, v));
        }
        // Unique filename per test to avoid concurrent writes to the same file
        let mut h = DefaultHasher::new();
        content.hash(&mut h);
        let hash = h.finish();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("test_fallback_{:x}_Makefile", hash));
        std::fs::write(&path, &content).unwrap();
        let parsed = parse_makefile(std::path::Path::new(&path)).unwrap().unwrap();
        let _ = std::fs::remove_file(&path);
        parsed.source_type
    }

    #[test]
    fn test_fallback_pypi_name() {
        let st = parse_fake(&[("PYPI_NAME", "cryptography")]);
        assert!(matches!(st, SourceType::PyPI { package } if package == "cryptography"),
            "PYPI_NAME should trigger PyPI source type");
    }

    #[test]
    fn test_fallback_metacpan_name() {
        let st = parse_fake(&[("METACPAN_NAME", "URI"), ("METACPAN_AUTHOR", "OALDERS")]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "URI"),
            "METACPAN_NAME should trigger Cpan source type");
    }

    #[test]
    fn test_fallback_pecl_name() {
        let st = parse_fake(&[("PECL_NAME", "redis")]);
        assert!(matches!(st, SourceType::Pecl { package } if package == "redis"),
            "PECL_NAME should trigger Pecl source type");
    }

    #[test]
    fn test_fallback_scoped_npm() {
        let st = parse_fake(&[("PKG_NPM_SCOPE", "azure"), ("PKG_NPM_NAME", "event-hubs")]);
        assert!(matches!(st, SourceType::Npm { package } if package == "@azure/event-hubs"),
            "PKG_NPM_SCOPE + PKG_NPM_NAME should produce @scope/name");
    }

    #[test]
    fn test_fallback_unscoped_npm() {
        let st = parse_fake(&[("PKG_NPM_NAME", "lodash")]);
        assert!(matches!(st, SourceType::Npm { package } if package == "lodash"),
            "PKG_NPM_NAME alone should trigger Npm source type");
    }

    // ── GitHub releases/download and bare .git URL ────────────────────────

    #[test]
    fn test_github_releases_download_with_v_prefix() {
        // mac80211: releases/download/backports-v6.18.7 (custom prefix)
        let st = detect_source_type(
            "https://github.com/openwrt/backports/releases/download/backports-v6.18.7",
            "6.18.7", "mac80211", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, tag_template }
            if owner == "openwrt" && repo == "backports"
            && matches!(tag_template, TagTemplate::Custom(_))),
            "releases/download with custom prefix should be GitHubRelease Custom");
    }

    #[test]
    fn test_github_releases_download_plain_v() {
        // pcapplusplus: releases/download/v21.11
        let st = detect_source_type(
            "https://github.com/seladb/PcapPlusPlus/releases/download/v21.11/",
            "21.11", "pcapplusplus", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, tag_template }
            if owner == "seladb" && repo == "PcapPlusPlus"
            && matches!(tag_template, TagTemplate::WithV)),
            "releases/download with v prefix should be GitHubRelease WithV");
    }

    #[test]
    fn test_fallback_github_git_commit() {
        // lua-md5: PKG_SOURCE_URL ends in .git, PKG_SOURCE_VERSION is commit hash
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://github.com/keplerproject/md5.git"),
            ("PKG_SOURCE_VERSION", "2a98633d7587a4900cfa7cbed340f377f4acd930"),
        ]);
        assert!(matches!(st, SourceType::GitHubCommit { owner, repo, .. }
            if owner == "keplerproject" && repo == "md5"),
            "github.com/*.git + commit hash should be GitHubCommit");
    }

    #[test]
    fn test_fallback_github_git_tag() {
        // ls-mc: PKG_SOURCE_URL ends in .git, PKG_SOURCE_VERSION is tag
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://github.com/NXP/qoriq-mc-binary.git"),
            ("PKG_SOURCE_VERSION", "mc_release_10.39.0"),
        ]);
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, .. }
            if owner == "NXP" && repo == "qoriq-mc-binary"),
            "github.com/*.git + tag string should be GitHubRelease");
    }

    #[test]
    fn test_fallback_cpan_dir_url_with_source_name() {
        // perl-authen-sasl: URL is author directory, module name in PKG_SOURCE_NAME
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://www.cpan.org/authors/id/G/GB/GBARR/"),
            ("PKG_SOURCE_NAME", "Authen-SASL"),
        ]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "Authen-SASL"),
            "cpan.org directory URL + PKG_SOURCE_NAME should give Cpan");
    }

    #[test]
    fn test_fallback_cpan_metacpan_dir_url() {
        // perl-future: cpan.metacpan.org directory URL
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://cpan.metacpan.org/authors/id/P/PE/PEVANS"),
            ("PKG_SOURCE_NAME", "Future"),
        ]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "Future"),
            "cpan.metacpan.org directory URL + PKG_SOURCE_NAME should give Cpan");
    }

    #[test]
    fn test_fallback_github_bare_url_with_pkg_source_version_commit() {
        // fman-ucode: github.com/nxp-qoriq/qoriq-fm-ucode (no .git, no path)
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://github.com/nxp-qoriq/qoriq-fm-ucode"),
            ("PKG_SOURCE_VERSION", "1a2b3c4d5e6f7890abcdef1234567890abcdef12"),
        ]);
        assert!(matches!(st, SourceType::GitHubCommit { owner, repo, .. }
            if owner == "nxp-qoriq" && repo == "qoriq-fm-ucode"),
            "github.com bare URL + commit hash should be GitHubCommit");
    }

    // ── CPAN PKG_SOURCE extraction ─────────────────────────────────────────

    #[test]
    fn test_cpan_from_pkg_source() {
        // perl-cgi: PKG_SOURCE:=CGI-$(PKG_VERSION).tar.gz, URL is cpan.org dir
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://www.cpan.org/authors/id/L/LE/LEEJO"),
            ("PKG_SOURCE", "CGI-5.1.tar.gz"),
            ("PKG_VERSION", "5.1"),
        ]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "CGI"),
            "PKG_SOURCE should yield module name CGI");
    }

    #[test]
    fn test_cpan_from_pkg_source_v_prefix() {
        // perl-ack: PKG_SOURCE:=ack-v$(PKG_VERSION).tar.gz
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://www.cpan.org/authors/id/P/PE/PETDANCE/"),
            ("PKG_SOURCE", "ack-v3.7.0.tar.gz"),
            ("PKG_VERSION", "3.7.0"),
        ]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "ack"),
            "PKG_SOURCE with v-prefix should yield module name ack");
    }

    #[test]
    fn test_cpan_search_cpan_org() {
        // perl-file-rsyncp: search.cpan.org/CPAN/authors/id/.../
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://search.cpan.org/CPAN/authors/id/C/CB/CBARRATT/"),
            ("PKG_SOURCE", "File-RsyncP-0.70.tar.gz"),
            ("PKG_VERSION", "0.70"),
        ]);
        assert!(matches!(st, SourceType::Cpan { module } if module == "File-RsyncP"),
            "search.cpan.org/CPAN/authors/ should match RE_CPAN_DIR");
    }

    // ── GitLab bare .git URL ───────────────────────────────────────────────

    #[test]
    fn test_gitlab_bare_git_url_with_tag() {
        // mox-pkcs11: https://gitlab.nic.cz/turris/mox-pkcs11.git, PKG_SOURCE_VERSION=v2.0
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://gitlab.nic.cz/turris/mox-pkcs11.git"),
            ("PKG_SOURCE_VERSION", "v2.0"),
            ("PKG_VERSION", "2.0"),
        ]);
        assert!(matches!(st, SourceType::GitLab { host, owner: _, repo, tag_template }
            if host == "gitlab.nic.cz" && repo == "mox-pkcs11"
            && matches!(tag_template, TagTemplate::WithV)),
            "gitlab.<host>/*.git + v-prefixed version should be GitLab WithV");
    }

    #[test]
    fn test_gitlab_com_bare_git_url() {
        // vrx518: https://gitlab.com/prpl-foundation/intel/vrx518_aca_fw.git
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://gitlab.com/prpl-foundation/intel/vrx518_aca_fw.git"),
            ("PKG_SOURCE_VERSION", "abc1234567890abcdef1234567890abcdef123456"),
        ]);
        // Should be GitLab (commit case uses Plain tag template as fallback)
        assert!(matches!(st, SourceType::GitLab { host, repo, .. }
            if host == "gitlab.com" && repo == "vrx518_aca_fw"),
            "gitlab.com/group/subgroup/repo.git should be GitLab with correct repo name");
    }

    // ── GitHub /archive/VERSION (no extension) ────────────────────────────

    #[test]
    fn test_github_archive_no_extension() {
        // checksec: https://github.com/slimm609/checksec.sh/archive/2.5.0
        let st = detect_source_type(
            "https://github.com/slimm609/checksec.sh/archive/2.5.0",
            "2.5.0", "checksec", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, tag_template }
            if owner == "slimm609" && repo == "checksec.sh"
            && matches!(tag_template, TagTemplate::Plain)),
            "github.com/archive/<version> without extension should be GitHubRelease");
    }

    // ── golang go.dev/dl/ ─────────────────────────────────────────────────

    #[test]
    fn test_golang_dl_url() {
        let st = detect_source_type(
            "https://go.dev/dl/",
            "1.26.1", "golang1.26", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GoLang),
            "go.dev/dl/ should be GoLang");
    }

    #[test]
    fn test_golang_cn_dl_url() {
        let st = detect_source_type(
            "https://golang.google.cn/dl/",
            "1.26.1", "golang1.26", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GoLang),
            "golang.google.cn/dl/ should be GoLang");
    }

    // ── freedesktop.org ───────────────────────────────────────────────────

    #[test]
    fn test_freedesktop_gstreamer() {
        let st = detect_source_type(
            "https://gstreamer.freedesktop.org/src/gst-plugins-base",
            "1.24.0", "gst1-plugins-base", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::Freedesktop { project } if project == "gst-plugins-base"),
            "freedesktop.org/src/<name> should capture last segment as project");
    }

    // ── sources.openwrt.org → OpenWrtMirror ───────────────────────────────

    #[test]
    fn test_openwrt_sources_url() {
        // ltq-adsl-fw: https://sources.openwrt.org/
        let st = detect_source_type(
            "https://sources.openwrt.org/",
            "1.0", "ltq-adsl-fw", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::OpenWrtMirror),
            "sources.openwrt.org should map to OpenWrtMirror");
    }

    // ── googlesource.com → GoogleSource ───────────────────────────────────

    #[test]
    fn test_googlesource_adb() {
        // adb: https://android.googlesource.com/platform/system/core
        let st = detect_source_type(
            "https://android.googlesource.com/platform/system/core",
            "5.0.2", "adb", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GoogleSource { repo_url }
            if repo_url.contains("googlesource.com")),
            "googlesource.com URL should be GoogleSource");
    }

    // ── obfs4proxy: expand $(PKG_NAME) in gitlab URL ───────────────────────

    // ── codeberg.org /archive/ref (no extension) ─────────────────────────

    #[test]
    fn test_codeberg_archive_no_ext() {
        // schroot: https://codeberg.org/shelter/reschroot/archive/release
        let st = detect_source_type(
            "https://codeberg.org/shelter/reschroot/archive/release",
            "1.6.13", "schroot", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::Gitea { host, owner, repo, .. }
            if host == "codeberg.org" && owner == "shelter" && repo == "reschroot"),
            "codeberg.org /archive/<ref> without .tar should be Gitea");
    }

    #[test]
    fn test_ltq_ifxos_gitlab_ugw_version() {
        // ltq-ifxos: PKG_SOURCE_URL uses $(PKG_NAME) and $(UGW_VERSION) which both expand
        // UGW_VERSION=8.5.2.10 → URL: gitlab.com/.../ifxos/-/archive/ugw_8.5.2.10/
        let st = parse_fake(&[
            ("PKG_NAME", "ifxos"),
            ("PKG_SOURCE_URL", "https://gitlab.com/prpl-foundation/intel/$(PKG_NAME)/-/archive/ugw_$(UGW_VERSION)/"),
            ("UGW_VERSION", "8.5.2.10"),
            ("PKG_VERSION", "8.5.2.10"),
        ]);
        assert!(matches!(st, SourceType::GitLab { host, repo, .. }
            if host == "gitlab.com" && repo == "ifxos"),
            "gitlab.com /-/archive/ with UGW_VERSION should expand and match GitLab");
    }

    #[test]
    fn test_obfs4proxy_gitlab_with_pkg_name() {
        // PKG_SOURCE_URL:=https://gitlab.com/yawning/obfs4/-/archive/$(PKG_NAME)-$(PKG_VERSION)/
        // After expansion: .../archive/obfs4proxy-0.0.14/
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://gitlab.com/yawning/obfs4/-/archive/$(PKG_NAME)-$(PKG_VERSION)/"),
            ("PKG_VERSION", "0.0.14"),
        ]);
        // parse_fake sets PKG_NAME from the map; here pkg_name comes from the fake filename
        // The URL still has unexpanded $(PKG_NAME) but expand_vars should resolve it from vars
        assert!(matches!(st, SourceType::GitLab { host, owner, repo, .. }
            if host == "gitlab.com" && owner == "yawning" && repo == "obfs4"),
            "gitlab.com /-/archive/ should be GitLab even when tag contains pkg_name");
    }

    #[test]
    fn test_no_source() {
        // mwan3, travelmate: no PKG_SOURCE_URL at all
        let st = parse_fake(&[]);
        assert!(matches!(st, SourceType::NoSource),
            "empty PKG_SOURCE_URL should give NoSource");
    }

    #[test]
    fn test_openwrt_mirror() {
        // ltq-adsl, fibocom: @OPENWRT private mirror
        let st = parse_fake(&[("PKG_SOURCE_URL", "@OPENWRT")]);
        assert!(matches!(st, SourceType::OpenWrtMirror),
            "@OPENWRT should give OpenWrtMirror");
    }

    #[test]
    fn test_immortalwrt_mirror() {
        let st = parse_fake(&[("PKG_SOURCE_URL", "@IMMORTALWRT")]);
        assert!(matches!(st, SourceType::OpenWrtMirror),
            "@IMMORTALWRT should give OpenWrtMirror");
    }

    #[test]
    fn test_gnu_mirror() {
        // libtool: @GNU/libtool
        let st = parse_fake(&[("PKG_SOURCE_URL", "@GNU/libtool")]);
        assert!(matches!(st, SourceType::GnuMirror { mirror, package }
            if mirror == "GNU" && package == "libtool"),
            "@GNU/<name> should give GnuMirror");
    }

    #[test]
    fn test_gnome_mirror() {
        // libsoup3: @GNOME/libsoup/3.6
        let st = parse_fake(&[("PKG_SOURCE_URL", "@GNOME/libsoup/3.6")]);
        assert!(matches!(st, SourceType::GnuMirror { mirror, package }
            if mirror == "GNOME" && package == "libsoup"),
            "@GNOME/<name>/... should give GnuMirror with first segment");
    }

    #[test]
    fn test_apache_mirror() {
        // apr: @APACHE/apr/
        let st = parse_fake(&[("PKG_SOURCE_URL", "@APACHE/apr/")]);
        assert!(matches!(st, SourceType::GnuMirror { mirror, package }
            if mirror == "APACHE" && package == "apr"),
            "@APACHE/<name>/ should give GnuMirror");
    }

    #[test]
    fn test_custom_url() {
        // bird2: https://bird.nic.cz/download/
        let st = parse_fake(&[("PKG_SOURCE_URL", "https://bird.nic.cz/download/")]);
        assert!(matches!(st, SourceType::CustomUrl { url } if url.contains("bird.nic.cz")),
            "unrecognised HTTP URL should give CustomUrl");
    }

    #[test]
    fn test_at_sf_macro_sourceforge() {
        // dejavu-fonts-ttf: PKG_SOURCE_URL:=@SF/dejavu
        let st = parse_fake(&[("PKG_SOURCE_URL", "@SF/dejavu")]);
        assert!(matches!(st, SourceType::SourceForge { project } if project == "dejavu"),
            "@SF/<project> should map to SourceForge");
    }

    #[test]
    fn test_at_sf_macro_with_subpath() {
        // emailrelay: PKG_SOURCE_URL:=@SF/emailrelay/1.9.2
        let st = parse_fake(&[("PKG_SOURCE_URL", "@SF/emailrelay/1.9.2")]);
        assert!(matches!(st, SourceType::SourceForge { project } if project == "emailrelay"),
            "@SF/<project>/<subpath> should use first segment as project");
    }

    #[test]
    fn test_at_sf_project_prefix() {
        // libsoxr: PKG_SOURCE_URL:=@SF/project/soxr/
        let st = parse_fake(&[("PKG_SOURCE_URL", "@SF/project/soxr/")]);
        assert!(matches!(st, SourceType::SourceForge { project } if project == "soxr"),
            "@SF/project/<name> should skip 'project' and use second segment");
    }

    #[test]
    fn test_codeload_zip_url() {
        // nlohmannjson: codeload.github.com/.../zip/v1.0.0?
        let st = detect_source_type(
            "https://codeload.github.com/nlohmann/json/zip/v1.0.0",
            "1.0.0", "nlohmannjson", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, tag_template }
            if owner == "nlohmann" && repo == "json"
            && matches!(tag_template, TagTemplate::WithV)),
            "codeload zip URL should be detected as GitHubRelease");
    }

    #[test]
    fn test_bitbucket_downloads_dir() {
        // lua-bencode: bitbucket.org/wilhelmy/lua-bencode/downloads/
        let st = detect_source_type(
            "https://bitbucket.org/wilhelmy/lua-bencode/downloads/",
            "2.2.0", "lua-bencode", &HashMap::new(),
        );
        assert!(matches!(st, SourceType::BitBucket { owner, repo, .. }
            if owner == "wilhelmy" && repo == "lua-bencode"),
            "bitbucket.org/.../downloads/ should be detected as BitBucket");
    }

    #[test]
    fn test_fallback_github_bare_url_with_tag() {
        // ls-rcw: github.com/nxp-qoriq/rcw (no .git, no path), tag-like version
        let st = parse_fake(&[
            ("PKG_SOURCE_URL", "https://github.com/nxp-qoriq/rcw"),
            ("PKG_SOURCE_VERSION", "LSDK-21.08"),
        ]);
        assert!(matches!(st, SourceType::GitHubRelease { owner, repo, .. }
            if owner == "nxp-qoriq" && repo == "rcw"),
            "github.com bare URL + tag string should be GitHubRelease");
    }

    #[test]
    fn test_bcm27xx_gpu_fw_nodot_version() {
        // bcm27xx-gpu-fw: PKG_VERSION=2025.04.30
        // PKG_VERSION_REAL = 1.$(subst .,,$(PKG_VERSION)) = 1.20250430
        // PKG_SOURCE_URL = https://github.com/raspberrypi/firmware/releases/download/1.20250430
        // Expected: GitHubRelease with Custom("1.${VERSION_NODOT}") template
        let st = detect_source_type(
            "https://github.com/raspberrypi/firmware/releases/download/1.20250430",
            "2025.04.30", "bcm27xx-gpu-fw", &HashMap::new(),
        );
        assert!(
            matches!(&st, SourceType::GitHubRelease { owner, repo, tag_template }
                if owner == "raspberrypi" && repo == "firmware"
                && matches!(tag_template, TagTemplate::Custom(p) if p.contains("${VERSION_NODOT}"))),
            "bcm27xx-gpu-fw nodot URL should produce Custom(VERSION_NODOT) template, got {:?}", st
        );
    }

    // ── detect_tag_template: prefixed-tag family (tfa-layerscape / uboot-layerscape) ──

    #[test]
    fn test_detect_tag_template_lf_prefix() {
        // PKG_VERSION=6.12.20.2.0.0  PKG_SOURCE_VERSION=lf-6.12.20-2.0.0
        // numeric segments are equal → should produce Custom("lf-${VERSION}")
        let tmpl = detect_tag_template("lf-6.12.20-2.0.0", "6.12.20.2.0.0");
        assert!(
            matches!(&tmpl, TagTemplate::Custom(s) if s == "lf-${VERSION}"),
            "expected Custom(\"lf-${{VERSION}}\"), got {:?}", tmpl
        );
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
