use anyhow::{Context, Result};
use std::path::Path;

/// Fields that can be updated in a Makefile.
#[derive(Debug, Default)]
pub struct MakefileUpdate {
    /// New PKG_VERSION value (plain semver/date, no hash suffix)
    pub pkg_version: Option<String>,
    /// New PKG_SOURCE_VERSION (full commit SHA)
    pub pkg_source_version: Option<String>,
    /// New PKG_SOURCE_DATE (YYYY-MM-DD)
    pub pkg_source_date: Option<String>,
    /// New PKG_HASH value
    pub pkg_hash: Option<String>,
}

/// Back up `path` to `path.bak`, then apply the given updates.
/// Only variables that are present in `update` (Some) are modified.
/// Returns the list of variable names that were actually changed.
pub fn update_makefile(path: &Path, update: &MakefileUpdate) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read Makefile {}", path.display()))?;

    // Backup
    let bak = path.with_extension("bak");
    std::fs::write(&bak, &content)
        .with_context(|| format!("write backup {}", bak.display()))?;

    let mut changed: Vec<String> = Vec::new();
    let new_content = apply_updates(&content, update, &mut changed);

    std::fs::write(path, &new_content)
        .with_context(|| format!("write updated Makefile {}", path.display()))?;

    Ok(changed)
}

/// Apply updates to the Makefile text, returning the new content.
/// Records which variable names were changed into `changed`.
fn apply_updates(content: &str, update: &MakefileUpdate, changed: &mut Vec<String>) -> String {
    let mut result = String::with_capacity(content.len() + 64);

    for line in content.lines() {
        let new_line = try_replace_var(line, "PKG_VERSION", update.pkg_version.as_deref(), changed)
            .or_else(|| try_replace_var(line, "PKG_SOURCE_VERSION", update.pkg_source_version.as_deref(), changed))
            .or_else(|| try_replace_var(line, "PKG_SOURCE_DATE", update.pkg_source_date.as_deref(), changed))
            .or_else(|| try_replace_var(line, "PKG_HASH", update.pkg_hash.as_deref(), changed));

        match new_line {
            Some(l) => result.push_str(&l),
            None    => result.push_str(line),
        }
        result.push('\n');
    }

    // Preserve exact trailing newline behaviour
    if !content.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    result
}

/// If `line` sets `var_name` (`:=`, `=`, or `?=`), replace its value with
/// `new_value` and push `var_name` into `changed`.  Returns Some(new_line)
/// on match, None otherwise.
fn try_replace_var(
    line: &str,
    var_name: &str,
    new_value: Option<&str>,
    changed: &mut Vec<String>,
) -> Option<String> {
    let new_value = new_value?; // nothing to do if not requested
    // Match "VAR_NAME<spaces>[:?]?=<spaces>..."
    let stripped = line.trim_start();
    let rest = stripped.strip_prefix(var_name)?;
    let rest = rest.trim_start();
    let (op, after_op) = if let Some(r) = rest.strip_prefix(":=") {
        (":=", r)
    } else if let Some(r) = rest.strip_prefix("?=") {
        ("?=", r)
    } else if let Some(r) = rest.strip_prefix('=') {
        ("=", r)
    } else {
        return None;
    };

    // Compute leading indentation from original line
    let indent_len = line.len() - stripped.len();
    let indent = &line[..indent_len];

    // Preserve spacing between operator and value
    let spaces: &str = {
        let trimmed = after_op.trim_start();
        let n = after_op.len() - trimmed.len();
        &after_op[..n]
    };

    // Do not overwrite a value that is itself a Makefile variable reference
    // (e.g. PKG_SOURCE_VERSION:=v$(PKG_VERSION)).  Such lines are derived
    // from PKG_VERSION and must stay as-is; only PKG_VERSION itself is updated.
    let current_value = after_op.trim();
    if current_value.contains("$(") || current_value.contains("${") {
        return None;
    }

    changed.push(var_name.to_string());
    Some(format!("{}{}{}{}{}", indent, var_name, op, spaces, new_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_pkg_version() {
        let src = "PKG_VERSION:=1.2.3\n";
        let upd = MakefileUpdate { pkg_version: Some("1.3.0".into()), ..Default::default() };
        let mut ch = vec![];
        let out = apply_updates(src, &upd, &mut ch);
        assert_eq!(out, "PKG_VERSION:=1.3.0\n");
        assert_eq!(ch, ["PKG_VERSION"]);
    }

    #[test]
    fn test_replace_pkg_hash() {
        let src = "PKG_HASH:=aabbcc\n";
        let upd = MakefileUpdate { pkg_hash: Some("ddeeff".into()), ..Default::default() };
        let mut ch = vec![];
        let out = apply_updates(src, &upd, &mut ch);
        assert_eq!(out, "PKG_HASH:=ddeeff\n");
        assert_eq!(ch, ["PKG_HASH"]);
    }

    #[test]
    fn test_replace_with_spacing() {
        let src = "PKG_VERSION  :=  2.0.0\n";
        let upd = MakefileUpdate { pkg_version: Some("3.0.0".into()), ..Default::default() };
        let mut ch = vec![];
        let out = apply_updates(src, &upd, &mut ch);
        assert_eq!(out, "PKG_VERSION:=  3.0.0\n");
        assert!(ch.contains(&"PKG_VERSION".to_string()));
    }

    #[test]
    fn test_derived_source_version_not_overwritten() {
        // lua-libmodbus: PKG_SOURCE_VERSION:=v$(PKG_VERSION) is derived,
        // must not be replaced with a literal even when write_pkg_source_version is set.
        let src = "PKG_VERSION:=0.7\nPKG_SOURCE_VERSION:=v$(PKG_VERSION)\n";
        let upd = MakefileUpdate {
            pkg_version: Some("0.8".into()),
            pkg_source_version: Some("v0.8".into()),
            ..Default::default()
        };
        let mut ch = vec![];
        let out = apply_updates(src, &upd, &mut ch);
        assert!(out.contains("PKG_VERSION:=0.8"), "PKG_VERSION should update");
        assert!(out.contains("PKG_SOURCE_VERSION:=v$(PKG_VERSION)"),
            "derived PKG_SOURCE_VERSION must not be replaced with literal");
        assert!(!ch.contains(&"PKG_SOURCE_VERSION".to_string()),
            "PKG_SOURCE_VERSION must not appear in changed list");
    }

    #[test]
    fn test_no_change_when_not_requested() {
        let src = "PKG_VERSION:=1.0\nPKG_HASH:=abc\n";
        let upd = MakefileUpdate { pkg_version: Some("2.0".into()), ..Default::default() };
        let mut ch = vec![];
        let out = apply_updates(src, &upd, &mut ch);
        assert!(out.contains("PKG_VERSION:=2.0"));
        assert!(out.contains("PKG_HASH:=abc")); // unchanged
        assert_eq!(ch, ["PKG_VERSION"]);
    }
}
