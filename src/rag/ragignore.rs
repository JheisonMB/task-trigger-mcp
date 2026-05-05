//! `.canopy/ragignore` — global exclusion patterns for RAG indexing.
//!
//! Lives in the canopy data dir (`~/.canopy/ragignore`), not per-project.
//! On first use it is created with default exclusions. When a project is
//! registered its `.gitignore` patterns are merged in automatically.

use std::path::{Path, PathBuf};

const DEFAULT_PATTERNS: &[&str] = &[
    ".agents/",
    ".git/",
    "target/",
    "node_modules/",
    "dist/",
    "build/",
    ".cache/",
    "__pycache__/",
    ".venv/",
    "vendor/",
    ".idea/",
    ".vscode/",
    "*.lock",
    "*.min.js",
    "*.min.css",
];

/// Path to the global ragignore file.
pub fn ragignore_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ragignore")
}

/// Ensure the global ragignore exists in `data_dir`.
/// Seeds it with default patterns if it doesn't exist yet.
pub fn ensure_ragignore(data_dir: &Path) {
    let path = ragignore_path(data_dir);
    if path.exists() {
        return;
    }
    if std::fs::create_dir_all(data_dir).is_err() {
        return;
    }
    let content = format!(
        "# ragignore — global patterns excluded from RAG indexing\n# Glob patterns, one per line. Lines starting with # are comments.\n\n{}\n",
        DEFAULT_PATTERNS.join("\n")
    );
    let _ = std::fs::write(&path, content);
}

/// Merge patterns from a project's `.gitignore` into the global ragignore.
/// Only adds patterns not already present.
pub fn merge_gitignore(data_dir: &Path, project_root: &Path) {
    let gitignore = project_root.join(".gitignore");
    let Ok(gitignore_content) = std::fs::read_to_string(&gitignore) else {
        return;
    };

    let ragignore = ragignore_path(data_dir);
    let existing = std::fs::read_to_string(&ragignore).unwrap_or_default();
    let existing_patterns: std::collections::HashSet<&str> = existing
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let new_patterns: Vec<&str> = gitignore_content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !existing_patterns.contains(l))
        .collect();

    if new_patterns.is_empty() {
        return;
    }

    let addition = format!(
        "\n# from {}\n{}\n",
        project_root.display(),
        new_patterns.join("\n")
    );
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .open(&ragignore)
        .and_then(|mut f| std::io::Write::write_all(&mut f, addition.as_bytes()));
}

/// Load ignore patterns from the global ragignore in `data_dir`.
pub fn load_patterns(data_dir: &Path) -> Vec<String> {
    let path = ragignore_path(data_dir);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();
    };
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Returns true if `file_path` (absolute) should be excluded given `project_root`
/// and the loaded `patterns`.
pub fn is_ignored(file_path: &Path, project_root: &Path, patterns: &[String]) -> bool {
    let rel = file_path
        .strip_prefix(project_root)
        .unwrap_or(file_path)
        .to_string_lossy();

    for pattern in patterns {
        let pat = pattern.trim_end_matches('/');
        if pattern.ends_with('/') {
            if rel.starts_with(pat)
                && (rel.len() == pat.len() || rel.as_bytes().get(pat.len()) == Some(&b'/'))
            {
                return true;
            }
            continue;
        }
        if let Some(suffix) = pattern.strip_prefix('*') {
            if rel.ends_with(suffix.trim_start_matches('/')) {
                return true;
            }
            continue;
        }
        if rel.as_ref() == pattern.as_str() || rel.starts_with(&format!("{pattern}/")) {
            return true;
        }
    }
    false
}
