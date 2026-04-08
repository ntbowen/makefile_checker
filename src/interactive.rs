use anyhow::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Arc;

use crate::config::{Config, OutputFormat, PkgRule};
use crate::i18n::{Lang, *};
use crate::makefile_parser::{find_makefiles, parse_makefile};
use crate::makefile_updater::{update_makefile, MakefileUpdate};
use crate::reporter::{print_results_table, print_summary, save_report, CheckResult};
use crate::snapshot::Snapshot;
use crate::upstream::UpstreamChecker;

pub async fn run() -> Result<()> {
    let mut config = Config::load();

    // On very first launch (lang still default En and no saved config), ask language
    let config_path = Config::config_path();
    if !config_path.exists() {
        config.lang = select_language()?;
        config.save()?;
    }

    let lang = config.lang;

    loop {
        let action = main_menu(&config, lang)?;

        match action {
            MainAction::Run => {
                run_check(&config, lang).await?;
            }
            MainAction::Configure => {
                config = configure(config, lang)?;
                config.save()?;
                println!("  {} {}\n", "✓".green(), CFG_SAVED.get(lang));
            }
            MainAction::Quit => {
                println!("{}", match lang {
                    Lang::En => "Bye!",
                    Lang::Zh => "再见！",
                });
                break;
            }
        }
    }

    Ok(())
}

fn select_language() -> Result<Lang> {
    let theme = ColorfulTheme::default();
    let items = &["English", "中文"];
    let sel = Select::with_theme(&theme)
        .with_prompt(LANG_SELECT_PROMPT)
        .items(items)
        .default(0)
        .interact()?;
    Ok(if sel == 1 { Lang::Zh } else { Lang::En })
}

enum MainAction {
    Run,
    Configure,
    Quit,
}

fn main_menu(config: &Config, lang: Lang) -> Result<MainAction> {
    let theme = ColorfulTheme::default();

    println!(
        "  {} {}",
        STATUS_PATHS.get(lang).dimmed(),
        config.search_paths.join(", ").cyan()
    );
    let timeout_str = format!(
        "{}{}",
        config.timeout_secs,
        SECONDS_SUFFIX.get(lang)
    );
    println!(
        "  {} {}  {} {}  {} {}",
        STATUS_JOBS.get(lang).dimmed(),
        config.parallel_jobs.to_string().cyan(),
        STATUS_TIMEOUT.get(lang).dimmed(),
        timeout_str.cyan(),
        STATUS_OUTPUT.get(lang).dimmed(),
        config
            .output_path
            .as_deref()
            .unwrap_or(STATUS_NONE.get(lang))
            .cyan()
    );
    println!();

    let items = &[
        MENU_RUN.get(lang),
        MENU_CONFIGURE.get(lang),
        MENU_QUIT.get(lang),
    ];

    let selection = Select::with_theme(&theme)
        .with_prompt(MENU_PROMPT.get(lang))
        .items(items)
        .default(0)
        .interact()?;

    Ok(match selection {
        0 => MainAction::Run,
        1 => MainAction::Configure,
        _ => MainAction::Quit,
    })
}

