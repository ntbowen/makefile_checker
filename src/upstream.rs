use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::time::Duration;

use crate::config::PkgRule;
use crate::makefile_parser::{ParsedMakefile, SourceType, TagTemplate};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct UpstreamInfo {
    pub pkg_name: String,
    pub current_version: String,
    pub latest_version: Option<String>,
    pub latest_tag: Option<String>,
    pub latest_commit: Option<String>,
    pub latest_hash_sha256: Option<String>,
    pub is_outdated: Option<bool>,
    pub upstream_url: Option<String>,
    pub check_error: Option<String>,
    /// source backend used to find this result
    pub source_backend: String,
    /// PKG_HASH mismatch: Some(true) = mismatch detected, Some(false) = ok, None = not checked
    pub hash_mismatch: Option<bool>,
}

// ─────────────────────── API response structs ─────────────────────────────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GithubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    tarball_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubTag {
    name: String,
    commit: GithubTagCommit,
}

#[derive(Debug, Deserialize)]
struct GithubTagCommit {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GithubCommit {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitLabTag {
    name: String,
    commit: GitLabCommit,
}

#[derive(Debug, Deserialize)]
struct GitLabCommit {
    id: String,
}

#[derive(Debug, Deserialize)]
struct PyPIResponse {
    info: PyPIInfo,
}

#[derive(Debug, Deserialize)]
struct PyPIInfo {
    version: String,
}

#[derive(Debug, Deserialize)]
struct RepologyPackage {
    version: String,
    status: Option<String>,
}

// ─────────────────────────── checker ──────────────────────────────────────

pub struct UpstreamChecker {
    github_client: reqwest::Client,
    plain_client: reqwest::Client,
    retry_times: u32,
}

impl UpstreamChecker {
    pub fn new(github_token: Option<&str>, timeout_secs: u64, retry_times: u32) -> Result<Self> {
        // GitHub client: GitHub-specific Accept header + optional auth
        let mut gh_headers = reqwest::header::HeaderMap::new();
        gh_headers.insert(
            reqwest::header::USER_AGENT,
            "makefile_checker/0.1".parse().unwrap(),
        );
        gh_headers.insert(
            reqwest::header::ACCEPT,
            "application/vnd.github+json".parse().unwrap(),
        );
        if let Some(token) = github_token {
            if !token.is_empty() {
                gh_headers.insert(
                    reqwest::header::AUTHORIZATION,
                    format!("Bearer {}", token).parse().unwrap(),
                );
            }
        }
        let github_client = reqwest::Client::builder()
            .default_headers(gh_headers)
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("build github http client")?;

        // Plain client: no special headers (for PyPI, Repology, SourceForge, GitLab)
        let mut plain_headers = reqwest::header::HeaderMap::new();
        plain_headers.insert(
            reqwest::header::USER_AGENT,
            "makefile_checker/0.1".parse().unwrap(),
        );
        let plain_client = reqwest::Client::builder()
            .default_headers(plain_headers)
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .context("build plain http client")?;

        Ok(Self { github_client, plain_client, retry_times })
    }

    pub async fn check(&self, parsed: &ParsedMakefile, rule: &PkgRule) -> UpstreamInfo {
        let result = self.check_with_retry(parsed, rule).await;
        let mut info = match result {
            Ok(info) => info,
            Err(e) => UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: None,
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: None,
                upstream_url: None,
                check_error: Some(e.to_string()),
                source_backend: "error".to_string(),
                hash_mismatch: None,
            },
        };
        // PKG_HASH verification: only when NOT outdated (current tarball)
        if info.is_outdated == Some(false) {
            if let (Some(pkg_hash), Some(url)) = (&parsed.pkg_hash, &parsed.source_url) {
                if let Some(fname) = &parsed.source_file {
                    info.hash_mismatch = self.verify_hash(url, fname, pkg_hash).await.ok();
                }
            }
        }
        info
    }

    async fn check_with_retry(&self, parsed: &ParsedMakefile, rule: &PkgRule) -> Result<UpstreamInfo> {
        let mut last_err = anyhow::anyhow!("no attempts");
        for attempt in 0..=self.retry_times {
            match self.check_inner(parsed, rule).await {
                Ok(info) => return Ok(info),
                Err(e) => {
                    let msg = e.to_string();
                    // Detect rate-limit (HTTP 429) or server error (5xx): wait and retry
                    let is_retryable = msg.contains("429")
                        || msg.contains("502")
                        || msg.contains("503")
                        || msg.contains("504")
                        || msg.contains("timed out")
                        || msg.contains("connection");
                    last_err = e;
                    if is_retryable && attempt < self.retry_times {
                        // Exponential back-off: 2^attempt seconds (1s, 2s, 4s)
                        let wait = Duration::from_secs(1u64 << attempt);
                        tokio::time::sleep(wait).await;
                    } else {
                        break;
                    }
                }
            }
        }
        Err(last_err)
    }

