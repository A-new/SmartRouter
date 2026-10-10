//! Windows PATH hydration from the registry.
//!
//! A GUI-launched process inherits a PATH that can be MISSING the per-user
//! dirs where CLI agents install — npm global (`%APPDATA%\npm`), pnpm, bun,
//! cargo, scoop shims, volta, nvm-windows, mise, the Anthropic native
//! installer in `~/.local/bin`, etc. SmartRouter's per-tool `paths.win32`
//! candidate lists (plus the `~/.smartrouter/tool-paths.json` override) cover
//! many of these, but they're a maintained guess — every new install method
//! needs another entry and scoop/volta/nvm/mise are still routinely missed.
//!
//! The registry is the source of truth a fresh `cmd.exe` sees, so we read
//! HKLM + HKCU `Path` and merge those dirs into the process PATH. This is
//! the Windows analog of `utils::platform::shell_command_path` (which sources
//! the login shell's PATH on Unix). Append-only + existence-gated: it can
//! only make more tools resolvable, never removes or shadows anything — a
//! harmless no-op when the inherited PATH was already complete.
//!
//! Ported from Coffee-CLI's `src/windows_path.rs` (commit 38cfa33), adapted
//! to SmartRouter's existing `winreg` crate (no new dependency) and to
//! `utils::platform::expand_path` — `winreg`'s `FromRegValue for String` does NOT
//! auto-expand `REG_EXPAND_SZ`, so `%USERPROFILE%` etc. must be expanded per
//! entry by the shared helper.

use std::collections::HashSet;

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::{RegKey, HKEY};

use crate::utils::platform::expand_path;

/// Read the real PATH (HKLM + HKCU `Path`) and merge any missing entries into
/// the process PATH. Append-only + existence-gated; no-op on error or when
/// PATH is already complete. Called once at GUI startup, before `scan_tools`.
pub(crate) fn hydrate() {
    let candidates = real_path_dirs();
    if candidates.is_empty() {
        return;
    }
    let current = std::env::var("PATH").unwrap_or_default();
    let (merged, added) = merge_into_path(&current, &candidates);
    if added > 0 {
        // `set_var` is `unsafe` from edition 2024 (cross-thread env races);
        // we run single-threaded at GUI startup, before the Tauri runtime
        // spawns any threads — see `lib.rs::run()`.
        unsafe {
            std::env::set_var("PATH", merged);
        }
        log::info!(
            "[windows_path] hydrated {} dir(s) from the registry PATH into the process PATH",
            added
        );
    }
}

/// Existing dirs from the real (registry) PATH — system entries first, then
/// user entries (matches a fresh `cmd.exe`'s merge order). Empty on error.
fn real_path_dirs() -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    dirs.extend(split_existing(&read_path_value(
        HKEY_LOCAL_MACHINE,
        "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment",
        "Path",
    )));
    dirs.extend(split_existing(&read_path_value(
        HKEY_CURRENT_USER,
        "Environment",
        "Path",
    )));
    dirs
}