fn configure(mut config: Config, lang: Lang) -> Result<Config> {
    let theme = ColorfulTheme::default();

    println!("\n{}\n", CFG_TITLE.get(lang).cyan().bold());

    // Search paths
    let paths_str: String = Input::with_theme(&theme)
        .with_prompt(CFG_PATHS.get(lang))
        .default(config.search_paths.join(","))
        .interact_text()?;
    config.search_paths = paths_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Parallel jobs
    let jobs: String = Input::with_theme(&theme)
        .with_prompt(CFG_JOBS.get(lang))
        .default(config.parallel_jobs.to_string())
        .interact_text()?;
    if let Ok(n) = jobs.trim().parse::<usize>() {
        if n > 0 {
            config.parallel_jobs = n;
        }
    }

    // Timeout
    let timeout: String = Input::with_theme(&theme)
        .with_prompt(CFG_TIMEOUT.get(lang))
        .default(config.timeout_secs.to_string())
        .interact_text()?;
    if let Ok(n) = timeout.trim().parse::<u64>() {
        if n > 0 {
            config.timeout_secs = n;
        }
    }

    // GitHub token
    let token: String = Input::with_theme(&theme)
        .with_prompt(CFG_TOKEN.get(lang))
        .allow_empty(true)
        .default(config.github_token.clone().unwrap_or_default())
        .interact_text()?;
    config.github_token = if token.is_empty() { None } else { Some(token) };

    // Output format
    let fmt_items = &["xlsx", "csv", "both", "none"];
    let current_fmt_idx = match config.output_format {
        OutputFormat::Xlsx => 0,
        OutputFormat::Csv => 1,
        OutputFormat::Both => 2,
        OutputFormat::None => 3,
    };
    let fmt_sel = Select::with_theme(&theme)
        .with_prompt(CFG_FORMAT.get(lang))
        .items(fmt_items)
        .default(current_fmt_idx)
        .interact()?;
    config.output_format = match fmt_sel {
        0 => OutputFormat::Xlsx,
        1 => OutputFormat::Csv,
        2 => OutputFormat::Both,
        _ => OutputFormat::None,
    };

    // Output path
    if config.output_format != OutputFormat::None {
        let out: String = Input::with_theme(&theme)
            .with_prompt(CFG_OUTDIR.get(lang))
            .default(
                config
                    .output_path
                    .clone()
                    .unwrap_or_else(|| ".".to_string()),
            )
            .interact_text()?;
        config.output_path = if out.trim().is_empty() {
            None
        } else {
            Some(out.trim().to_string())
        };
    }

    // Skip patterns (directory patterns for scanning)
    let skip_str: String = Input::with_theme(&theme)
        .with_prompt(CFG_SKIP.get(lang))
        .default(config.skip_patterns.join(","))
        .interact_text()?;
    config.skip_patterns = skip_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Retry times
    let retry_str: String = Input::with_theme(&theme)
        .with_prompt(CFG_RETRY.get(lang))
        .default(config.retry_times.to_string())
        .interact_text()?;
    if let Ok(n) = retry_str.trim().parse::<u32>() {
        config.retry_times = n;
    }

    // Skip packages (exact package names to exclude from checking)
    let skip_pkgs_str: String = Input::with_theme(&theme)
        .with_prompt(CFG_SKIP_PKGS.get(lang))
        .allow_empty(true)
        .default(config.skip_packages.join(","))
        .interact_text()?;
    config.skip_packages = skip_pkgs_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Per-package rules: show read-only summary, hint to edit config.toml
    println!("\n{}", CFG_PKG_RULES_TITLE.get(lang).cyan().bold());
    println!("  {}", CFG_PKG_RULES_LIST.get(lang).dimmed());
    if config.pkg_rules.is_empty() {
        println!("  {}", CFG_PKG_RULES_NONE.get(lang).dimmed());
    } else {
        for (pkg, rule) in &config.pkg_rules {
            let mut parts: Vec<String> = Vec::new();
            if !rule.ignore_regex.is_empty() {
                parts.push(format!("ignore_regex=[{}]", rule.ignore_regex.join(",")));
            }
            if let Some(v) = &rule.min_version { parts.push(format!("min={}", v)); }
            if let Some(v) = &rule.max_version { parts.push(format!("max={}", v)); }
            if let Some(v) = &rule.strip_prefix { parts.push(format!("strip_prefix={}", v)); }
            if let Some(v) = &rule.strip_suffix { parts.push(format!("strip_suffix={}", v)); }
            if rule.include_prerelease { parts.push("include_prerelease".to_string()); }
            if let Some(u) = &rule.url_regex_url { parts.push(format!("url_regex_url={}", u)); }
            println!("  {} {}", format!("{:<30}", pkg).yellow(), parts.join("  ").dimmed());
        }
    }
    println!("  {}", CFG_PKG_RULES_HINT.get(lang).dimmed());

    // Global pre-release toggle
    println!();
    config.include_prerelease = Confirm::with_theme(&theme)
        .with_prompt(CFG_PRERELEASE.get(lang))
        .default(config.include_prerelease)
        .interact()?;
    println!("{}", CFG_PRERELEASE_NOTE.get(lang).dimmed());

    // Download directory (dl_path)
    let dl_str: String = Input::with_theme(&theme)
        .with_prompt(CFG_DL_PATH.get(lang))
        .allow_empty(true)
        .default(config.dl_path.clone().unwrap_or_default())
        .interact_text()?;
    config.dl_path = if dl_str.trim().is_empty() { None } else { Some(dl_str.trim().to_string()) };

    // Fetch upstream hash toggle
    config.fetch_upstream_hash = Confirm::with_theme(&theme)
        .with_prompt(CFG_FETCH_HASH.get(lang))
        .default(config.fetch_upstream_hash)
        .interact()?;

    // Language
    let lang_items = &["English", "中文"];
    let cur_lang_idx = match config.lang {
        Lang::En => 0,
        Lang::Zh => 1,
    };
    let lang_sel = Select::with_theme(&theme)
        .with_prompt(LANG_SELECT_PROMPT)
        .items(lang_items)
        .default(cur_lang_idx)
        .interact()?;
    config.lang = if lang_sel == 1 { Lang::Zh } else { Lang::En };

    println!();
    Ok(config)
}