    async fn check_inner(&self, parsed: &ParsedMakefile, rule: &PkgRule) -> Result<UpstreamInfo> {
        // PkgRule url_regex override: highest priority, bypasses source detection
        if let (Some(url), Some(pattern)) = (&rule.url_regex_url, &rule.url_regex_pattern) {
            return self.check_url_regex(parsed, url, pattern).await;
        }

        let result = match &parsed.source_type {
            SourceType::GitHubRelease { owner, repo, tag_template } =>
                self.check_github_release(parsed, owner, repo, tag_template).await,
            SourceType::GitHubCommit { owner, repo, commit } =>
                self.check_github_commit(parsed, owner, repo, commit).await,
            SourceType::GitHubTagPath { owner, repo, tag_path } =>
                self.check_github_tag_path(parsed, owner, repo, tag_path).await,
            SourceType::GitLab { host, owner, repo, tag_template } =>
                self.check_gitlab(parsed, host, owner, repo, tag_template).await,
            SourceType::BitBucket { owner, repo, tag_template } =>
                self.check_bitbucket(parsed, owner, repo, tag_template).await,
            SourceType::Gitea { host, owner, repo, tag_template } =>
                self.check_gitea(parsed, host, owner, repo, tag_template).await,
            SourceType::SourceForge { project } =>
                self.check_sourceforge(parsed, project).await,
            SourceType::PyPI { package } =>
                self.check_pypi(parsed, package).await,
            SourceType::CratesIo { package } =>
                self.check_cratesio(parsed, package).await,
            SourceType::Npm { package } =>
                self.check_npm(parsed, package).await,
            SourceType::RubyGems { gem } =>
                self.check_rubygems(parsed, gem).await,
            SourceType::Hackage { package } =>
                self.check_hackage(parsed, package).await,
            SourceType::Cpan { module } =>
                self.check_cpan(parsed, module).await,
            SourceType::KernelOrg { package } =>
                self.check_kernelorg(parsed, package).await,
            SourceType::Cgit { repo_url } =>
                self.check_cgit(parsed, repo_url).await,
            SourceType::Maven { group_id, artifact_id } =>
                self.check_maven(parsed, group_id, artifact_id).await,
            SourceType::GoModule { module_path } =>
                self.check_gomodule(parsed, module_path).await,
            SourceType::UrlRegex { url, regex } =>
                self.check_url_regex(parsed, url, regex).await,
            SourceType::Unknown => Ok(self.unknown_info(parsed, "unknown source")),
        };

        // Apply PkgRule filters, then Repology → Anitya fallback
        let result = match result {
            Ok(mut info) => {
                apply_rule(&mut info, rule, parsed);
                Ok(info)
            }
            err => err,
        };

        // Repology → Anitya fallback chain: try if primary gave no version
        match result {
            Ok(ref info) if info.latest_version.is_none() && info.check_error.is_some() => {
                if let Ok(repology) = self.check_repology(parsed).await {
                    if repology.latest_version.is_some() {
                        return Ok(repology);
                    }
                }
                if let Ok(anitya) = self.check_anitya(parsed).await {
                    if anitya.latest_version.is_some() {
                        return Ok(anitya);
                    }
                }
                result
            }
            other => other,
        }
    }

    // ─────────────────────────── GitHub Release ───────────────────────────

