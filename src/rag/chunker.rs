#![allow(dead_code)]
//! Structural chunker: language detection + per-type chunking strategies.
//!
//! Supported extensions per spec:
//! `.rs .py .java .kt .js .ts .tsx .jsx .go .c .cpp .h`  → code (function-level)
//! `.md .mdx`                                             → markdown (by heading)
//! `.yaml .yml .toml`                                     → config (top-level blocks)
//! `.txt .pdf`                                            → paragraph / default
//! Everything else                                        → ignored (returns empty)

const MAX_CHUNK_TOKENS: usize = 512;
const OVERLAP_TOKENS: usize = 64;
// Rough approximation: 1 token ≈ 4 chars
const CHARS_PER_TOKEN: usize = 4;

/// Detect language tag from file extension.
pub fn detect_lang(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "java" => Some("java"),
        "kt" => Some("kotlin"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        "go" => Some("go"),
        "c" | "cpp" | "h" => Some("c"),
        "md" | "mdx" => Some("markdown"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "txt" | "pdf" => Some("text"),
        _ => None,
    }
}

/// Chunk `content` according to the language strategy.
/// Returns `(chunk_index, text)` pairs.
pub fn chunk(content: &str, lang: &str) -> Vec<(usize, String)> {
    match lang {
        "rust" | "python" | "java" | "kotlin" | "javascript" | "typescript" | "go" | "c" => {
            chunk_code(content)
        }
        "markdown" => chunk_markdown(content),
        "yaml" | "toml" => chunk_config(content),
        _ => chunk_paragraphs(content),
    }
}

/// Code: split on top-level function/method/class boundaries.
/// Heuristic: blank line before a line that starts with a non-whitespace char
/// that looks like a definition keyword or `fn`/`def`/`func`/`class`/`impl`.
fn chunk_code(content: &str) -> Vec<(usize, String)> {
    let definition_starters = [
        "fn ",
        "pub fn",
        "async fn",
        "pub async fn",
        "impl ",
        "pub impl",
        "struct ",
        "pub struct",
        "enum ",
        "pub enum",
        "trait ",
        "pub trait",
        "mod ",
        "pub mod",
        "def ",
        "class ",
        "func ",
        "function ",
        "const ",
        "let ",
        "var ",
        "interface ",
        "type ",
        "export ",
    ];

    let lines: Vec<&str> = content.lines().collect();
    let mut chunks: Vec<(usize, String)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut idx = 0usize;

    for (i, &line) in lines.iter().enumerate() {
        let is_boundary = line.trim().is_empty()
            && i + 1 < lines.len()
            && definition_starters
                .iter()
                .any(|kw| lines[i + 1].trim_start().starts_with(kw));

        if is_boundary && !current.is_empty() {
            let text = current.join("\n").trim().to_owned();
            if !text.is_empty() {
                chunks.push((idx, text));
                idx += 1;
            }
            current.clear();
        }
        current.push(line);
    }
    if !current.is_empty() {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            chunks.push((idx, text));
        }
    }

    // If no boundaries found, fall back to paragraph chunking
    if chunks.len() <= 1 {
        return chunk_paragraphs(content);
    }
    chunks
}

/// Markdown: split on headings (`#`, `##`, `###`).
fn chunk_markdown(content: &str) -> Vec<(usize, String)> {
    let mut chunks: Vec<(usize, String)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut idx = 0usize;

    for line in content.lines() {
        if line.starts_with('#') && !current.is_empty() {
            let text = current.join("\n").trim().to_owned();
            if !text.is_empty() {
                chunks.push((idx, text));
                idx += 1;
            }
            current.clear();
        }
        current.push(line);
    }
    if !current.is_empty() {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            chunks.push((idx, text));
        }
    }
    if chunks.is_empty() {
        chunk_paragraphs(content)
    } else {
        chunks
    }
}

