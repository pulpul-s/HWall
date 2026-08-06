//! Optional KDE Plasma/KWin integration for persistent window position.
//!
//! Native Wayland clients cannot control global top-level coordinates. HWall
//! therefore delegates position persistence to a narrowly scoped KWin
//! `Remember` rule while continuing to own size and maximized state itself.

use crate::APPLICATION_ID;
use std::env;
use std::process::{Command, Output};

pub const MAIN_WINDOW_TITLE: &str = "HWall";

const RULE_ID: &str = "io.github.hwall.HWallPlacement";
const RULE_FILE: &str = "kwinrulesrc";
const RULE_LIST_GROUP: &str = "General";
const MISSING_VALUE: &str = "__HWALL_CONFIG_VALUE_MISSING__";

const MANAGED_RULE_ENTRIES: &[(&str, &str)] = &[
    ("Description", "HWall remembered window placement"),
    ("enabled", "true"),
    ("wmclass", APPLICATION_ID),
    ("wmclassmatch", "1"),
    ("wmclasscomplete", "false"),
    ("title", MAIN_WINDOW_TITLE),
    ("titlematch", "1"),
    ("types", "1"),
    ("positionrule", "4"),
];

pub fn plasma_window_placement_supported() -> bool {
    PlasmaTools::discover().is_some()
}

/// Synchronize the managed KWin position rule.
///
/// `allow_rule_creation` must remain false until the GTK top-level has mapped
/// at least once. Existing rules can be refreshed before the first map, while a
/// new seedless `Remember` rule is created only after KWin has placed the window.
pub fn sync_plasma_window_placement(
    enabled: bool,
    allow_rule_creation: bool,
) -> Result<bool, String> {
    let tools = PlasmaTools::discover();
    let mut changed = false;
    let mut creation_pending = false;

    if enabled {
        let Some(tools) = tools else {
            return Err(
                "KDE Plasma placement requires a Plasma session, kreadconfig6, \
                 kwriteconfig6 and qdbus"
                    .to_owned(),
            );
        };
        let rule = install_or_update_rule(allow_rule_creation)?;
        changed |= rule.changed;
        creation_pending = rule.creation_pending;
        if changed {
            tools.reconfigure()?;
        }
    } else if kconfig_tools_available() {
        changed |= remove_rule()?;
        if changed && let Some(tools) = tools {
            tools.reconfigure()?;
        }
    }

    Ok(creation_pending)
}

#[derive(Clone, Copy)]
struct PlasmaTools {
    qdbus: &'static str,
}

impl PlasmaTools {
    fn discover() -> Option<Self> {
        if !is_plasma_session() || !kconfig_tools_available() {
            return None;
        }
        ["qdbus6", "qdbus-qt6", "qdbus"]
            .into_iter()
            .find(|command| command_exists(command))
            .map(|qdbus| Self { qdbus })
    }

    fn reconfigure(self) -> Result<(), String> {
        run_output(self.qdbus, &["org.kde.KWin", "/KWin", "reconfigure"])?;
        Ok(())
    }
}

fn is_plasma_session() -> bool {
    if env::var_os("KDE_FULL_SESSION").is_some() {
        return true;
    }
    env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .map(|desktop| {
            desktop
                .split([':', ';'])
                .any(|part| part.eq_ignore_ascii_case("kde") || part.eq_ignore_ascii_case("plasma"))
        })
        .unwrap_or(false)
}

struct RuleSync {
    changed: bool,
    creation_pending: bool,
}

fn install_or_update_rule(allow_creation: bool) -> Result<RuleSync, String> {
    let mut rule_ids = read_rule_ids()?;
    let rule_exists = rule_ids.iter().any(|id| id == RULE_ID);
    if !rule_exists && !allow_creation {
        return Ok(RuleSync {
            changed: false,
            creation_pending: true,
        });
    }

    let stale_unscoped_rule = rule_exists
        && (read_config(RULE_FILE, RULE_ID, "title", MISSING_VALUE)? != MAIN_WINDOW_TITLE
            || read_config(RULE_FILE, RULE_ID, "titlematch", MISSING_VALUE)? != "1");

    let mut changed = false;
    if !rule_exists {
        rule_ids.push(RULE_ID.to_owned());
        changed = true;
    } else if stale_unscoped_rule {
        changed |= delete_config_key(RULE_FILE, RULE_ID, "position")?;
    }
    changed |= write_rule_list(&rule_ids)?;

    for &(key, value) in MANAGED_RULE_ENTRIES {
        changed |= write_config_if_changed(RULE_FILE, RULE_ID, key, value)?;
    }
    // Deliberately leave `position` absent on first creation. KWin treats an
    // unset Remember value as invalid, leaves the mapped window where it is,
    // then records the compositor-owned position through its normal rule path.
    Ok(RuleSync {
        changed,
        creation_pending: false,
    })
}