    async fn check_github_release(
        &self,
        parsed: &ParsedMakefile,
        owner: &str,
        repo: &str,
        tag_template: &TagTemplate,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://api.github.com/repos/{}/{}/releases", owner, repo);
        let upstream_url = format!("https://github.com/{}/{}/releases", owner, repo);

        let releases: Vec<GithubRelease> = self
            .github_client
            .get(&api_url)
            .query(&[("per_page", "20")])
            .send().await.context("fetch releases")?
            .error_for_status().context("releases HTTP error")?
            .json().await.context("parse releases JSON")?;

        // Skip prerelease, draft, and pre-release tags (rc/alpha/beta/dev)
        let latest = releases
            .iter()
            .find(|r| !r.prerelease && !r.draft && !is_prerelease_tag(&r.tag_name));

        if let Some(rel) = latest {
            let tag = rel.tag_name.clone();
            let version = extract_version_from_tag(&tag, tag_template);
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag),
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "github-release".to_string(),
                hash_mismatch: None,
            });
        }

        // Fallback to tags
        self.check_github_tags_as_releases(parsed, owner, repo, tag_template, &upstream_url).await
    }

    async fn check_github_tags_as_releases(
        &self,
        parsed: &ParsedMakefile,
        owner: &str,
        repo: &str,
        tag_template: &TagTemplate,
        upstream_url: &str,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://api.github.com/repos/{}/{}/tags", owner, repo);
        let tags: Vec<GithubTag> = self
            .github_client
            .get(&api_url)
            .query(&[("per_page", "30")])
            .send().await.context("fetch tags")?
            .error_for_status().context("tags HTTP error")?
            .json().await.context("parse tags JSON")?;

        // Filter out pre-release tags
        let stable_tags: Vec<&GithubTag> = tags
            .iter()
            .filter(|t| !is_prerelease_tag(&t.name))
            .collect();

        let best = find_best_tag(&stable_tags, tag_template, &parsed.pkg_version);

        if let Some((tag, version)) = best {
            let commit = tag.commit.sha[..tag.commit.sha.len().min(8)].to_string();
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag.name.clone()),
                latest_commit: Some(commit),
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url.to_string()),
                check_error: None,
                source_backend: "github-tags".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no stable releases or tags found", upstream_url))
    }

    // ─────────────────────────── GitHub Commit ────────────────────────────

    async fn check_github_commit(
        &self,
        parsed: &ParsedMakefile,
        owner: &str,
        repo: &str,
        current_commit: &str,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://api.github.com/repos/{}/{}/commits", owner, repo);
        let upstream_url = format!("https://github.com/{}/{}/commits", owner, repo);

        let commits: Vec<GithubCommit> = self
            .github_client
            .get(&api_url)
            .query(&[("per_page", "1")])
            .send().await.context("fetch commits")?
            .error_for_status().context("commits HTTP error")?
            .json().await.context("parse commits JSON")?;

        if let Some(latest) = commits.first() {
            let latest_short = &latest.sha[..latest.sha.len().min(8)];
            let current_short = &current_commit[..current_commit.len().min(8)];
            let is_outdated = !latest.sha.starts_with(current_commit)
                && !current_commit.starts_with(&latest.sha[..]);

            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: format!("{} ({})", parsed.pkg_version, current_short),
                latest_version: Some(latest_short.to_string()),
                latest_tag: None,
                latest_commit: Some(latest.sha.clone()),
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "github-commit".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no commits found", &upstream_url))
    }

    // ─────────────────────── GitHub Tag Path ──────────────────────────────

    async fn check_github_tag_path(
        &self,
        parsed: &ParsedMakefile,
        owner: &str,
        repo: &str,
        tag_path_prefix: &str,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://api.github.com/repos/{}/{}/tags", owner, repo);
        let upstream_url = format!("https://github.com/{}/{}/tags", owner, repo);

        let tags: Vec<GithubTag> = self
            .github_client
            .get(&api_url)
            .query(&[("per_page", "30")])
            .send().await.context("fetch tags")?
            .error_for_status().context("tags HTTP error")?
            .json().await.context("parse tags JSON")?;

        let prefix = tag_path_prefix.trim_end_matches('/');
        let matching: Vec<&GithubTag> = tags
            .iter()
            .filter(|t| {
                if prefix.is_empty() { return true; }
                t.name.starts_with(&format!("{}/", prefix))
                    || t.name.starts_with(&format!("{}-", prefix))
                    || t.name.starts_with(prefix)
            })
            .filter(|t| !is_prerelease_tag(&t.name))
            .collect();

        if let Some(tag) = matching.first() {
            let version = extract_version_from_prefixed_tag(&tag.name, prefix);
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            let commit_short = tag.commit.sha[..tag.commit.sha.len().min(8)].to_string();
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag.name.clone()),
                latest_commit: Some(commit_short),
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "github-tag-path".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no matching tags found", &upstream_url))
    }

    // ───────────────────────────── GitLab ─────────────────────────────────

    async fn check_gitlab(
        &self,
        parsed: &ParsedMakefile,
        host: &str,
        owner: &str,
        repo: &str,
        tag_template: &TagTemplate,
    ) -> Result<UpstreamInfo> {
        let project_path = format!("{}/{}", owner, repo);
        let encoded = urlencoding::encode(&project_path);
        let api_url = format!("https://{}/api/v4/projects/{}/repository/tags", host, encoded);
        let upstream_url = format!("https://{}/{}/{}", host, owner, repo);

        let tags: Vec<GitLabTag> = self
            .plain_client
            .get(&api_url)
            .query(&[("per_page", "30"), ("order_by", "version"), ("sort", "desc")])
            .send().await.context("fetch gitlab tags")?
            .error_for_status().context("gitlab tags HTTP error")?
            .json().await.context("parse gitlab tags JSON")?;

        let stable: Vec<&GitLabTag> = tags
            .iter()
            .filter(|t| !is_prerelease_tag(&t.name))
            .collect();

        if let Some(tag) = stable.first() {
            let version = extract_version_from_tag(&tag.name, tag_template);
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            let commit_short = tag.commit.id[..tag.commit.id.len().min(8)].to_string();
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag.name.clone()),
                latest_commit: Some(commit_short),
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: format!("gitlab({})", host),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no stable gitlab tags", &upstream_url))
    }

    // ─────────────────────────── SourceForge ──────────────────────────────

    async fn check_sourceforge(
        &self,
        parsed: &ParsedMakefile,
        project: &str,
    ) -> Result<UpstreamInfo> {
        // SourceForge provides an RSS feed of latest files
        let rss_url = format!("https://sourceforge.net/projects/{}/rss", project);
        let upstream_url = format!("https://sourceforge.net/projects/{}/files/", project);

        let body = self
            .plain_client
            .get(&rss_url)
            .query(&[("limit", "20")])
            .send().await.context("fetch sf rss")?
            .error_for_status().context("sf rss HTTP error")?
            .text().await.context("read sf rss body")?;

        // Very simple extraction: look for version-like strings in <title> tags
        let version = extract_version_from_sf_rss(&body, &parsed.pkg_version);

        if let Some(v) = version {
            let is_outdated = compare_versions(&parsed.pkg_version, &v);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(v),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "sourceforge".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "could not parse SF version", &upstream_url))
    }

    // ──────────────────────────── PyPI ────────────────────────────────────

    async fn check_pypi(
        &self,
        parsed: &ParsedMakefile,
        package: &str,
    ) -> Result<UpstreamInfo> {
        let pkg_name = if package.is_empty() { &parsed.pkg_name } else { package };
        let api_url = format!("https://pypi.org/pypi/{}/json", pkg_name);
        let upstream_url = format!("https://pypi.org/project/{}/", pkg_name);

        let resp: PyPIResponse = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch pypi")?
            .error_for_status().context("pypi HTTP error")?
            .json().await.context("parse pypi JSON")?;

        let version = resp.info.version;
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "pypi".to_string(),
                hash_mismatch: None,
        })
    }

    // ─────────────────────────── Repology ─────────────────────────────────

    async fn check_repology(&self, parsed: &ParsedMakefile) -> Result<UpstreamInfo> {
        // Repology API: newest stable version across all repos
        let project = parsed.pkg_name.to_lowercase().replace('_', "-");
        let api_url = format!("https://repology.org/api/v1/project/{}", project);
        let upstream_url = format!("https://repology.org/project/{}/versions", project);

        let packages: Vec<RepologyPackage> = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch repology")?
            .error_for_status().context("repology HTTP error")?
            .json().await.context("parse repology JSON")?;

        // Find newest stable version
        let newest = packages
            .iter()
            .filter(|p| {
                p.status.as_deref() == Some("newest")
                    || p.status.as_deref() == Some("unique")
            })
            .max_by(|a, b| version_cmp(&a.version, &b.version));

        if let Some(pkg) = newest {
            let version = pkg.version.clone();
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "repology".to_string(),
                hash_mismatch: None,
            });
        }

        anyhow::bail!("no newest version in repology for {}", parsed.pkg_name)
    }

    // ─────────────────────────── BitBucket ────────────────────────────────

    async fn check_bitbucket(
        &self,
        parsed: &ParsedMakefile,
        owner: &str,
        repo: &str,
        tag_template: &TagTemplate,
    ) -> Result<UpstreamInfo> {
        let api_url = format!(
            "https://api.bitbucket.org/2.0/repositories/{}/{}/refs/tags",
            owner, repo
        );
        let upstream_url = format!("https://bitbucket.org/{}/{}", owner, repo);

        #[derive(Deserialize)]
        struct BbTagsResp { values: Vec<BbTag> }
        #[derive(Deserialize)]
        struct BbTag { name: String }

        let resp: BbTagsResp = self
            .plain_client
            .get(&api_url)
            .query(&[("sort", "-name"), ("pagelen", "30")])
            .send().await.context("fetch bitbucket tags")?
            .error_for_status().context("bitbucket tags HTTP error")?
            .json().await.context("parse bitbucket tags JSON")?;

        let stable: Vec<&BbTag> = resp.values.iter()
            .filter(|t| !is_prerelease_tag(&t.name))
            .collect();

        if let Some(tag) = stable.first() {
            let version = extract_version_from_tag(&tag.name, tag_template);
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag.name.clone()),
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "bitbucket".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no stable bitbucket tags", &upstream_url))
    }

    // ─────────────────────────── Gitea / Forgejo ──────────────────────────

    async fn check_gitea(
        &self,
        parsed: &ParsedMakefile,
        host: &str,
        owner: &str,
        repo: &str,
        tag_template: &TagTemplate,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://{}/api/v1/repos/{}/{}/tags", host, owner, repo);
        let upstream_url = format!("https://{}/{}/{}", host, owner, repo);

        #[derive(Deserialize)]
        struct GiteaTag { name: String }

        let tags: Vec<GiteaTag> = self
            .plain_client
            .get(&api_url)
            .query(&[("limit", "30"), ("page", "1")])
            .send().await.context("fetch gitea tags")?
            .error_for_status().context("gitea tags HTTP error")?
            .json().await.context("parse gitea tags JSON")?;

        let stable: Vec<&GiteaTag> = tags.iter()
            .filter(|t| !is_prerelease_tag(&t.name))
            .collect();

        if let Some(tag) = stable.first() {
            let version = extract_version_from_tag(&tag.name, tag_template);
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: Some(tag.name.clone()),
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: format!("gitea({})", host),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no stable gitea tags", &upstream_url))
    }

    // ─────────────────────────── crates.io ────────────────────────────────

    async fn check_cratesio(
        &self,
        parsed: &ParsedMakefile,
        package: &str,
    ) -> Result<UpstreamInfo> {
        let pkg = if package.is_empty() { &parsed.pkg_name } else { package };
        let api_url = format!("https://crates.io/api/v1/crates/{}", pkg);
        let upstream_url = format!("https://crates.io/crates/{}", pkg);

        #[derive(Deserialize)]
        struct CrateResp { #[serde(rename = "crate")] krate: CrateInfo }
        #[derive(Deserialize)]
        struct CrateInfo { newest_version: String }

        let resp: CrateResp = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch crates.io")?
            .error_for_status().context("crates.io HTTP error")?
            .json().await.context("parse crates.io JSON")?;

        let version = resp.krate.newest_version;
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "crates.io".to_string(),
                hash_mismatch: None,
        })
    }

    // ──────────────────────────── npm ─────────────────────────────────────

    async fn check_npm(
        &self,
        parsed: &ParsedMakefile,
        package: &str,
    ) -> Result<UpstreamInfo> {
        let pkg = if package.is_empty() { &parsed.pkg_name } else { package };
        let api_url = format!("https://registry.npmjs.org/{}/latest", pkg);
        let upstream_url = format!("https://www.npmjs.com/package/{}", pkg);

        #[derive(Deserialize)]
        struct NpmResp { version: String }

        let resp: NpmResp = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch npm")?
            .error_for_status().context("npm HTTP error")?
            .json().await.context("parse npm JSON")?;

        let version = resp.version;
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "npm".to_string(),
                hash_mismatch: None,
        })
    }

    // ─────────────────────────── RubyGems ─────────────────────────────────

    async fn check_rubygems(
        &self,
        parsed: &ParsedMakefile,
        gem: &str,
    ) -> Result<UpstreamInfo> {
        let name = if gem.is_empty() { &parsed.pkg_name } else { gem };
        let api_url = format!("https://rubygems.org/api/v1/gems/{}.json", name);
        let upstream_url = format!("https://rubygems.org/gems/{}", name);

        #[derive(Deserialize)]
        struct GemResp { version: String }

        let resp: GemResp = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch rubygems")?
            .error_for_status().context("rubygems HTTP error")?
            .json().await.context("parse rubygems JSON")?;

        let version = resp.version;
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "rubygems".to_string(),
                hash_mismatch: None,
        })
    }

    // ─────────────────────────── Hackage ──────────────────────────────────

    async fn check_hackage(
        &self,
        parsed: &ParsedMakefile,
        package: &str,
    ) -> Result<UpstreamInfo> {
        let pkg = if package.is_empty() { &parsed.pkg_name } else { package };
        // Hackage preferred-versions JSON
        let api_url = format!("https://hackage.haskell.org/package/{}/preferred", pkg);
        let upstream_url = format!("https://hackage.haskell.org/package/{}", pkg);

        #[derive(Deserialize)]
        struct HackageResp { #[serde(rename = "normal-version")] normal: Vec<String> }

        let resp: HackageResp = self
            .plain_client
            .get(&api_url)
            .header("Accept", "application/json")
            .send().await.context("fetch hackage")?
            .error_for_status().context("hackage HTTP error")?
            .json().await.context("parse hackage JSON")?;

        let mut versions = resp.normal;
        versions.sort_by(|a, b| version_cmp(b, a));

        if let Some(version) = versions.into_iter().next() {
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "hackage".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no hackage versions", &upstream_url))
    }

    // ──────────────────────────── CPAN ────────────────────────────────────

    async fn check_cpan(
        &self,
        parsed: &ParsedMakefile,
        module: &str,
    ) -> Result<UpstreamInfo> {
        let name = if module.is_empty() { &parsed.pkg_name } else { module };
        // MetaCPAN API
        let api_url = format!("https://fastapi.metacpan.org/v1/release/{}", name);
        let upstream_url = format!("https://metacpan.org/dist/{}", name);

        #[derive(Deserialize)]
        struct CpanResp { version: String }

        let resp: CpanResp = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch cpan")?
            .error_for_status().context("cpan HTTP error")?
            .json().await.context("parse cpan JSON")?;

        let version = resp.version.trim_start_matches('v').to_string();
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "cpan".to_string(),
                hash_mismatch: None,
        })
    }

    // ─────────────────────────── Anitya ───────────────────────────────────

    async fn check_anitya(
        &self,
        parsed: &ParsedMakefile,
    ) -> Result<UpstreamInfo> {
        // Anitya / release-monitoring.org: search by package name
        let api_url = format!(
            "https://release-monitoring.org/api/v2/projects/?name={}&distribution=Fedora",
            urlencoding::encode(&parsed.pkg_name)
        );
        let upstream_url = format!(
            "https://release-monitoring.org/projects/search/?pattern={}",
            parsed.pkg_name
        );

        #[derive(Deserialize)]
        struct AnityaResp { items: Vec<AnityaProject> }
        #[derive(Deserialize)]
        struct AnityaProject {
            latest_version: Option<String>,
            version: Option<String>,
        }

        let resp: AnityaResp = self
            .plain_client
            .get(&api_url)
            .send().await.context("fetch anitya")?
            .error_for_status().context("anitya HTTP error")?
            .json().await.context("parse anitya JSON")?;

        let version = resp.items.iter()
            .find_map(|p| p.latest_version.clone().or_else(|| p.version.clone()));

        if let Some(v) = version {
            let is_outdated = compare_versions(&parsed.pkg_version, &v);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(v),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "anitya".to_string(),
                hash_mismatch: None,
            });
        }

        anyhow::bail!("not found in anitya: {}", parsed.pkg_name)
    }

    // ─────────────────────────── helpers ──────────────────────────────────

    fn unknown_info(&self, parsed: &ParsedMakefile, reason: &str) -> UpstreamInfo {
        UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: None,
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: None,
            upstream_url: None,
            check_error: Some(reason.to_string()),
            source_backend: "unknown".to_string(),
            hash_mismatch: None,
        }
    }

    fn unknown_info_with_url(&self, parsed: &ParsedMakefile, reason: &str, url: &str) -> UpstreamInfo {
        UpstreamInfo {
            upstream_url: Some(url.to_string()),
            ..self.unknown_info(parsed, reason)
        }
    }

    // ─────────────────────────── kernel.org ───────────────────────────────

    async fn check_kernelorg(
        &self,
        parsed: &ParsedMakefile,
        package: &str,
    ) -> Result<UpstreamInfo> {
        // kernel.org JSON releases index for well-known subsystems
        // e.g. https://www.kernel.org/releases.json (kernel itself)
        // For other tarballs use the directory listing HTML
        let pkg = if package.is_empty() { &parsed.pkg_name } else { package };

        // Special case: "linux" means the kernel itself
        if pkg == "linux" || pkg == "kernel" {
            return self.check_kernelorg_kernel(parsed).await;
        }

        // For other packages: scrape https://www.kernel.org/pub/linux/utils/<pkg>/
        let index_url = format!("https://www.kernel.org/pub/linux/utils/{}/", pkg);
        let upstream_url = index_url.clone();

        let body = self
            .plain_client
            .get(&index_url)
            .send().await.context("fetch kernel.org index")?
            .error_for_status().context("kernel.org HTTP error")?
            .text().await.context("read kernel.org body")?;

        let version = extract_version_from_html_index(&body, &parsed.pkg_version);

        if let Some(v) = version {
            let is_outdated = compare_versions(&parsed.pkg_version, &v);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(v),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "kernel.org".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "could not parse kernel.org version", &upstream_url))
    }

    async fn check_kernelorg_kernel(&self, parsed: &ParsedMakefile) -> Result<UpstreamInfo> {
        let api_url = "https://www.kernel.org/releases.json";
        let upstream_url = "https://www.kernel.org/";

        #[derive(Deserialize)]
        struct KernelReleases { releases: Vec<KernelRelease> }
        #[derive(Deserialize)]
        struct KernelRelease {
            version: String,
            moniker: String,
        }

        let resp: KernelReleases = self
            .plain_client
            .get(api_url)
            .send().await.context("fetch kernel releases")?
            .error_for_status().context("kernel releases HTTP error")?
            .json().await.context("parse kernel releases JSON")?;

        // Pick latest stable release
        let stable = resp.releases.iter()
            .find(|r| r.moniker == "stable" || r.moniker == "longterm");

        if let Some(rel) = stable {
            let version = rel.version.trim_start_matches('v').to_string();
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url.to_string()),
                check_error: None,
                source_backend: "kernel.org".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no stable kernel found", upstream_url))
    }

    // ─────────────────────────── cgit / gitweb ────────────────────────────

    async fn check_cgit(
        &self,
        parsed: &ParsedMakefile,
        repo_url: &str,
    ) -> Result<UpstreamInfo> {
        // cgit tags JSON endpoint: <repo_url>/refs/?h=&format=json (newer cgit)
        // Fallback: scrape the HTML refs page
        let refs_json = format!("{}refs/?format=json", repo_url.trim_end_matches('/'));
        let refs_html = format!("{}/refs/", repo_url.trim_end_matches('/'));

        // Try JSON first (cgit >= 1.2.3)
        if let Ok(resp) = self.plain_client.get(&refs_json).send().await {
            if resp.status().is_success() {
                #[derive(Deserialize)]
                struct CgitRefsJson { tags: Vec<CgitTag> }
                #[derive(Deserialize)]
                struct CgitTag { name: String }

                if let Ok(data) = resp.json::<CgitRefsJson>().await {
                    let stable: Vec<&CgitTag> = data.tags.iter()
                        .filter(|t| !is_prerelease_tag(&t.name))
                        .collect();

                    // Sort descending and pick best
                    let mut versions: Vec<(&CgitTag, String)> = stable.iter()
                        .filter_map(|t| {
                            let v = t.name.trim_start_matches('v').to_string();
                            if v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                                Some((*t, v))
                            } else { None }
                        })
                        .collect();
                    versions.sort_by(|a, b| version_cmp(&b.1, &a.1));

                    if let Some((tag, version)) = versions.into_iter().next() {
                        let is_outdated = compare_versions(&parsed.pkg_version, &version);
                        return Ok(UpstreamInfo {
                            pkg_name: parsed.pkg_name.clone(),
                            current_version: parsed.pkg_version.clone(),
                            latest_version: Some(version),
                            latest_tag: Some(tag.name.clone()),
                            latest_commit: None,
                            latest_hash_sha256: None,
                            is_outdated: Some(is_outdated),
                            upstream_url: Some(repo_url.to_string()),
                            check_error: None,
                            source_backend: "cgit".to_string(),
                hash_mismatch: None,
                        });
                    }
                }
            }
        }

        // Fallback: HTML scraping
        let body = self.plain_client.get(&refs_html)
            .send().await.context("fetch cgit refs html")?
            .error_for_status().context("cgit refs HTTP error")?
            .text().await.context("read cgit refs body")?;

        let version = extract_version_from_html_index(&body, &parsed.pkg_version);
        if let Some(v) = version {
            let is_outdated = compare_versions(&parsed.pkg_version, &v);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(v),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(repo_url.to_string()),
                check_error: None,
                source_backend: "cgit".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "could not parse cgit tags", repo_url))
    }

    // ─────────────────────────── Maven Central ────────────────────────────

    async fn check_maven(
        &self,
        parsed: &ParsedMakefile,
        group_id: &str,
        artifact_id: &str,
    ) -> Result<UpstreamInfo> {
        let api_url = format!(
            "https://search.maven.org/solrsearch/select?q=g:%22{}%22+AND+a:%22{}%22&rows=1&wt=json",
            urlencoding::encode(group_id),
            urlencoding::encode(artifact_id)
        );
        let upstream_url = format!(
            "https://search.maven.org/artifact/{}/{}",
            group_id, artifact_id
        );

        #[derive(Deserialize)]
        struct MavenResp { response: MavenResponse }
        #[derive(Deserialize)]
        struct MavenResponse { docs: Vec<MavenDoc> }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct MavenDoc { latest_version: String }

        let resp: MavenResp = self.plain_client.get(&api_url)
            .send().await.context("fetch maven")?
            .error_for_status().context("maven HTTP error")?
            .json().await.context("parse maven JSON")?;

        if let Some(doc) = resp.response.docs.first() {
            let version = doc.latest_version.clone();
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(upstream_url),
                check_error: None,
                source_backend: "maven-central".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "maven artifact not found", &upstream_url))
    }

    // ─────────────────────────── Go module proxy ──────────────────────────

    async fn check_gomodule(
        &self,
        parsed: &ParsedMakefile,
        module_path: &str,
    ) -> Result<UpstreamInfo> {
        let api_url = format!("https://proxy.golang.org/{}/@latest", module_path);
        let upstream_url = format!("https://pkg.go.dev/{}", module_path);

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct GoLatest { version: String }

        let resp: GoLatest = self.plain_client.get(&api_url)
            .send().await.context("fetch go module")?
            .error_for_status().context("go module HTTP error")?
            .json().await.context("parse go module JSON")?;

        let version = resp.version.trim_start_matches('v').to_string();
        let is_outdated = compare_versions(&parsed.pkg_version, &version);

        Ok(UpstreamInfo {
            pkg_name: parsed.pkg_name.clone(),
            current_version: parsed.pkg_version.clone(),
            latest_version: Some(version),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(is_outdated),
            upstream_url: Some(upstream_url),
            check_error: None,
            source_backend: "go-module".to_string(),
                hash_mismatch: None,
        })
    }

    // ─────────────────────────── URL regex ────────────────────────────────

    async fn check_url_regex(
        &self,
        parsed: &ParsedMakefile,
        url: &str,
        regex: &str,
    ) -> Result<UpstreamInfo> {
        let body = self.plain_client.get(url)
            .send().await.context("fetch url-regex page")?
            .error_for_status().context("url-regex HTTP error")?
            .text().await.context("read url-regex body")?;

        let re = Regex::new(regex).context("compile url-regex pattern")?;

        // Collect all matches (capture group 1, or full match if no groups)
        let mut versions: Vec<String> = re
            .captures_iter(&body)
            .filter_map(|c| {
                c.get(1).or_else(|| c.get(0)).map(|m| m.as_str().to_string())
            })
            .filter(|v| !is_prerelease_tag(v))
            .collect();

        versions.sort_by(|a, b| version_cmp(b, a));
        versions.dedup();

        if let Some(version) = versions.into_iter().next() {
            let is_outdated = compare_versions(&parsed.pkg_version, &version);
            return Ok(UpstreamInfo {
                pkg_name: parsed.pkg_name.clone(),
                current_version: parsed.pkg_version.clone(),
                latest_version: Some(version),
                latest_tag: None,
                latest_commit: None,
                latest_hash_sha256: None,
                is_outdated: Some(is_outdated),
                upstream_url: Some(url.to_string()),
                check_error: None,
                source_backend: "url-regex".to_string(),
                hash_mismatch: None,
            });
        }

        Ok(self.unknown_info_with_url(parsed, "no version matched url-regex pattern", url))
    }

    // ─────────────────────────── PKG_HASH verify ──────────────────────────

    /// Download tarball and verify SHA-256 against PKG_HASH.
    /// Returns Ok(true) on mismatch, Ok(false) on match.
    async fn verify_hash(&self, url: &str, fname: &str, expected_hash: &str) -> Result<bool> {
        use sha2::{Digest, Sha256};
        let full_url = if url.ends_with('/') {
            format!("{}{}", url, fname)
        } else {
            format!("{}/{}", url, fname)
        };

        let bytes = self.plain_client
            .get(&full_url)
            .send().await.context("fetch tarball for hash check")?
            .error_for_status().context("tarball HTTP error")?
            .bytes().await.context("read tarball bytes")?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let digest = format!("{:x}", hasher.finalize());

        let expected = expected_hash.trim().to_lowercase();
        let expected = expected.trim_start_matches("sha256:");
        Ok(digest != expected)
    }
}

