mod config;
mod i18n;
mod makefile_parser;
mod upstream;
mod reporter;
mod snapshot;
mod interactive;

use anyhow::Result;
use colored::Colorize;

use crate::i18n::{Lang, BANNER_SUBTITLE};

#[tokio::main]
async fn main() -> Result<()> {
    println!(
        "{}",
        r#"
  __  __       _         __ _ _        ____ _               _
 |  \/  | __ _| | _____ / _(_) | ___  / ___| |__   ___  ___| | _____ _ __
 | |\/| |/ _` | |/ / _ \ |_| | |/ _ \| |   | '_ \ / _ \/ __| |/ / _ \ '__|
 | |  | | (_| |   <  __/  _| | |  __/| |___| | | |  __/ (__|   <  __/ |
 |_|  |_|\__,_|_|\_\___|_| |_|_|\___| \____|_| |_|\___|\___|_|\_\___|_|
"#
        .cyan()
        .bold()
    );

    // Show subtitle in both languages before config is loaded
    println!(
        "  {}  {}\n  {}  {}\n",
        BANNER_SUBTITLE.get(Lang::En).white(),
        "v0.1.0".dimmed(),
        BANNER_SUBTITLE.get(Lang::Zh).white(),
        "v0.1.0".dimmed(),
    );

    interactive::run().await
}
