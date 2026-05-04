//! `.canopy/ragignore` — per-project exclusion patterns for RAG indexing.
//!
//! On project registration the file is created (if absent) and seeded with
//! default exclusions plus any patterns found in the project's `.gitignore`.

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

/// Path to the ragignore file for a given project root.
pub fn ragignore_path(project_root: &Path) -> PathBuf {
    project_root.join(".canopy").join("ragignore")
}

/// Create `.canopy/ragignore` in `project_root` if it doesn't exist.
/// Seeds it with default patterns + patterns from `.gitignore` (if present).
pub fn ensure_ragignore(project_root: &Path) {
    let path = ragignore_path(project_root);
    if path.exists() {
        return;
    }
    let dir = path.parent().unwrap();
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }

    let mut patterns: Vec<String> = DEFAULT_PATTERNS.iter().map(|s| s.to_string()).collect();

    // Seed from .gitignore
    let gitignore = project_root.join(".gitignore");
    if let Ok(content) = std::fs::read_to_string(&gitignore) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            // Avoid duplicates
            if !patterns.iter().any(|p| p == trimmed) {
                patterns.push(trimmed.to_string());
            }
        }
    }

    let content = format!(
        "# ragignore — patterns excluded from RAG indexing\n# Glob patterns, one per line. Lines starting with # are comments.\n\n{}\n",
        patterns.join("\n")
    );
    let _ = std::fs::write(&path, content);
}

/// Load ignore patterns from `.canopy/ragignore` in `project_root`.
/// Returns an empty vec if the file doesn't exist.
pub fn load_patterns(project_root: &Path) -> Vec<String> {
    let path = ragignore_path(project_root);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return vec![];
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
    // Work with the path relative to the project root for matching.
    let rel = file_path
        .strip_prefix(project_root)
        .unwrap_or(file_path)
        .to_string_lossy();

    for pattern in patterns {
        let pat = pattern.trim_end_matches('/');
        // Directory prefix match: "target/" matches "target/foo/bar"
        if pattern.ends_with('/') {
            if rel.starts_with(pat)
                && (rel.len() == pat.len() || rel.as_bytes().get(pat.len()) == Some(&b'/'))
            {
                return true;
            }
            continue;
        }
        // Simple glob: only support leading `*` wildcard (e.g. `*.lock`)
        if let Some(suffix) = pattern.strip_prefix('*') {
            if rel.ends_with(suffix.trim_start_matches('/')) {
                return true;
            }
            continue;
        }
        // Exact segment or prefix match
        if rel == rel.as_ref()
            && (rel.as_ref() == pattern.as_str() || rel.starts_with(&format!("{pattern}/")))
        {
            return true;
        }
    }
    false
}