// ─────────────────────────── helper functions ─────────────────────────────

/// Apply per-package PkgRule filters to a finished UpstreamInfo:
/// strip_prefix/suffix, ignore_regex, min/max_version, include_prerelease.
fn apply_rule(info: &mut UpstreamInfo, rule: &PkgRule, parsed: &ParsedMakefile) {
    // Work on an owned copy to avoid borrow conflicts
    let mut version = match info.latest_version.clone() {
        Some(v) => v,
        None => return,
    };

    // strip_prefix / strip_suffix
    if let Some(pfx) = &rule.strip_prefix {
        if version.starts_with(pfx.as_str()) {
            version = version[pfx.len()..].to_string();
        }
    }
    if let Some(sfx) = &rule.strip_suffix {
        if version.ends_with(sfx.as_str()) {
            let new_len = version.len() - sfx.len();
            version = version[..new_len].to_string();
        }
    }

    // Pre-release check
    if !rule.include_prerelease && is_prerelease_tag(&version) {
        info.latest_version = None;
        info.is_outdated = None;
        info.check_error = Some("filtered: pre-release version".to_string());
        return;
    }

    // ignore_regex
    for pattern in &rule.ignore_regex {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(&version) {
                info.latest_version = None;
                info.is_outdated = None;
                info.check_error = Some(format!("filtered by ignore_regex: {}", pattern));
                return;
            }
        }
    }

    // min_version constraint
    if let Some(min) = &rule.min_version {
        if version_cmp(&version, min) == std::cmp::Ordering::Less {
            info.latest_version = None;
            info.is_outdated = None;
            info.check_error = Some(format!("filtered: version {} < min {}", version, min));
            return;
        }
    }

    // max_version constraint
    if let Some(max) = &rule.max_version {
        if version_cmp(&version, max) == std::cmp::Ordering::Greater {
            info.latest_version = None;
            info.is_outdated = None;
            info.check_error = Some(format!("filtered: version {} > max {}", version, max));
            return;
        }
    }

    // Write back transformed version and re-evaluate is_outdated
    info.latest_version = Some(version.clone());
    info.is_outdated = Some(compare_versions(&parsed.pkg_version, &version));
}