fn remove_rule() -> Result<bool, String> {
    let mut rule_ids = read_rule_ids()?;
    let original_len = rule_ids.len();
    rule_ids.retain(|id| id != RULE_ID);

    let mut changed = false;
    if rule_ids.len() != original_len {
        changed |= write_rule_list(&rule_ids)?;
    }
    for &(key, _) in MANAGED_RULE_ENTRIES {
        changed |= delete_config_key(RULE_FILE, RULE_ID, key)?;
    }
    changed |= delete_config_key(RULE_FILE, RULE_ID, "position")?;
    Ok(changed)
}

fn read_rule_ids() -> Result<Vec<String>, String> {
    let value = read_config(RULE_FILE, RULE_LIST_GROUP, "rules", "")?;
    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect())
}

fn write_rule_list(rule_ids: &[String]) -> Result<bool, String> {
    let rules = rule_ids.join(",");
    let count = rule_ids.len().to_string();
    let rules_changed = write_config_if_changed(RULE_FILE, RULE_LIST_GROUP, "rules", &rules)?;
    let count_changed = write_config_if_changed(RULE_FILE, RULE_LIST_GROUP, "count", &count)?;
    Ok(rules_changed || count_changed)
}

fn read_config(file: &str, group: &str, key: &str, default: &str) -> Result<String, String> {
    let output = run_output(
        "kreadconfig6",
        &[
            "--file",
            file,
            "--group",
            group,
            "--key",
            key,
            "--default",
            default,
        ],
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn write_config(file: &str, group: &str, key: &str, value: &str) -> Result<(), String> {
    run_output(
        "kwriteconfig6",
        &["--file", file, "--group", group, "--key", key, value],
    )?;
    Ok(())
}

fn write_config_if_changed(
    file: &str,
    group: &str,
    key: &str,
    value: &str,
) -> Result<bool, String> {
    if read_config(file, group, key, MISSING_VALUE)? == value {
        return Ok(false);
    }
    write_config(file, group, key, value)?;
    Ok(true)
}

fn delete_config_key(file: &str, group: &str, key: &str) -> Result<bool, String> {
    if read_config(file, group, key, MISSING_VALUE)? == MISSING_VALUE {
        return Ok(false);
    }
    run_output(
        "kwriteconfig6",
        &[
            "--file", file, "--group", group, "--key", key, "--delete", "",
        ],
    )?;
    Ok(true)
}

fn kconfig_tools_available() -> bool {
    command_exists("kreadconfig6") && command_exists("kwriteconfig6")
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH")
        .map(|path| env::split_paths(&path).any(|directory| directory.join(command).is_file()))
        .unwrap_or(false)
}

fn run_output(command: &str, arguments: &[&str]) -> Result<Output, String> {
    let output = Command::new(command)
        .args(arguments)
        .output()
        .map_err(|error| format!("Could not run {command}: {error}"))?;
    if output.status.success() {
        return Ok(output);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("{command} exited with {}", output.status))
    } else {
        Err(format!("{command} failed: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_rule_remembers_position_without_overwriting_it() {
        assert_eq!(RULE_ID, "io.github.hwall.HWallPlacement");
        assert!(MANAGED_RULE_ENTRIES.contains(&("enabled", "true")));
        assert!(MANAGED_RULE_ENTRIES.contains(&("title", MAIN_WINDOW_TITLE)));
        assert!(MANAGED_RULE_ENTRIES.contains(&("titlematch", "1")));
        assert!(MANAGED_RULE_ENTRIES.contains(&("positionrule", "4")));
        assert!(
            !MANAGED_RULE_ENTRIES
                .iter()
                .any(|(key, _)| *key == "position")
        );
    }
}
