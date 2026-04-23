//! Global skill standard — `~/.agents/skills/` management.
//!
//! Skills live in a single master directory (`~/.agents/skills/`).
//! Canopy creates symlinks from each platform's own skills folder to the
//! master directory so every agent always sees the same set of skills.
//!
//! Layout:
//! ```text
//! ~/.agents/skills/
//!   code-review/
//!     SKILL.md          ← instructions injected by the @ picker
//!   rust-idiomatic-patterns/
//!     SKILL.md
//! ~/.kiro/skills/code-review  →  ~/.agents/skills/code-review  (symlink)
//! ```

use anyhow::{Context, Result};
use inquire::Select;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Well-known skills master directory path.
pub fn global_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".agents").join("skills"))
}

/// Ensure the global skills directory exists.
pub fn ensure_global_skills_dir() -> Result<PathBuf> {
    let dir = global_skills_dir().context("No home directory")?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Create (or repair) symlinks from each platform's skills directory to the
/// global skills master.
///
/// A symlink is created for every immediate child directory inside
/// `~/.agents/skills/`. Each platform that exposes a `skills_dir` in the
/// registry gets a per-skill symlink `<platform_skills_dir>/<skill_name>`.
pub fn create_platform_symlinks(
    home: &Path,
    platforms: &[&crate::setup_module::Platform],
) -> Result<Vec<String>> {
    let global = home.join(".agents").join("skills");
    if !global.exists() {
        return Ok(Vec::new());
    }

    let skill_entries = list_skill_dirs(&global);
    let mut created = Vec::new();

    for platform in platforms {
        let Some(ref skills_rel) = platform.skills_dir else {
            continue;
        };
        let platform_skills = home.join(skills_rel);
        if let Err(e) = std::fs::create_dir_all(&platform_skills) {
            tracing::warn!("Could not create skills dir for {}: {}", platform.name, e);
            continue;
        }

        for skill_name in &skill_entries {
            let link = platform_skills.join(skill_name);
            let target = global.join(skill_name);

            // Skip if symlink/dir already exists and points to the right place
            if link.exists() || link.is_symlink() {
                continue;
            }

            #[cfg(unix)]
            {
                if let Err(e) = std::os::unix::fs::symlink(&target, &link) {
                    tracing::warn!("Symlink {}: {}", link.display(), e);
                } else {
                    created.push(format!("{}/{}", platform.name, skill_name));
                }
            }
            #[cfg(not(unix))]
            {
                // On Windows, copy the directory instead of symlinking
                if copy_dir_recursive(&target, &link).is_ok() {
                    created.push(format!("{}/{}", platform.name, skill_name));
                }
            }
        }
    }

    Ok(created)
}

/// Validate symlink integrity: return broken symlink paths.
/// Used by the `canopy skills` wizard (future subcommand).
#[allow(dead_code)]
pub fn find_broken_symlinks(home: &Path, platforms: &[&crate::setup_module::Platform]) -> Vec<PathBuf> {
    let mut broken = Vec::new();

    for platform in platforms {
        let Some(ref skills_rel) = platform.skills_dir else {
            continue;
        };
        let platform_skills = home.join(skills_rel);
        if !platform_skills.exists() {
            continue;
        }

        if let Ok(rd) = std::fs::read_dir(&platform_skills) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_symlink() && !path.exists() {
                    broken.push(path);
                }
            }
        }
    }

    broken
}

/// List immediate subdirectory names inside `dir` that look like skill folders
/// (contain a `SKILL.md` or `INSTRUCTIONS.md`).
pub fn list_skill_dirs(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| {
            let p = e.path();
            p.join("SKILL.md").exists() || p.join("INSTRUCTIONS.md").exists()
        })
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    names.sort();
    names
}

/// Locate the instructions file for a skill directory.
/// Returns `Some(path)` for `SKILL.md`, then `INSTRUCTIONS.md`, otherwise `None`.
pub fn find_skill_instructions(skill_dir: &Path) -> Option<PathBuf> {
    let skill_md = skill_dir.join("SKILL.md");
    if skill_md.exists() {
        return Some(skill_md);
    }
    let instructions_md = skill_dir.join("INSTRUCTIONS.md");
    if instructions_md.exists() {
        return Some(instructions_md);
    }
    None
}

// ── Essential Pack download ────────────────────────────────────────────────

const ESSENTIAL_PACK_REPO: &str = "UniverLab/skills";
const ESSENTIAL_PACK_API: &str = "https://api.github.com/repos/UniverLab/skills/contents";