/// Extract the newest stable version from a kernel.org HTML directory listing.
fn extract_version_from_html_index(html: &str, current: &str) -> Option<String> {
    use std::sync::LazyLock;
    static RE_VER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"href="[^"]*?-([0-9]+\.[0-9]+(?:\.[0-9]+)*)\.tar"#).unwrap()
    });

    let mut versions: Vec<String> = RE_VER
        .captures_iter(html)
        .map(|c| c[1].to_string())
        .filter(|v| !is_prerelease_tag(v))
        .collect();

    versions.sort_by(|a, b| version_cmp(b, a));
    versions.into_iter().next().filter(|v| v != current)
}

/// Returns true if a tag name looks like a pre-release
/// (contains rc, alpha, beta, dev, pre, nightly, snapshot, test, next)
pub fn is_prerelease_tag(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    // Patterns: -rc1, .rc2, _alpha, -beta3, -dev, .pre, etc.
    for kw in &["alpha", "beta", "rc", "dev", "pre", "nightly", "snapshot", "test", "next", "preview"] {
        // Must be preceded by non-alphanumeric or start, followed by non-alpha or end
        if let Some(pos) = lower.find(kw) {
            let before = if pos == 0 { b'.' } else { lower.as_bytes()[pos - 1] };
            let after_pos = pos + kw.len();
            let after = if after_pos >= lower.len() { b'.' } else { lower.as_bytes()[after_pos] };
            if !before.is_ascii_alphanumeric() || !after.is_ascii_alphabetic() {
                return true;
            }
        }
    }
    false
}

