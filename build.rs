//! Discovers one-file SDK strategies so authors never edit a module list or registry.
//!
//! Strategy files come from `src/strategies/user/` in this checkout plus any extra
//! folders named in `local.toml` (`[strategies] dirs = [...]`) or the
//! `BACKTESTER_STRATEGY_DIRS` environment variable, which is how a private strategies
//! repository is compiled into the engine without living in this repository. Every
//! file must define `pub fn entry() -> crate::sdk::StrategyEntry`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=local.toml");
    println!("cargo:rerun-if-env-changed=BACKTESTER_STRATEGY_DIRS");

    let mut dirs = vec![manifest_dir.join("src/strategies/user")];
    dirs.extend(local_strategy_dirs(&manifest_dir));
    if let Ok(extra) = env::var("BACKTESTER_STRATEGY_DIRS") {
        dirs.extend(
            env::split_paths(&extra)
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| absolute_from(&manifest_dir, &p)),
        );
    }

    // Module name -> file. Later directories override earlier ones so a private copy of
    // an example can replace it deliberately.
    let mut files: BTreeMap<String, PathBuf> = BTreeMap::new();
    for dir in &dirs {
        println!("cargo:rerun-if-changed={}", dir.display());
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("rs") || stem == "mod" {
                continue;
            }
            if !is_valid_module_name(stem) {
                println!(
                    "cargo:warning=skipping strategy file {} (module names must be snake_case)",
                    path.display()
                );
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            files.insert(stem.to_owned(), absolute(&path));
        }
    }

    let mut out = String::new();
    out.push_str("/// Strategies discovered by build.rs.\npub mod user {\n");
    for (name, path) in &files {
        out.push_str(&format!(
            "    #[path = {:?}]\n    pub mod {name};\n",
            path.display().to_string()
        ));
    }
    out.push_str("}\n\n");
    out.push_str("fn user_entries() -> Vec<crate::sdk::strategy::StrategyEntry> {\n    vec![\n");
    for name in files.keys() {
        out.push_str(&format!("        user::{name}::entry(),\n"));
    }
    out.push_str("    ]\n}\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("out dir"));
    fs::write(out_dir.join("user_strategies.rs"), out).expect("write registry");
}

/// Reads `[strategies] dirs` from local.toml without pulling a TOML crate into the build
/// script: a minimal scan for the `dirs = [ ... ]` line inside the `[strategies]` table.
fn local_strategy_dirs(manifest_dir: &Path) -> Vec<PathBuf> {
    let Ok(text) = fs::read_to_string(manifest_dir.join("local.toml")) else {
        return Vec::new();
    };
    let mut in_table = false;
    let mut dirs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[strategies]";
            continue;
        }
        if !in_table || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("dirs") {
            let rest = rest.trim_start().trim_start_matches('=').trim();
            let inner = rest.trim_start_matches('[').trim_end_matches(']');
            for item in inner.split(',') {
                let item = item.trim().trim_matches('"').trim_matches('\'');
                if !item.is_empty() {
                    dirs.push(absolute_from(manifest_dir, Path::new(item)));
                }
            }
        }
    }
    dirs
}

fn is_valid_module_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        absolute(path)
    } else {
        absolute(&base.join(path))
    }
}

fn absolute(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