/// YAML/TOML: split on section headers ([section]) or YAML root keys (key:).
fn chunk_config(content: &str) -> Vec<(usize, String)> {
    let mut chunks: Vec<(usize, String)> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut idx = 0usize;

    for line in content.lines() {
        // TOML section header: [section] or [[array]]
        let is_toml_section = line.starts_with('[') && line.contains(']');
        // YAML root key: no indent, not a comment, ends with ':'
        let is_yaml_root = !line.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.starts_with('#')
            && !line.starts_with('[')
            && line.trim_end().ends_with(':');

        if (is_toml_section || is_yaml_root) && !current.is_empty() {
            let text = current.join("\n").trim().to_owned();
            if !text.is_empty() {
                chunks.push((idx, text));
                idx += 1;
            }
            current.clear();
        }
        current.push(line);
    }
    if !current.is_empty() {
        let text = current.join("\n").trim().to_owned();
        if !text.is_empty() {
            chunks.push((idx, text));
        }
    }
    if chunks.is_empty() {
        chunk_paragraphs(content)
    } else {
        chunks
    }
}

/// Default: paragraph chunking with 512-token window and 64-token overlap.
fn chunk_paragraphs(content: &str) -> Vec<(usize, String)> {
    let window = MAX_CHUNK_TOKENS * CHARS_PER_TOKEN;
    let overlap = OVERLAP_TOKENS * CHARS_PER_TOKEN;

    // Split into paragraphs first
    let paragraphs: Vec<&str> = content
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let mut chunks: Vec<(usize, String)> = Vec::new();
    let mut buf = String::new();
    let mut idx = 0usize;

    for para in &paragraphs {
        if buf.len() + para.len() > window && !buf.is_empty() {
            chunks.push((idx, buf.trim().to_owned()));
            idx += 1;
            // Keep overlap from end of previous buffer
            let overlap_start = buf.len().saturating_sub(overlap);
            buf = buf[overlap_start..].to_owned();
        }
        if !buf.is_empty() {
            buf.push_str("\n\n");
        }
        buf.push_str(para);
    }
    if !buf.trim().is_empty() {
        chunks.push((idx, buf.trim().to_owned()));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lang_rust() {
        assert_eq!(detect_lang("src/main.rs"), Some("rust"));
    }

    #[test]
    fn detect_lang_unknown_returns_none() {
        assert_eq!(detect_lang("file.xyz"), None);
    }

    #[test]
    fn detect_lang_markdown() {
        assert_eq!(detect_lang("README.md"), Some("markdown"));
    }

    #[test]
    fn chunk_markdown_splits_on_headings() {
        let md =
            "# Title\n\nIntro text.\n\n## Section A\n\nContent A.\n\n## Section B\n\nContent B.";
        let chunks = chunk(md, "markdown");
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].1.contains("Title"));
        assert!(chunks[1].1.contains("Section A"));
        assert!(chunks[2].1.contains("Section B"));
    }

    #[test]
    fn chunk_markdown_indices_are_sequential() {
        let md = "# A\n\ntext\n\n## B\n\nmore";
        let chunks = chunk(md, "markdown");
        let indices: Vec<usize> = chunks.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, (0..chunks.len()).collect::<Vec<_>>());
    }

    #[test]
    fn chunk_paragraphs_respects_window() {
        // Build content with paragraph breaks that exceeds the window
        let para = "word ".repeat(150); // ~750 chars per paragraph
        let big = [para.as_str(); 6].join("\n\n"); // ~4500 chars total
        let chunks = chunk_paragraphs(&big);
        assert!(chunks.len() >= 2, "should split into multiple chunks");
    }

    #[test]
    fn chunk_config_splits_toml_top_level() {
        let toml = "[package]\nname = \"foo\"\n\n[dependencies]\nanyhow = \"1.0\"";
        let chunks = chunk(toml, "toml");
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].1.contains("[package]"));
        assert!(chunks[1].1.contains("[dependencies]"));
    }

    #[test]
    fn chunk_code_falls_back_to_paragraphs_when_no_boundaries() {
        let code = "let x = 1;\nlet y = 2;";
        let chunks = chunk(code, "rust");
        assert!(!chunks.is_empty());
    }
}
