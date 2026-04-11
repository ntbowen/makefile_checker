use anyhow::{Context, Result};
use chrono::Local;
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, Attribute, Cell, CellAlignment, Color, Table};
use std::path::Path;

use crate::config::OutputFormat;
use crate::i18n::{Lang, HDR_UPSTREAM_COMMIT, HDR_UPSTREAM_HASH, *};
use crate::makefile_parser::ParsedMakefile;
use crate::upstream::UpstreamInfo;

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub parsed: ParsedMakefile,
    pub upstream: UpstreamInfo,
}

pub fn print_results_table(results: &[CheckResult], lang: Lang) {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(vec![
        Cell::new(TBL_PACKAGE.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_CURRENT.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_LATEST.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_STATUS.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_TAG_COMMIT.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_BACKEND.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_HASH.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
        Cell::new(TBL_NOTE.get(lang))
            .add_attribute(Attribute::Bold)
            .fg(Color::Cyan),
    ]);

    let mut outdated = 0usize;
    let mut up_to_date = 0usize;
    let mut unknown = 0usize;
    let mut hash_mismatches = 0usize;
    let mut format_mismatches = 0usize;

    for r in results {
        let info = &r.upstream;

        let status_cell = if info.format_mismatch {
            format_mismatches += 1;
            Cell::new(STATUS_FORMAT_MISMATCH.get(lang))
                .fg(Color::Magenta)
                .add_attribute(Attribute::Bold)
        } else {
            match info.is_outdated {
                Some(true) => {
                    outdated += 1;
                    Cell::new(STATUS_OUTDATED.get(lang))
                        .fg(Color::Red)
                        .add_attribute(Attribute::Bold)
                }
                Some(false) => {
                    up_to_date += 1;
                    Cell::new(STATUS_OK.get(lang)).fg(Color::Green)
                }
                None => {
                    unknown += 1;
                    Cell::new(STATUS_UNKNOWN.get(lang)).fg(Color::Yellow)
                }
            }
        };

        let latest_str = info.latest_version.as_deref().unwrap_or("-").to_string();
        let tag_commit = match (&info.latest_tag, &info.latest_commit) {
            (Some(t), Some(c)) => format!("{} ({})", t, &c[..c.len().min(8)]),
            (Some(t), None) => t.clone(),
            (None, Some(c)) => c[..c.len().min(12)].to_string(),
            (None, None) => "-".to_string(),
        };
        let note = info.check_error.as_deref().unwrap_or("").to_string();

        let hash_cell = match info.hash_mismatch {
            Some(true) => {
                hash_mismatches += 1;
                Cell::new(HASH_MISMATCH.get(lang))
                    .fg(Color::Red)
                    .add_attribute(Attribute::Bold)
            }
            Some(false) => Cell::new(HASH_OK.get(lang)).fg(Color::Green),
            None => Cell::new(HASH_UNCHECKED.get(lang)).fg(Color::DarkGrey),
        };

        table.add_row(vec![
            Cell::new(&info.pkg_name),
            Cell::new(&info.current_version),
            Cell::new(&latest_str).set_alignment(CellAlignment::Center),
            status_cell,
            Cell::new(&tag_commit),
            Cell::new(&info.source_backend).fg(Color::DarkGrey),
            hash_cell,
            Cell::new(&note).fg(Color::DarkGrey),
        ]);
    }

    println!("{}", table);
    print!(
        "\n  {} {}  {} {}  {} {}  {} {}",
        results.len(),
        SUMMARY_CHECKED.get(lang),
        outdated.to_string().red().bold(),
        SUMMARY_OUTDATED_CNT.get(lang).red(),
        up_to_date.to_string().green(),
        SUMMARY_OK_CNT.get(lang).green(),
        unknown.to_string().yellow(),
        SUMMARY_UNKNOWN_CNT.get(lang).yellow(),
    );
    if format_mismatches > 0 {
        print!(
            "  {} {}",
            format_mismatches.to_string().magenta().bold(),
            SUMMARY_FORMAT_MISMATCH_CNT.get(lang).magenta(),
        );
    }
    if hash_mismatches > 0 {
        print!(
            "  {} {}",
            hash_mismatches.to_string().red().bold(),
            SUMMARY_HASH_MISMATCH.get(lang).red().bold(),
        );
    }
    println!();
}

pub fn print_summary(results: &[CheckResult], lang: Lang) {
    let outdated: Vec<&CheckResult> = results
        .iter()
        .filter(|r| r.upstream.is_outdated == Some(true))
        .collect();
    let mismatched: Vec<&CheckResult> = results
        .iter()
        .filter(|r| r.upstream.hash_mismatch == Some(true))
        .collect();

    // Hash mismatch warning — always show even if all up-to-date
    if !mismatched.is_empty() {
        println!(
            "\n  ⚠  {} {}:",
            mismatched.len().to_string().red().bold(),
            SUMMARY_HASH_MISMATCH.get(lang).red().bold(),
        );
        for r in &mismatched {
            println!("     {}", r.upstream.pkg_name.red().bold());
        }
    }

    if outdated.is_empty() {
        println!("\n{}", SUMMARY_ALL_OK.get(lang).green().bold());
        return;
    }

    println!(
        "\n{} {}:",
        SUMMARY_OUTDATED.get(lang).red().bold(),
        outdated.len()
    );

    for r in &outdated {
        let info = &r.upstream;
        let current = &info.current_version;
        let latest = info.latest_version.as_deref().unwrap_or("?");
        let tag = info
            .latest_tag
            .as_deref()
            .map(|t| format!(" [tag: {}]", t))
            .unwrap_or_default();
        let commit = info
            .latest_commit
            .as_deref()
            .map(|c| format!(" [commit: {}]", &c[..c.len().min(12)]))
            .unwrap_or_default();
        let hash_warn = if info.hash_mismatch == Some(true) {
            " ⚠ HASH MISMATCH".red().bold().to_string()
        } else {
            String::new()
        };

        println!(
            "  {} {} → {}{}{}{}",
            format!("{:<40}", info.pkg_name).yellow().bold(),
            current.red(),
            latest.green().bold(),
            tag.dimmed(),
            commit.dimmed(),
            hash_warn,
        );
    }
}

pub fn save_report(
    results: &[CheckResult],
    output_path: &str,
    format: &OutputFormat,
    lang: Lang,
) -> Result<()> {
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let base = Path::new(output_path);

    std::fs::create_dir_all(base).context("create output dir")?;

    match format {
        OutputFormat::Xlsx => {
            let file = base.join(format!("makefile_checker_{}.xlsx", timestamp));
            save_xlsx(results, &file, lang)?;
            println!("  {} {}", SAVE_XLSX.get(lang).green(), file.display());
        }
        OutputFormat::Csv => {
            let file = base.join(format!("makefile_checker_{}.csv", timestamp));
            save_csv(results, &file, lang)?;
            println!("  {} {}", SAVE_CSV.get(lang).green(), file.display());
        }
        OutputFormat::Both => {
            let xlsx = base.join(format!("makefile_checker_{}.xlsx", timestamp));
            let csv = base.join(format!("makefile_checker_{}.csv", timestamp));
            save_xlsx(results, &xlsx, lang)?;
            save_csv(results, &csv, lang)?;
            println!("  {} {}", SAVE_XLSX.get(lang).green(), xlsx.display());
            println!("  {} {}", SAVE_CSV.get(lang).green(), csv.display());
        }
        OutputFormat::None => {}
    }

    Ok(())
}

fn row_data(r: &CheckResult, lang: Lang) -> Vec<String> {
    let info = &r.upstream;
    let status = if info.format_mismatch {
        STATUS_FORMAT_MISMATCH.get(lang)
    } else {
        match info.is_outdated {
            Some(true) => STATUS_OUTDATED.get(lang),
            Some(false) => STATUS_OK.get(lang),
            None => STATUS_UNKNOWN.get(lang),
        }
    };
    let tag_commit = match (&info.latest_tag, &info.latest_commit) {
        (Some(t), Some(c)) => format!("{} ({})", t, &c[..c.len().min(8)]),
        (Some(t), None) => t.clone(),
        (None, Some(c)) => c[..c.len().min(12)].to_string(),
        (None, None) => String::new(),
    };

    let hash_status = match info.hash_mismatch {
        Some(true) => HASH_MISMATCH.get(lang),
        Some(false) => HASH_OK.get(lang),
        None => HASH_UNCHECKED.get(lang),
    };

    vec![
        info.pkg_name.clone(),
        r.parsed
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        info.current_version.clone(),
        info.latest_version.as_deref().unwrap_or("").to_string(),
        status.to_string(),
        tag_commit,
        info.source_backend.clone(),
        hash_status.to_string(),
        info.latest_commit
            .as_deref()
            .unwrap_or("")
            .to_string(),
        // Upstream commit (full SHA, populated after hash-fetch step)
        info.upstream_commit.as_deref().unwrap_or("").to_string(),
        // Upstream SHA-256 (populated after hash-fetch step)
        info.latest_hash_sha256.as_deref().unwrap_or("").to_string(),
        info.upstream_url.as_deref().unwrap_or("").to_string(),
        info.check_error.as_deref().unwrap_or("").to_string(),
        r.parsed.path.to_string_lossy().to_string(),
    ]
}

fn headers(lang: Lang) -> Vec<&'static str> {
    vec![
        HDR_PKG_NAME.get(lang),
        HDR_DIRECTORY.get(lang),
        HDR_CURRENT.get(lang),
        HDR_LATEST.get(lang),
        HDR_STATUS.get(lang),
        HDR_TAG_COMMIT.get(lang),
        HDR_BACKEND.get(lang),
        HDR_HASH_STATUS.get(lang),
        HDR_COMMIT_SHA.get(lang),
        HDR_UPSTREAM_COMMIT.get(lang),
        HDR_UPSTREAM_HASH.get(lang),
        HDR_UPSTREAM_URL.get(lang),
        HDR_NOTE.get(lang),
        HDR_PATH.get(lang),
    ]
}