fn extract_version_from_tag(tag: &str, template: &TagTemplate) -> String {
    match template {
        TagTemplate::WithV => tag.trim_start_matches('v').to_string(),
        TagTemplate::Plain => tag.to_string(),
        TagTemplate::Custom(pattern) => {
            let before_var = pattern.split("${VERSION}").next().unwrap_or("");
            let after_var = pattern.split("${VERSION}").nth(1).unwrap_or("");
            let mut v = tag.to_string();
            if !before_var.is_empty() && v.starts_with(before_var) {
                v = v[before_var.len()..].to_string();
            }
            if !after_var.is_empty() && v.ends_with(after_var) {
                v = v[..v.len() - after_var.len()].to_string();
            }
            v.trim_start_matches('v').to_string()
        }
    }
}

fn extract_version_from_prefixed_tag(tag: &str, prefix: &str) -> String {
    if prefix.is_empty() {
        return tag.trim_start_matches('v').to_string();
    }
    for sep in &['/', '-'] {
        let pref = format!("{}{}", prefix, sep);
        if tag.starts_with(&pref) {
            return tag[pref.len()..].trim_start_matches('v').to_string();
        }
    }
    tag.to_string()
}

fn find_best_tag<'a>(
    tags: &[&'a GithubTag],
    template: &TagTemplate,
    _current_version: &str,
) -> Option<(&'a GithubTag, String)> {
    let mut candidates: Vec<(&GithubTag, String)> = tags
        .iter()
        .filter_map(|t| {
            let version = extract_version_from_tag(&t.name, template);
            let v = version.trim_start_matches('v');
            if v.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                Some((*t, version))
            } else {
                None
            }
        })
        .collect();

    candidates.sort_by(|a, b| version_cmp(&b.1, &a.1));
    candidates.into_iter().next()
}

