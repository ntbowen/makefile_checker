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

    // Pass 1: collect variable definitions (:= = ?= +=)
    // Also handle line continuations (trailing backslash)
    let mut logical_line = String::new();
    for raw in content.lines() {
        if raw.ends_with('\\') {
            logical_line.push_str(raw.trim_end_matches('\\'));
            logical_line.push(' ');
            continue;
        } else {
            logical_line.push_str(raw);
        }
        let line = logical_line.trim();
        if !line.starts_with('#') && !line.is_empty() {
            if let Some(caps) = RE_VAR_ASSIGN.captures(line) {
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
        logical_line.clear();
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
    let source_urls: Vec<String> = if expanded_source_url.is_empty() {
        vec![]
    } else {
        expanded_source_url
            .split_whitespace()
            .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
            .map(|u| u.to_string())
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

/// Fully expand a Make expression: variable refs + built-in function calls.
/// Supports up to 10 recursive passes; unknown $(shell ...) expressions are
/// left unexpanded (returned as-is) rather than causing a panic.
pub(crate) fn expand_vars(input: &str, vars: &HashMap<String, String>) -> String {
    let mut result = input.to_string();
    for _ in 0..10 {
        let expanded = expand_once(&result, vars);
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
fn expand_once(input: &str, vars: &HashMap<String, String>) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let open = bytes[i + 1];
            if open == b'(' || open == b'{' {
                let close = if open == b'(' { b')' } else { b'}' };
                // Find matching close bracket (depth-aware)
                let start = i + 2;
                let mut depth = 1usize;
                let mut j = start;
                while j < bytes.len() && depth > 0 {
                    if bytes[j] == open  { depth += 1; }
                    if bytes[j] == close { depth -= 1; }
                    if depth > 0 { j += 1; }
                }
                if depth == 0 {
                    let inner = &input[start..j];
                    let replacement = eval_make_expr(inner, vars);
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
fn eval_make_expr(inner: &str, vars: &HashMap<String, String>) -> String {
    let inner = inner.trim();

    // Detect function call: starts with a known function name followed by space/tab
    // Split only on the FIRST whitespace after the function name
    if let Some(sp) = inner.find(|c: char| c == ' ' || c == '\t') {
        let func = inner[..sp].trim();
        let rest = inner[sp..].trim();
        match func {
            "subst" => return make_subst(rest, vars),
            "patsubst" => return make_patsubst(rest, vars),
            "strip" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            "findstring" => return make_findstring(rest, vars),
            "filter" => return make_filter(rest, vars, false),
            "filter-out" => return make_filter(rest, vars, true),
            "sort" => {
                let t = expand_vars(rest, vars);
                let mut words: Vec<&str> = t.split_whitespace().collect();
                words.sort_unstable();
                words.dedup();
                return words.join(" ");
            }
            "word" => return make_word(rest, vars),
            "words" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace().count().to_string();
            }
            "wordlist" => return make_wordlist(rest, vars),
            "firstword" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace().next().unwrap_or("").to_string();
            }
            "lastword" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace().last().unwrap_or("").to_string();
            }
            "dir" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace()
                    .map(|w| match w.rfind('/') {
                        Some(p) => w[..=p].to_string(),
                        None => "./".to_string(),
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "notdir" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace()
                    .map(|w| match w.rfind('/') {
                        Some(p) => &w[p+1..],
                        None => w,
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "suffix" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace()
                    .filter_map(|w| w.rfind('.').map(|p| &w[p..]))
                    .collect::<Vec<_>>().join(" ");
            }
            "basename" => {
                let t = expand_vars(rest, vars);
                return t.split_whitespace()
                    .map(|w| match w.rfind('.') {
                        Some(p) => &w[..p],
                        None => w,
                    })
                    .collect::<Vec<_>>().join(" ");
            }
            "addsuffix" => return make_addsuffix(rest, vars),
            "addprefix" => return make_addprefix(rest, vars),
            "join" => return make_join(rest, vars),
            "if" => return make_if(rest, vars),
            "or" => return make_or(rest, vars),
            "and" => return make_and(rest, vars),
            "shell" => {
                // Do not execute — return empty string as safe fallback
                return String::new();
            }
            "call" => {
                // Basic $(call var,arg1,arg2) — expand the variable only
                let parts: Vec<&str> = rest.splitn(2, ',').collect();
                let fname = expand_vars(parts[0].trim(), vars);
                if let Some(v) = vars.get(fname.trim()) {
                    return expand_vars(v, vars);
                }
                return String::new();
            }
            "value" => {
                let vname = expand_vars(rest, vars);
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
                let expanded = expand_vars(val, vars);
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
        .map(|v| expand_vars(v, vars))
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

fn make_subst(args: &str, vars: &HashMap<String, String>) -> String {
    let (from, to, text) = split3(args);
    let from = expand_vars(from.trim(), vars);
    let to   = expand_vars(&to,   vars);  // intentionally keep internal spaces
    let text = expand_vars(text.trim(), vars);
    text.replace(from.as_str(), to.as_str())
}

fn make_patsubst(args: &str, vars: &HashMap<String, String>) -> String {
    let (pat, rep, text) = split3(args);
    let pat  = expand_vars(pat.trim(),  vars);
    let rep  = expand_vars(rep.trim(),  vars);
    let text = expand_vars(text.trim(), vars);
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

fn make_findstring(args: &str, vars: &HashMap<String, String>) -> String {
    let (find, text) = split2(args);
    let find = expand_vars(find.trim(), vars);
    let text = expand_vars(text.trim(), vars);
    if text.contains(find.as_str()) { find } else { String::new() }
}

fn make_filter(args: &str, vars: &HashMap<String, String>, invert: bool) -> String {
    let (pats_raw, text) = split2(args);
    let pats_expanded = expand_vars(pats_raw.trim(), vars);
    let pats: Vec<&str> = pats_expanded.split_whitespace().collect();
    let text = expand_vars(text.trim(), vars);
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

fn make_word(args: &str, vars: &HashMap<String, String>) -> String {
    let (n_raw, text) = split2(args);
    let n_str = expand_vars(n_raw.trim(), vars);
    let text  = expand_vars(text.trim(),  vars);
    let n: usize = n_str.trim().parse().unwrap_or(0);
    text.split_whitespace().nth(n.saturating_sub(1)).unwrap_or("").to_string()
}

fn make_wordlist(args: &str, vars: &HashMap<String, String>) -> String {
    let parts = split_args(args, 3);
    let s = expand_vars(parts.first().map(String::as_str).unwrap_or(""), vars);
    let e = expand_vars(parts.get(1).map(String::as_str).unwrap_or(""), vars);
    let text = expand_vars(parts.get(2).map(String::as_str).unwrap_or(""), vars);
    let start: usize = s.trim().parse().unwrap_or(1);
    let end:   usize = e.trim().parse().unwrap_or(0);
    text.split_whitespace()
        .enumerate()
        .filter(|(i, _)| *i + 1 >= start && *i + 1 <= end)
        .map(|(_, w)| w)
        .collect::<Vec<_>>().join(" ")
}

fn make_addsuffix(args: &str, vars: &HashMap<String, String>) -> String {
    let (suf, text) = split2(args);
    let suf  = expand_vars(suf.trim(),  vars);
    let text = expand_vars(text.trim(), vars);
    text.split_whitespace()
        .map(|w| format!("{}{}", w, suf))
        .collect::<Vec<_>>().join(" ")
}

fn make_addprefix(args: &str, vars: &HashMap<String, String>) -> String {
    let (pre, text) = split2(args);
    let pre  = expand_vars(pre.trim(),  vars);
    let text = expand_vars(text.trim(), vars);
    text.split_whitespace()
        .map(|w| format!("{}{}", pre, w))
        .collect::<Vec<_>>().join(" ")
}

fn make_join(args: &str, vars: &HashMap<String, String>) -> String {
    let (list1, list2) = split2(args);
    let l1 = expand_vars(list1.trim(), vars);
    let l2 = expand_vars(list2.trim(), vars);
    let w1: Vec<&str> = l1.split_whitespace().collect();
    let w2: Vec<&str> = l2.split_whitespace().collect();
    let len = w1.len().max(w2.len());
    (0..len)
        .map(|i| format!("{}{}", w1.get(i).unwrap_or(&""), w2.get(i).unwrap_or(&"")))
        .collect::<Vec<_>>().join(" ")
}

fn make_if(args: &str, vars: &HashMap<String, String>) -> String {
    let (cond, rest) = split2(args);
    let (then_part, else_part) = split2(&rest);
    let cond_val = expand_vars(&cond, vars);
    if !cond_val.trim().is_empty() {
        expand_vars(&then_part, vars)
    } else {
        expand_vars(&else_part, vars)
    }
}

fn make_or(args: &str, vars: &HashMap<String, String>) -> String {
    // $(or cond1,cond2,...) — return first non-empty
    let mut rest = args.to_string();
    loop {
        let (a, b) = split2(&rest);
        let v = expand_vars(&a, vars);
        if !v.trim().is_empty() { return v; }
        if b.is_empty() { return String::new(); }
        rest = b;
    }
}

fn make_and(args: &str, vars: &HashMap<String, String>) -> String {
    // $(and cond1,cond2,...) — return last if all non-empty, else ""
    let mut rest = args.to_string();
    let mut last;
    loop {
        let (a, b) = split2(&rest);
        let v = expand_vars(&a, vars);
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
    // VAR := value  |  VAR = value  |  VAR ?= value  |  VAR += value
    Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*([:?+]?=)\s*(.*)$").unwrap()
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