fn save_csv(results: &[CheckResult], path: &Path, lang: Lang) -> Result<()> {
    let mut wtr = csv::Writer::from_path(path).context("create csv")?;
    wtr.write_record(&headers(lang)).context("write csv header")?;
    for r in results {
        wtr.write_record(&row_data(r, lang)).context("write csv row")?;
    }
    wtr.flush().context("flush csv")?;
    Ok(())
}

fn save_xlsx(results: &[CheckResult], path: &Path, lang: Lang) -> Result<()> {
    use rust_xlsxwriter::*;

    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.set_name(SHEET_NAME.get(lang))?;

    // Header format
    let header_fmt = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0x1F_4E_79))
        .set_font_color(Color::White)
        .set_align(FormatAlign::Center);

    // Write headers
    let hdrs = headers(lang);
    for (col, h) in hdrs.iter().enumerate() {
        sheet.write_with_format(0, col as u16, *h, &header_fmt)?;
    }

    // Row formats
    let outdated_fmt = Format::new()
        .set_background_color(Color::RGB(0xFF_C7_CE))
        .set_font_color(Color::RGB(0x9C_00_06));
    let ok_fmt = Format::new()
        .set_background_color(Color::RGB(0xC6_EF_CE))
        .set_font_color(Color::RGB(0x27_6221));
    let unknown_fmt = Format::new()
        .set_background_color(Color::RGB(0xFF_EB_9C))
        .set_font_color(Color::RGB(0x9C_65_00));
    let format_mismatch_fmt = Format::new()
        .set_background_color(Color::RGB(0xE8_D5_F5))
        .set_font_color(Color::RGB(0x6A_0D_9A));
    let default_fmt = Format::new();

    for (row_idx, r) in results.iter().enumerate() {
        let row = (row_idx + 1) as u32;
        let data = row_data(r, lang);

        let row_fmt = if r.upstream.format_mismatch {
            &format_mismatch_fmt
        } else {
            match r.upstream.is_outdated {
                Some(true) => &outdated_fmt,
                Some(false) => &ok_fmt,
                None => &unknown_fmt,
            }
        };

        // Hash mismatch: override cell 7 with red if mismatched
        let hash_mismatch_fmt = Format::new()
            .set_background_color(Color::RGB(0xFF_00_00))
            .set_font_color(Color::White)
            .set_bold();

        for (col, val) in data.iter().enumerate() {
            let fmt = if col == 4 {
                row_fmt
            } else if col == 7 && r.upstream.hash_mismatch == Some(true) {
                &hash_mismatch_fmt
            } else {
                &default_fmt
            };
            sheet.write_with_format(row, col as u16, val.as_str(), fmt)?;
        }
    }

    // Auto-fit columns (approximate)
    for col in 0..hdrs.len() {
        sheet.set_column_width(col as u16, 20.0)?;
    }
    sheet.set_column_width(9, 50.0)?; // path column wider

    workbook.save(path).context("save xlsx")?;
    Ok(())
}