/// Extract version from SourceForge RSS feed body
fn extract_version_from_sf_rss(body: &str, current: &str) -> Option<String> {
    use std::sync::LazyLock;
    use regex::Regex;
    static RE_SF_TITLE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"/([0-9]+\.[0-9]+(?:\.[0-9]+)*)/").unwrap()
    });

    // Collect all version candidates from RSS <title> lines
    let mut versions: Vec<String> = RE_SF_TITLE
        .captures_iter(body)
        .map(|c| c[1].to_string())
        .filter(|v| !is_prerelease_tag(v))
        .collect();

    versions.sort_by(|a, b| version_cmp(b, a));
    versions.into_iter().next().filter(|v| v != current)
}

/// Simple version comparison: returns true if `latest` is newer than `current`.
pub fn compare_versions(current: &str, latest: &str) -> bool {
    let cv = normalize_version(current);
    let lv = normalize_version(latest);

    if let (Ok(c), Ok(l)) = (semver::Version::parse(&cv), semver::Version::parse(&lv)) {
        return l > c;
    }

    version_cmp(&lv, &cv) == std::cmp::Ordering::Greater
}

fn normalize_version(v: &str) -> String {
    let v = v.trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    match parts.len() {
        1 => format!("{}.0.0", parts[0]),
        2 => format!("{}.{}.0", parts[0], parts[1]),
        _ => {
            // Only keep first 3 numeric-ish segments to satisfy semver parser
            let clean: Vec<&str> = parts[..3.min(parts.len())]
                .iter()
                .map(|p| p.trim_end_matches(|c: char| !c.is_ascii_digit()))
                .collect();
            clean.join(".")
        }
    }
}

pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let av = normalize_version(a);
    let bv = normalize_version(b);

    if let (Ok(va), Ok(vb)) = (semver::Version::parse(&av), semver::Version::parse(&bv)) {
        return va.cmp(&vb);
    }

    let a_parts: Vec<u64> = av.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let b_parts: Vec<u64> = bv.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let av = a_parts.get(i).copied().unwrap_or(0);
        let bv = b_parts.get(i).copied().unwrap_or(0);
        if av != bv {
            return av.cmp(&bv);
        }
    }
    std::cmp::Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PkgRule;
    use crate::makefile_parser::{ParsedMakefile, SourceType};
    use std::path::PathBuf;

    fn make_parsed(current: &str) -> ParsedMakefile {
        ParsedMakefile {
            path: PathBuf::from("/tmp/Makefile"),
            pkg_name: "testpkg".to_string(),
            pkg_version: current.to_string(),
            pkg_release: None,
            source_url: None,
            source_urls: vec![],
            source_file: None,
            pkg_hash: None,
            pkg_source_date: None,
            pkg_source_version: None,
            source_type: SourceType::Unknown,
            raw_vars: Default::default(),
        }
    }

    fn make_info(current: &str, latest: &str) -> UpstreamInfo {
        UpstreamInfo {
            pkg_name: "testpkg".to_string(),
            current_version: current.to_string(),
            latest_version: Some(latest.to_string()),
            latest_tag: None,
            latest_commit: None,
            latest_hash_sha256: None,
            is_outdated: Some(compare_versions(current, latest)),
            upstream_url: None,
            check_error: None,
            source_backend: "test".to_string(),
            hash_mismatch: None,
        }
    }

    // ── compare_versions ─────────────────────────────────────────────────

    #[test]
    fn test_compare_newer() {
        assert!(compare_versions("1.0.0", "1.0.1"));
        assert!(compare_versions("1.0.0", "2.0.0"));
        assert!(compare_versions("1.9.9", "2.0.0"));
        assert!(compare_versions("0.9", "1.0"));
    }

    #[test]
    fn test_compare_same() {
        assert!(!compare_versions("1.0.0", "1.0.0"));
        assert!(!compare_versions("2.5", "2.5"));
    }

    #[test]
    fn test_compare_older() {
        assert!(!compare_versions("2.0.0", "1.9.9"));
        assert!(!compare_versions("1.0.1", "1.0.0"));
    }

    #[test]
    fn test_version_cmp_ordering() {
        use std::cmp::Ordering;
        assert_eq!(version_cmp("1.0.0", "1.0.1"), Ordering::Less);
        assert_eq!(version_cmp("1.0.1", "1.0.0"), Ordering::Greater);
        assert_eq!(version_cmp("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(version_cmp("10.0", "9.9"), Ordering::Greater);
    }

    // ── is_prerelease_tag ─────────────────────────────────────────────────

    #[test]
    fn test_prerelease_detected() {
        assert!(is_prerelease_tag("1.0.0-rc1"));
        assert!(is_prerelease_tag("v2.0.0-alpha"));
        assert!(is_prerelease_tag("3.0.0-beta2"));
        assert!(is_prerelease_tag("1.0.0-dev"));
        assert!(is_prerelease_tag("1.0.0-nightly"));
        assert!(is_prerelease_tag("1.0.0.pre"));
    }

    #[test]
    fn test_prerelease_not_detected() {
        assert!(!is_prerelease_tag("1.0.0"));
        assert!(!is_prerelease_tag("v2.5.3"));
        assert!(!is_prerelease_tag("20240101"));
        assert!(!is_prerelease_tag("2.38.1"));
    }

    // ── apply_rule ────────────────────────────────────────────────────────

    #[test]
    fn test_apply_rule_strip_prefix() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "release-1.2.4");
        let rule = PkgRule {
            strip_prefix: Some("release-".to_string()),
            ..Default::default()
        };
        apply_rule(&mut info, &rule, &parsed);
        assert_eq!(info.latest_version.as_deref(), Some("1.2.4"));
    }

    #[test]
    fn test_apply_rule_strip_suffix() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "1.2.4-stable");
        let rule = PkgRule {
            strip_suffix: Some("-stable".to_string()),
            ..Default::default()
        };
        apply_rule(&mut info, &rule, &parsed);
        assert_eq!(info.latest_version.as_deref(), Some("1.2.4"));
    }

    #[test]
    fn test_apply_rule_prerelease_filtered() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "1.3.0-rc1");
        let rule = PkgRule::default(); // include_prerelease = false
        apply_rule(&mut info, &rule, &parsed);
        assert!(info.latest_version.is_none());
        assert!(info.check_error.as_deref().unwrap_or("").contains("pre-release"));
    }

    #[test]
    fn test_apply_rule_prerelease_allowed() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "1.3.0-rc1");
        let rule = PkgRule { include_prerelease: true, ..Default::default() };
        apply_rule(&mut info, &rule, &parsed);
        assert_eq!(info.latest_version.as_deref(), Some("1.3.0-rc1"));
    }

    #[test]
    fn test_apply_rule_ignore_regex() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "2.0.0");
        let rule = PkgRule {
            ignore_regex: vec!["^2\\..*".to_string()],
            ..Default::default()
        };
        apply_rule(&mut info, &rule, &parsed);
        assert!(info.latest_version.is_none());
        assert!(info.check_error.as_deref().unwrap_or("").contains("ignore_regex"));
    }

    #[test]
    fn test_apply_rule_min_version() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "1.0.0");
        let rule = PkgRule {
            min_version: Some("2.0.0".to_string()),
            ..Default::default()
        };
        apply_rule(&mut info, &rule, &parsed);
        assert!(info.latest_version.is_none());
        assert!(info.check_error.as_deref().unwrap_or("").contains("min"));
    }

    #[test]
    fn test_apply_rule_max_version() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "5.0.0");
        let rule = PkgRule {
            max_version: Some("3.0.0".to_string()),
            ..Default::default()
        };
        apply_rule(&mut info, &rule, &parsed);
        assert!(info.latest_version.is_none());
        assert!(info.check_error.as_deref().unwrap_or("").contains("max"));
    }

    #[test]
    fn test_apply_rule_passthrough() {
        let parsed = make_parsed("1.2.3");
        let mut info = make_info("1.2.3", "1.2.4");
        let rule = PkgRule::default();
        apply_rule(&mut info, &rule, &parsed);
        // 1.2.4 > 1.2.3, should remain and be marked outdated
        assert_eq!(info.latest_version.as_deref(), Some("1.2.4"));
        assert_eq!(info.is_outdated, Some(true));
    }
}