async fn run_check(config: &Config, lang: Lang) -> Result<()> {
    println!("\n{}\n", SCAN_TITLE.get(lang).cyan().bold());

    // Discover Makefiles
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap(),
    );
    spinner.set_message(SCAN_SEARCHING.get(lang));
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let makefiles = find_makefiles(&config.search_paths, &config.skip_patterns);
    spinner.finish_and_clear();

    println!(
        "  {} {} {} {}\n",
        "✓".green(),
        SCAN_FOUND.get(lang),
        makefiles.len().to_string().cyan(),
        SCAN_MAKEFILE_S.get(lang),
    );

    if makefiles.is_empty() {
        println!("  {}", SCAN_NONE.get(lang));
        return Ok(());
    }

    // Parse Makefiles
    let parse_pb = ProgressBar::new(makefiles.len() as u64);
    parse_pb.set_style(
        ProgressStyle::default_bar()
            .template(&format!(
                "[{{elapsed_precise}}] {{bar:40.cyan/blue}} {{pos}}/{{len}} {}",
                SCAN_PARSING.get(lang)
            ))
            .unwrap(),
    );

    let mut parsed_list = Vec::new();
    for path in &makefiles {
        parse_pb.inc(1);
        match parse_makefile(path) {
            Ok(Some(p)) => parsed_list.push(p),
            Ok(None) => {}
            Err(_) => {}
        }
    }
    parse_pb.finish_and_clear();

    println!(
        "  {} {} {} {}\n",
        "✓".green(),
        SCAN_PARSED.get(lang),
        parsed_list.len().to_string().cyan(),
        SCAN_VALID.get(lang),
    );

    if parsed_list.is_empty() {
        println!("  {}", SCAN_NONE_VALID.get(lang));
        return Ok(());
    }

    // Ask user if they want to filter
    let theme = ColorfulTheme::default();
    let check_all_prompt = format!(
        "{} {} {}",
        CHECK_ALL_PROMPT.get(lang),
        parsed_list.len(),
        match lang {
            Lang::En => "packages",
            Lang::Zh => "个包",
        }
    );
    let filter = Confirm::with_theme(&theme)
        .with_prompt(&check_all_prompt)
        .default(true)
        .interact()?;

    let to_check = if filter {
        parsed_list.clone()
    } else {
        let names: Vec<String> = parsed_list.iter().map(|p| p.pkg_name.clone()).collect();
        let selections = MultiSelect::with_theme(&theme)
            .with_prompt(CHECK_SELECT_PROMPT.get(lang))
            .items(&names)
            .interact()?;
        selections
            .into_iter()
            .map(|i| parsed_list[i].clone())
            .collect()
    };

    println!(
        "\n  {} {} {} {} {}...\n",
        CHECK_UPSTREAM_TITLE.get(lang),
        to_check.len().to_string().cyan(),
        match lang {
            Lang::En => "package(s) with",
            Lang::Zh => "个包，并行数",
        },
        config.parallel_jobs.to_string().cyan(),
        CHECK_PROGRESS.get(lang),
    );

    // Check upstream versions with progress
    let mp = MultiProgress::new();
    let pb = mp.add(ProgressBar::new(to_check.len() as u64));
    pb.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] {bar:40.green/white} {pos}/{len} {msg}")
            .unwrap(),
    );

    let checker = Arc::new(
        UpstreamChecker::new(
            config.github_token.as_deref(),
            config.timeout_secs,
            config.retry_times,
        )?,
    );

    // Filter skip_packages
    let skip_pkgs: std::collections::HashSet<String> =
        config.skip_packages.iter().cloned().collect();
    let to_check: Vec<_> = to_check
        .into_iter()
        .filter(|p| !skip_pkgs.contains(&p.pkg_name))
        .collect();

    let pkg_rules = Arc::new(config.pkg_rules.clone());
    let pb_arc = Arc::new(pb);
    let concurrency = config.parallel_jobs;
    let check_msg = CHECK_CHECKING.get(lang);
    let global_prerelease = config.include_prerelease;
    let empty_rule = Arc::new(PkgRule {
        include_prerelease: global_prerelease,
        ..PkgRule::default()
    });

    let results: Vec<CheckResult> = stream::iter(to_check.into_iter())
        .map(|parsed| {
            let checker = Arc::clone(&checker);
            let pb = Arc::clone(&pb_arc);
            let rules = Arc::clone(&pkg_rules);
            // Merge global include_prerelease into per-pkg rule:
            // pkg-level true always wins; otherwise fall back to global
            let rule = rules.get(&parsed.pkg_name)
                .map(|r| {
                    let mut merged = r.clone();
                    if !merged.include_prerelease {
                        merged.include_prerelease = global_prerelease;
                    }
                    Arc::new(merged)
                })
                .unwrap_or_else(|| Arc::clone(&empty_rule));
            let msg = format!("{} {}...", check_msg, parsed.pkg_name);
            async move {
                pb.set_message(msg);
                let upstream = checker.check(&parsed, &rule).await;
                pb.inc(1);
                CheckResult { parsed, upstream }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    pb_arc.finish_and_clear();

    // ── Snapshot: load previous run, ask changed-only ──
    let mut snapshot = Snapshot::load();
    let is_first_run = snapshot.versions.is_empty();

    let show_changed_only = if is_first_run {
        println!("  {}", SNAP_NEW_RUN.get(lang).dimmed());
        false
    } else {
        Confirm::with_theme(&theme)
            .with_prompt(SNAP_CHANGED_ONLY_PROMPT.get(lang))
            .default(false)
            .interact()?  
    };

    // Filter results if changed-only mode
    let display_results: Vec<CheckResult> = if show_changed_only {
        results
            .iter()
            .filter(|r| {
                if let Some(v) = &r.upstream.latest_version {
                    snapshot.has_changed(&r.upstream.pkg_name, v)
                } else {
                    false
                }
            })
            .cloned()
            .collect()
    } else {
        results.clone()
    };

    if show_changed_only && display_results.is_empty() {
        println!("\n  {}", SNAP_NO_CHANGES.get(lang).green().bold());
    } else {
        if show_changed_only {
            println!(
                "\n  {} {}",
                display_results.len().to_string().yellow().bold(),
                SNAP_CHANGED_COUNT.get(lang),
            );
        }
        println!();
        print_results_table(&display_results, lang);
        print_summary(&display_results, lang);

        // Save report if configured
        if config.output_format != OutputFormat::None {
            if let Some(out_path) = &config.output_path {
                println!();
                save_report(&display_results, out_path, &config.output_format, lang)?;
            } else {
                let save = Confirm::with_theme(&theme)
                    .with_prompt(SAVE_PROMPT.get(lang))
                    .default(true)
                    .interact()?;

                if save {
                    let path: String = Input::with_theme(&theme)
                        .with_prompt(SAVE_DIR_PROMPT.get(lang))
                        .default(".".to_string())
                        .interact_text()?;
                    println!();
                    save_report(&display_results, &path, &config.output_format, lang)?;
                }
            }
        }
    }

    // Always update snapshot with full results
    snapshot.apply_results(&results);
    if let Err(e) = snapshot.save() {
        eprintln!("  warn: could not save snapshot: {}", e);
    } else {
        println!("  {} {}", "✓".green(), SNAP_SAVED.get(lang));
    }

    // ── Post-check: offer hash fetch and Makefile update for outdated packages ──
    let outdated: Vec<&CheckResult> = results
        .iter()
        .filter(|r| r.upstream.is_outdated == Some(true))
        .collect();

    if !outdated.is_empty() {
        println!("\n  {}\n", OUTDATED_HEADER.get(lang).cyan().bold());

        // Print the outdated list so the user knows what is available
        for (i, r) in outdated.iter().enumerate() {
            let info = &r.upstream;
            let v = info.latest_version.as_deref().unwrap_or("?");
            println!(
                "  {:>3}.  {}  {}  →  {}",
                i + 1,
                format!("{:<35}", info.pkg_name).yellow().bold(),
                info.current_version.red(),
                v.green().bold(),
            );
        }
        println!();

        // Step 1: select all or pick manually
        let select_all = Confirm::with_theme(&theme)
            .with_prompt(OUTDATED_SELECT_ALL_PROMPT.get(lang))
            .default(true)
            .interact()?;

        let chosen_indices: Vec<usize> = if select_all {
            (0..outdated.len()).collect()
        } else {
            let names: Vec<String> = outdated.iter().map(|r| {
                let v = r.upstream.latest_version.as_deref().unwrap_or("?");
                format!(
                    "{:<35}  {} → {}",
                    r.upstream.pkg_name, r.upstream.current_version, v
                )
            }).collect();
            MultiSelect::with_theme(&theme)
                .with_prompt(OUTDATED_SELECT_MANUAL_PROMPT.get(lang))
                .items(&names)
                .interact()?
        };

        if chosen_indices.is_empty() {
            println!("  {}", OUTDATED_ACTION_SKIP.get(lang).dimmed());
        } else {
            let chosen: Vec<&&CheckResult> = chosen_indices.iter().map(|&i| &outdated[i]).collect();
            println!(
                "\n  {} {}",
                chosen.len().to_string().cyan().bold(),
                match lang { crate::i18n::Lang::Zh => "个包已选中", _ => "package(s) selected" },
            );

            // Step 2: choose action
            println!();
            let actions = &[
                OUTDATED_ACTION_HASH.get(lang),
                OUTDATED_ACTION_UPDATE.get(lang),
                OUTDATED_ACTION_BOTH.get(lang),
                OUTDATED_ACTION_SKIP.get(lang),
            ];
            let action_sel = Select::with_theme(&theme)
                .with_prompt(OUTDATED_ACTION_PROMPT.get(lang))
                .items(actions)
                .default(2)   // default: fetch hash THEN update
                .interact()?;

            if action_sel == 3 {
                // user chose Skip
            } else {
                let do_hash   = action_sel == 0 || action_sel == 2;
                let do_update = action_sel == 1 || action_sel == 2;

                // (pkg_name → sha256) map populated during hash fetch
                let mut hash_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                // (pkg_name → commit SHA) map populated during commit fetch
                let mut commit_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                // (pkg_name → fetch error) map populated when hash/commit fetch fails
                let mut fetch_error_map: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();

                // ── Step A: fetch commit + hash ──────────────────────────
                if do_hash {
                    let title = if do_update {
                        HASH_FETCH_TITLE.get(lang)
                    } else {
                        HASH_FETCH_TITLE_ONLY.get(lang)
                    };
                    println!("\n  {}\n", title.cyan().bold());
                    let dl_path = config.dl_path.as_deref();

                    for r in &chosen {
                        let info   = &r.upstream;
                        let pkg    = &info.pkg_name;
                        let parsed = &r.parsed;

                        // Fetch latest commit for commit-tracked packages
                        if let Some(src_ver) = &parsed.pkg_source_version {
                            if src_ver.len() >= 12 {
                                let commit_result: Result<String> = async {
                                    if let Some(url) = &parsed.source_url {
                                        let re_gh = regex::Regex::new(
                                            r"github\.com[:/]([^/]+)/([^/.\s]+)"
                                        ).unwrap();
                                        if let Some(caps) = re_gh.captures(url) {
                                            let owner = caps.get(1).map_or("", |m| m.as_str());
                                            let repo  = caps.get(2).map_or("", |m| m.as_str());
                                            return checker.fetch_latest_github_commit(owner, repo).await;
                                        }
                                        let re_gl = regex::Regex::new(
                                            r"(gitlab\.[^/]+)/([^/]+)/([^/.\s]+)"
                                        ).unwrap();
                                        if let Some(caps) = re_gl.captures(url) {
                                            let host  = caps.get(1).map_or("gitlab.com", |m| m.as_str());
                                            let owner = caps.get(2).map_or("", |m| m.as_str());
                                            let repo  = caps.get(3).map_or("", |m| m.as_str());
                                            return checker.fetch_latest_gitlab_commit(host, owner, repo).await;
                                        }
                                    }
                                    anyhow::bail!("cannot determine git host from source URL")
                                }.await;
                                match &commit_result {
                                    Ok(c) => {
                                        commit_map.insert(pkg.clone(), c.clone());
                                        println!(
                                            "  {}  {} {}",
                                            format!("{:<35}", pkg).yellow().bold(),
                                            COMMIT_FETCH_OK.get(lang),
                                            c[..c.len().min(12)].cyan(),
                                        );
                                    }
                                    Err(e) => {
                                        fetch_error_map.insert(pkg.clone(), format!("commit: {}", e));
                                        println!(
                                            "  {}  {} {}",
                                            format!("{:<35}", pkg).yellow(),
                                            COMMIT_FETCH_ERR.get(lang).red(),
                                            e.to_string().dimmed(),
                                        );
                                    }
                                }
                            }
                        }

                        // Download tarball + compute SHA-256
                        let url = match &parsed.source_url {
                            Some(u) => u,
                            None => {
                                println!(
                                    "  {}  {}",
                                    format!("{:<35}", pkg).yellow(),
                                    HASH_FETCH_NO_URL.get(lang).dimmed()
                                );
                                continue;
                            }
                        };
                        let fname = match &parsed.source_file {
                            Some(f) => f,
                            None => {
                                println!(
                                    "  {}  {}",
                                    format!("{:<35}", pkg).yellow(),
                                    HASH_FETCH_NO_FILE.get(lang).dimmed()
                                );
                                continue;
                            }
                        };
                        let new_version = match &info.latest_version {
                            Some(v) => v,
                            None => continue,
                        };
                        let new_fname = fname.replace(&parsed.pkg_version, new_version.as_str());
                        let new_url   = url.replace(&parsed.pkg_version, new_version.as_str());

                        match checker.download_and_hash(&new_url, &new_fname, dl_path).await {
                            Ok(hash) => {
                                println!(
                                    "  {}  {} {}",
                                    format!("{:<35}", pkg).yellow().bold(),
                                    HASH_FETCH_OK.get(lang),
                                    hash.cyan(),
                                );
                                hash_map.insert(pkg.to_string(), hash);
                            }
                            Err(e) => {
                                fetch_error_map.insert(pkg.clone(), format!("hash: {}", e));
                                println!(
                                    "  {}  {} {}",
                                    format!("{:<35}", pkg).yellow(),
                                    HASH_FETCH_ERR.get(lang).red(),
                                    e.to_string().dimmed(),
                                );
                            }
                        }
                    }
                }

                // ── Step B: update Makefiles ─────────────────────────────
                if do_update {
                    let title = if do_hash { UPDATE_TITLE.get(lang) } else { UPDATE_TITLE_ONLY.get(lang) };
                    println!("\n  {}\n", title.cyan().bold());

                    // Preview what will be written
                    for r in &chosen {
                        let info = &r.upstream;
                        let pkg  = &info.pkg_name;
                        let new_version = info.latest_version.as_deref().unwrap_or("?");
                        let new_hash = hash_map.get(pkg.as_str());
                        let hash_preview = new_hash
                            .map(|h| format!("  PKG_HASH → {}", &h[..h.len().min(16)]))
                            .unwrap_or_else(|| "  PKG_HASH → (not fetched, unchanged)".dimmed().to_string());
                        println!(
                            "  {}  PKG_VERSION → {}  {}",
                            format!("{:<35}", pkg).yellow().bold(),
                            new_version.green().bold(),
                            hash_preview.dimmed(),
                        );
                    }
                    println!();

                    // Confirm before writing
                    let confirmed = Confirm::with_theme(&theme)
                        .with_prompt(UPDATE_CONFIRM.get(lang))
                        .default(true)
                        .interact()?;

                    if confirmed {
                        println!();
                        for r in &chosen {
                            let info = &r.upstream;
                            let pkg  = &info.pkg_name;
                            let path = &r.parsed.path;

                            let new_version = match &info.latest_version {
                                Some(v) => v.clone(),
                                None => {
                                    println!(
                                        "  {}  {}",
                                        format!("{:<35}", pkg).yellow(),
                                        UPDATE_NOTHING.get(lang).dimmed()
                                    );
                                    continue;
                                }
                            };

                            let new_hash = hash_map.get(pkg.as_str()).cloned();

                            let upd = MakefileUpdate {
                                pkg_version: Some(new_version),
                                pkg_source_version: None,
                                pkg_hash: new_hash,
                            };

                            match update_makefile(path, &upd) {
                                Ok(changed) if !changed.is_empty() => {
                                    let bak = path.with_extension("bak");
                                    println!(
                                        "  {}  {}  {}  ({})",
                                        "✓".green(),
                                        format!("{:<35}", pkg).yellow().bold(),
                                        format!("{} {}", UPDATE_OK.get(lang), changed.join(", ").cyan()),
                                        format!("{} {}", UPDATE_BAK.get(lang), bak.display()).dimmed(),
                                    );
                                }
                                Ok(_) => println!(
                                    "  {}  {}  {}",
                                    "–".dimmed(),
                                    format!("{:<35}", pkg).yellow(),
                                    UPDATE_NOTHING.get(lang).dimmed()
                                ),
                                Err(e) => println!(
                                    "  {}  {}  {}",
                                    "✗".red(),
                                    format!("{:<35}", pkg).yellow(),
                                    format!("{} {}", UPDATE_ERR.get(lang), e).red()
                                ),
                            }
                        }
                    }
                }

                // ── Re-save report with fetched hash / commit data ────────
                if config.output_format != OutputFormat::None && do_hash {
                    let updated_results: Vec<CheckResult> = results.iter().map(|r| {
                        let mut cloned = r.clone();
                        if let Some(h) = hash_map.get(&cloned.upstream.pkg_name) {
                            cloned.upstream.latest_hash_sha256 = Some(h.clone());
                        }
                        if let Some(c) = commit_map.get(&cloned.upstream.pkg_name) {
                            cloned.upstream.upstream_commit = Some(c.clone());
                        }
                        if let Some(e) = fetch_error_map.get(&cloned.upstream.pkg_name) {
                            cloned.upstream.check_error = Some(e.clone());
                        }
                        cloned
                    }).collect();

                    let out_path = config.output_path.as_deref().unwrap_or(".");
                    if let Err(e) = save_report(&updated_results, out_path, &config.output_format, lang) {
                        eprintln!("  warn: could not re-save report with hash data: {}", e);
                    } else {
                        println!("  {} {}", "✓".green(), SNAP_SAVED.get(lang));
                    }
                }
            }
        }
    }

    println!();
    Ok(())
}