/// Split a PATH string into existing dir strings. Each entry is `%ENV%`-
/// expanded (registry `Path` is `REG_EXPAND_SZ`, e.g.
/// `%USERPROFILE%\AppData\Local\Microsoft\WindowsApps`) and dropped if the dir
/// doesn't exist on disk, so stale registry entries never pollute the PATH.
fn split_existing(path: &Option<String>) -> Vec<String> {
    match path {
        Some(s) if !s.is_empty() => s
            .split(';')
            .map(|e| e.trim())
            .filter(|e| !e.is_empty())
            .filter_map(|e| {
                let p = expand_path(e);
                if p.is_dir() {
                    Some(p.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Append-only merge: add `candidates` not already present in `current`.
/// Comparison is case-insensitive and trailing-`\`-trimmed (Windows PATH
/// semantics). Duplicates among `candidates` are deduped. Returns the merged
/// string plus the count of entries actually appended. Pure / testable.
fn merge_into_path(current: &str, candidates: &[String]) -> (String, usize) {
    let mut present: HashSet<String> = current.split(';').map(normalize).collect();
    let mut additions: Vec<String> = Vec::new();
    for s in candidates {
        let n = normalize(s);
        if n.is_empty() || present.contains(&n) {
            continue;
        }
        present.insert(n);
        additions.push(s.clone());
    }
    let added = additions.len();
    if added == 0 {
        return (current.to_string(), 0);
    }
    let joined = additions.join(";");
    let merged = if current.is_empty() {
        joined
    } else {
        format!("{current};{joined}")
    };
    (merged, added)
}

/// Case-insensitive, trimmed, trailing-`\`-stripped key for PATH dedup.
fn normalize(entry: &str) -> String {
    entry.trim().trim_end_matches('\\').to_ascii_lowercase()
}

/// Read a registry `Path` value (`REG_SZ` or `REG_EXPAND_SZ`). `winreg` does
/// NOT auto-expand `REG_EXPAND_SZ` — its `FromRegValue for String` returns the
/// raw string with `%USERPROFILE%` literal — so `split_existing` expands each
/// entry via `utils::platform::expand_path`. Returns None on any error / missing
/// key (hydration is best-effort).
fn read_path_value(root: HKEY, subkey: &str, value: &str) -> Option<String> {
    let key = RegKey::predef(root);
    let env = key.open_subkey_with_flags(subkey, KEY_READ).ok()?;
    let path: String = env.get_value(value).ok()?;
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_current_joins_all_candidates() {
        let (merged, added) = merge_into_path("", &["C:\\foo".into(), "D:\\bar".into()]);
        assert_eq!(merged, "C:\\foo;D:\\bar");
        assert_eq!(added, 2);
    }

    #[test]
    fn already_present_skipped_case_insensitive_and_trailing_slash() {
        // "C:\\Foo\\" normalizes to "c:\\foo" — candidate "c:\\foo" matches.
        let (merged, added) = merge_into_path("C:\\Foo\\", &["c:\\foo".into()]);
        assert_eq!(merged, "C:\\Foo\\");
        assert_eq!(added, 0);
    }

    #[test]
    fn duplicate_candidates_deduped() {
        let (merged, added) = merge_into_path("", &["D:\\bar".into(), "D:\\bar".into()]);
        assert_eq!(merged, "D:\\bar");
        assert_eq!(added, 1);
    }

    #[test]
    fn empty_candidates_returns_current_unchanged() {
        let (merged, added) = merge_into_path("C:\\Windows", &[]);
        assert_eq!(merged, "C:\\Windows");
        assert_eq!(added, 0);
    }

    #[test]
    fn mixed_present_and_new() {
        let (merged, added) =
            merge_into_path("C:\\Windows;C:\\foo", &["c:\\FOO".into(), "D:\\bar".into()]);
        assert_eq!(merged, "C:\\Windows;C:\\foo;D:\\bar");
        assert_eq!(added, 1);
    }

    #[test]
    fn blank_entries_in_current_dont_shadow_candidates() {
        // A stray ";" must not create a phantom "" entry that blocks a real dir.
        let (merged, added) = merge_into_path("C:\\Windows;", &["D:\\bar".into()]);
        assert_eq!(merged, "C:\\Windows;;D:\\bar");
        assert_eq!(added, 1);
    }

    #[test]
    fn real_path_dirs_reads_registry() {
        // End-to-end: the system PATH always exists on Windows and always
        // contains C:\Windows. If this fails, the registry subkey/value names
        // in `read_path_value` are wrong (silent None → no hydration).
        let dirs = real_path_dirs();
        assert!(!dirs.is_empty(), "registry PATH read returned nothing");
        assert!(
            dirs.iter()
                .any(|d| d.to_ascii_lowercase().contains("\\windows")),
            "system PATH missing C:\\Windows — subkey path likely wrong"
        );
        // REG_EXPAND_SZ entries (e.g. %USERPROFILE%\...) must be expanded —
        // a literal '%' leaking through means utils::platform::expand_path didn't
        // run or the value wasn't read as EXPAND_SZ.
        assert!(
            dirs.iter().all(|d| !d.contains('%')),
            "unexpanded env var in PATH — EXPAND_SZ expansion failed"
        );
    }
}
