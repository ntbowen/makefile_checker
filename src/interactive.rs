use anyhow::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, MultiSelect, Select};
use futures::stream::{self, StreamExt};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Arc;

use crate::config::{Config, OutputFormat, PkgRule};
use crate::i18n::{Lang, *};
use crate::makefile_parser::{find_makefiles, parse_makefile};
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
    let empty_rule = Arc::new(PkgRule::default());

    let results: Vec<CheckResult> = stream::iter(to_check.into_iter())
        .map(|parsed| {
            let checker = Arc::clone(&checker);
            let pb = Arc::clone(&pb_arc);
            let rules = Arc::clone(&pkg_rules);
            let rule = rules.get(&parsed.pkg_name)
                .cloned()
                .map(Arc::new)
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

    println!();
    Ok(())
}