/// Fetch the "UniverLab Essential Pack" of skills from GitHub into the global
/// skills directory.
///
/// Each top-level directory in the repo that contains a `SKILL.md` or
/// `INSTRUCTIONS.md` is downloaded to `~/.agents/skills/<skill_name>/`.
pub fn download_essential_pack() -> Result<usize> {
    let global = ensure_global_skills_dir()?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("canopy")
        .build()?;

    // List root-level contents of the repo
    let resp = client
        .get(ESSENTIAL_PACK_API)
        .send()
        .context("Failed to connect to GitHub API")?;

    if !resp.status().is_success() {
        // Graceful degradation — not a fatal error
        tracing::warn!(
            "GitHub API returned {} for {}; skipping essential skills download.",
            resp.status(),
            ESSENTIAL_PACK_REPO
        );
        return Ok(0);
    }

    let entries: Vec<GhEntry> = resp.json().context("Failed to parse GitHub API response")?;

    let mut downloaded = 0usize;
    for entry in &entries {
        if entry.entry_type != "dir" {
            continue;
        }

        let skill_dir = global.join(&entry.name);
        if skill_dir.exists() {
            // Already installed — do not overwrite user customizations
            continue;
        }

        // List the directory contents
        let dir_url = format!("{}/{}", ESSENTIAL_PACK_API, entry.name);
        let Ok(dir_resp) = client.get(&dir_url).send() else {
            continue;
        };
        if !dir_resp.status().is_success() {
            continue;
        }
        let Ok(dir_entries) = dir_resp.json::<Vec<GhEntry>>() else {
            continue;
        };

        // Only download if there's a SKILL.md or INSTRUCTIONS.md
        let has_instructions = dir_entries
            .iter()
            .any(|e| e.name == "SKILL.md" || e.name == "INSTRUCTIONS.md");
        if !has_instructions {
            continue;
        }

        std::fs::create_dir_all(&skill_dir)?;

        for file in &dir_entries {
            if file.entry_type != "file" {
                continue;
            }
            let Some(ref raw_url) = file.download_url else {
                continue;
            };
            let Ok(file_resp) = client.get(raw_url).send() else {
                continue;
            };
            if !file_resp.status().is_success() {
                continue;
            }
            let Ok(content) = file_resp.bytes() else {
                continue;
            };
            let file_path = skill_dir.join(&file.name);
            let _ = std::fs::write(&file_path, &content);
        }

        downloaded += 1;
    }

    Ok(downloaded)
}

#[derive(serde::Deserialize)]
struct GhEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    download_url: Option<String>,
}

// ── Interactive Skills Wizard ──────────────────────────────────────────────

/// Run the interactive skills management wizard.
/// Intended for the future `canopy skills` subcommand.
#[allow(dead_code)]
pub fn run_skills_wizard(home: &Path, platforms: &[&crate::setup_module::Platform]) -> Result<()> {
    println!();
    println!("  \x1b[1mSkills Manager\x1b[0m");
    println!("  ─────────────────────────────────────────────");

    let global = home.join(".agents").join("skills");

    let action = Select::new(
        "What would you like to do?",
        vec![
            "List — show installed skills",
            "Validate — check symlink integrity",
            "Remove — uninstall a skill",
        ],
    )
    .with_help_message("↑↓ navigate | Enter select | Esc cancel")
    .prompt()
    .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;

    match action {
        a if a.starts_with("List") => list_skills(&global),
        a if a.starts_with("Validate") => validate_skills(home, platforms),
        a if a.starts_with("Remove") => remove_skill(home, &global, platforms),
        _ => Ok(()),
    }
}

#[allow(dead_code)]
fn list_skills(global: &Path) -> Result<()> {
    let skills = list_skill_dirs(global);
    if skills.is_empty() {
        println!(
            "  \x1b[33m⚠\x1b[0m  No skills installed in {}",
            global.display()
        );
        println!("  Run \x1b[1mcanopy setup\x1b[0m to download the Essential Pack.");
    } else {
        println!("  Installed skills ({}):", skills.len());
        for s in &skills {
            println!("    \x1b[32m•\x1b[0m {s}");
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_skills(home: &Path, platforms: &[&crate::setup_module::Platform]) -> Result<()> {
    let broken = find_broken_symlinks(home, platforms);
    if broken.is_empty() {
        println!("  \x1b[32m✓\x1b[0m All skill symlinks are healthy.");
    } else {
        println!("  \x1b[31m✗\x1b[0m Broken symlinks ({}):", broken.len());
        for p in &broken {
            println!("    \x1b[31m✗\x1b[0m {}", p.display());
        }
        print!("  Remove broken symlinks? [Y/n] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "n" {
            for p in &broken {
                let _ = std::fs::remove_file(p);
            }
            println!(
                "  \x1b[32m✓\x1b[0m Removed {} broken symlink(s).",
                broken.len()
            );
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn remove_skill(home: &Path, global: &Path, platforms: &[&crate::setup_module::Platform]) -> Result<()> {
    let skills = list_skill_dirs(global);
    if skills.is_empty() {
        println!("  \x1b[33m⚠\x1b[0m  No skills to remove.");
        return Ok(());
    }

    let selected = Select::new("Select skill to remove:", skills)
        .with_help_message("This removes the master copy and all platform symlinks")
        .prompt()
        .map_err(|e| anyhow::anyhow!("Cancelled: {}", e))?;

    print!("  Remove \x1b[1m{selected}\x1b[0m and all its platform symlinks? [Y/n] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    if input.trim().to_lowercase() == "n" {
        println!("  Cancelled.");
        return Ok(());
    }

    // Remove master
    let master = global.join(&selected);
    let _ = std::fs::remove_dir_all(&master);

    // Remove platform symlinks
    for platform in platforms {
        let Some(ref skills_rel) = platform.skills_dir else {
            continue;
        };
        let link = home.join(skills_rel).join(&selected);
        if link.exists() || link.is_symlink() {
            let _ = if link.is_symlink() || link.is_file() {
                std::fs::remove_file(&link)
            } else {
                std::fs::remove_dir_all(&link)
            };
        }
    }

    println!("  \x1b[32m✓\x1b[0m '{selected}' removed.");
    Ok(())
}

// ── Utilities ──────────────────────────────────────────────────────────────

/// Recursively copy a directory (used on non-Unix systems as symlink fallback).
#[cfg(not(unix))]
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let dest = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(&entry.path(), &dest)?;
        }
    }
    Ok(())
}
