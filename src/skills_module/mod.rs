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

const LIST_ACTION: &str = "List — show installed skills";
const VALIDATE_ACTION: &str = "Validate — check symlink integrity";
const REMOVE_ACTION: &str = "Remove — uninstall a skill";
const SKILL_ACTIONS: [&str; 3] = [LIST_ACTION, VALIDATE_ACTION, REMOVE_ACTION];

/// Well-known skills master directory path.
pub fn global_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| global_skills_dir_for(&h))
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
    let global = global_skills_dir_for(home);
    if !global.exists() {
        return Ok(Vec::new());
    }

    let skill_entries = list_skill_dirs(&global);
    let mut created = Vec::new();

    for (platform, platform_skills) in platform_skill_dirs(home, platforms) {
        create_platform_symlinks_for_platform(
            platform,
            &platform_skills,
            &global,
            &skill_entries,
            &mut created,
        );
    }

    Ok(created)
}

fn create_platform_symlinks_for_platform(
    platform: &crate::setup_module::Platform,
    platform_skills: &Path,
    global: &Path,
    skill_entries: &[String],
    created: &mut Vec<String>,
) {
    if !ensure_platform_skills_dir(platform, platform_skills) {
        return;
    }

    for skill_name in skill_entries {
        if let Some(created_link) =
            create_platform_skill_link(&platform.name, platform_skills, global, skill_name)
        {
            created.push(created_link);
        }
    }
}

fn create_platform_skill_link(
    platform_name: &str,
    platform_skills: &Path,
    global: &Path,
    skill_name: &str,
) -> Option<String> {
    let link = platform_skills.join(skill_name);
    if link.exists() || link.is_symlink() {
        return None;
    }

    let target = global.join(skill_name);
    if let Err(error) = install_skill_link(&target, &link) {
        tracing::warn!("Symlink {}: {}", link.display(), error);
        return None;
    }

    Some(format!("{platform_name}/{skill_name}"))
}

/// Validate symlink integrity: return broken symlink paths.
/// Used by the `canopy skills` wizard (future subcommand).
#[allow(dead_code)]
pub fn find_broken_symlinks(
    home: &Path,
    platforms: &[&crate::setup_module::Platform],
) -> Vec<PathBuf> {
    platform_skill_dirs(home, platforms)
        .flat_map(|(_, platform_skills)| broken_symlinks_in_dir(&platform_skills))
        .collect()
}

fn broken_symlinks_in_dir(dir: &Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_symlink() && !path.exists())
        .collect()
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
        .filter(|e| contains_skill_instructions(&e.path()))
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    names.sort();
    names
}

/// Locate the instructions file for a skill directory.
/// Returns `Some(path)` for `SKILL.md`, then `INSTRUCTIONS.md`, otherwise `None`.
pub fn find_skill_instructions(skill_dir: &Path) -> Option<PathBuf> {
    ["SKILL.md", "INSTRUCTIONS.md"]
        .into_iter()
        .map(|name| skill_dir.join(name))
        .find(|path| path.exists())
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
    let client = build_github_client()?;
    let Some(entries) = fetch_essential_pack_entries(&client)? else {
        return Ok(0);
    };

    download_missing_skill_dirs(&client, &global, &entries)
}

fn build_github_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("canopy")
        .build()
        .map_err(Into::into)
}

fn fetch_essential_pack_entries(
    client: &reqwest::blocking::Client,
) -> Result<Option<Vec<GhEntry>>> {
    let response = client
        .get(ESSENTIAL_PACK_API)
        .send()
        .context("Failed to connect to GitHub API")?;

    if !response.status().is_success() {
        tracing::warn!(
            "GitHub API returned {} for {}; skipping essential skills download.",
            response.status(),
            ESSENTIAL_PACK_REPO
        );
        return Ok(None);
    }

    let entries = response
        .json()
        .context("Failed to parse GitHub API response")?;
    Ok(Some(entries))
}

fn download_missing_skill_dirs(
    client: &reqwest::blocking::Client,
    global: &Path,
    entries: &[GhEntry],
) -> Result<usize> {
    let mut downloaded = 0usize;

    for entry in entries.iter().filter(|entry| entry.entry_type == "dir") {
        let skill_dir = global.join(&entry.name);
        if skill_dir.exists() {
            continue;
        }

        if download_skill_dir(client, &entry.name, &skill_dir)? {
            downloaded += 1;
        }
    }

    Ok(downloaded)
}

/// Download a single skill directory from GitHub. Returns `true` if downloaded.
fn download_skill_dir(
    client: &reqwest::blocking::Client,
    skill_name: &str,
    skill_dir: &Path,
) -> Result<bool> {
    let Some(dir_entries) = fetch_skill_dir_entries(client, skill_name)? else {
        return Ok(false);
    };
    if !has_skill_instructions_entry(&dir_entries) {
        return Ok(false);
    }

    std::fs::create_dir_all(skill_dir)?;
    write_skill_files(client, skill_dir, &dir_entries);
    Ok(true)
}

fn fetch_skill_dir_entries(
    client: &reqwest::blocking::Client,
    skill_name: &str,
) -> Result<Option<Vec<GhEntry>>> {
    let dir_url = format!("{ESSENTIAL_PACK_API}/{skill_name}");
    let Ok(response) = client.get(&dir_url).send() else {
        return Ok(None);
    };
    if !response.status().is_success() {
        return Ok(None);
    }

    let Ok(entries) = response.json() else {
        return Ok(None);
    };
    Ok(Some(entries))
}

fn has_skill_instructions_entry(entries: &[GhEntry]) -> bool {
    entries
        .iter()
        .any(|entry| matches!(entry.name.as_str(), "SKILL.md" | "INSTRUCTIONS.md"))
}

fn write_skill_files(client: &reqwest::blocking::Client, skill_dir: &Path, entries: &[GhEntry]) {
    for file in entries.iter().filter(|entry| entry.entry_type == "file") {
        write_skill_file(client, skill_dir, file);
    }
}

fn write_skill_file(client: &reqwest::blocking::Client, skill_dir: &Path, file: &GhEntry) {
    let Some(raw_url) = file.download_url.as_deref() else {
        return;
    };

    let Ok(response) = client.get(raw_url).send() else {
        return;
    };
    if !response.status().is_success() {
        return;
    }

    let Ok(content) = response.bytes() else {
        return;
    };
    let _ = std::fs::write(skill_dir.join(&file.name), &content);
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
    print_skills_wizard_header();

    let global = global_skills_dir_for(home);
    let action = prompt_skills_action()?;
    handle_skills_action(action, home, &global, platforms)
}

fn print_skills_wizard_header() {
    println!();
    println!("  \x1b[1mSkills Manager\x1b[0m");
    println!("  ─────────────────────────────────────────────");
}

fn prompt_skills_action() -> Result<&'static str> {
    Select::new("What would you like to do?", SKILL_ACTIONS.to_vec())
        .with_help_message("↑↓ navigate | Enter select | Esc cancel")
        .prompt()
        .map_err(|error| anyhow::anyhow!("Cancelled: {}", error))
}

fn handle_skills_action(
    action: &str,
    home: &Path,
    global: &Path,
    platforms: &[&crate::setup_module::Platform],
) -> Result<()> {
    match action {
        LIST_ACTION => list_skills(global),
        VALIDATE_ACTION => validate_skills(home, platforms),
        REMOVE_ACTION => remove_skill(home, global, platforms),
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
        for skill in &skills {
            println!("    \x1b[32m•\x1b[0m {skill}");
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn validate_skills(home: &Path, platforms: &[&crate::setup_module::Platform]) -> Result<()> {
    let broken = find_broken_symlinks(home, platforms);
    if broken.is_empty() {
        println!("  \x1b[32m✓\x1b[0m All skill symlinks are healthy.");
        return Ok(());
    }

    print_broken_symlinks(&broken);
    if !prompt_yes_no("  Remove broken symlinks? [Y/n] ")? {
        return Ok(());
    }

    remove_paths(&broken);
    println!(
        "  \x1b[32m✓\x1b[0m Removed {} broken symlink(s).",
        broken.len()
    );
    Ok(())
}

fn print_broken_symlinks(broken: &[PathBuf]) {
    println!("  \x1b[31m✗\x1b[0m Broken symlinks ({}):", broken.len());
    for path in broken {
        println!("    \x1b[31m✗\x1b[0m {}", path.display());
    }
}

#[allow(dead_code)]
fn remove_skill(
    home: &Path,
    global: &Path,
    platforms: &[&crate::setup_module::Platform],
) -> Result<()> {
    let Some(selected) = prompt_skill_selection(global)? else {
        return Ok(());
    };
    if !prompt_yes_no(&format!(
        "  Remove \x1b[1m{selected}\x1b[0m and all its platform symlinks? [Y/n] "
    ))? {
        println!("  Cancelled.");
        return Ok(());
    }

    remove_skill_installation(home, global, platforms, &selected);
    println!("  \x1b[32m✓\x1b[0m '{selected}' removed.");
    Ok(())
}

fn prompt_skill_selection(global: &Path) -> Result<Option<String>> {
    let skills = list_skill_dirs(global);
    if skills.is_empty() {
        println!("  \x1b[33m⚠\x1b[0m  No skills to remove.");
        return Ok(None);
    }

    Select::new("Select skill to remove:", skills)
        .with_help_message("This removes the master copy and all platform symlinks")
        .prompt()
        .map(Some)
        .map_err(|error| anyhow::anyhow!("Cancelled: {}", error))
}

fn remove_skill_installation(
    home: &Path,
    global: &Path,
    platforms: &[&crate::setup_module::Platform],
    selected: &str,
) {
    let _ = std::fs::remove_dir_all(global.join(selected));

    for (_, platform_skills) in platform_skill_dirs(home, platforms) {
        remove_skill_path(&platform_skills.join(selected));
    }
}

fn remove_skill_path(path: &Path) {
    if !(path.exists() || path.is_symlink()) {
        return;
    }

    let _ = if path.is_symlink() || path.is_file() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    };
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(!matches!(input.trim(), "n" | "N"))
}

// ── Utilities ──────────────────────────────────────────────────────────────

fn global_skills_dir_for(home: &Path) -> PathBuf {
    home.join(".agents").join("skills")
}

fn contains_skill_instructions(dir: &Path) -> bool {
    find_skill_instructions(dir).is_some()
}

fn platform_skill_dirs<'a>(
    home: &'a Path,
    platforms: &'a [&'a crate::setup_module::Platform],
) -> impl Iterator<Item = (&'a crate::setup_module::Platform, PathBuf)> + 'a {
    platforms.iter().copied().filter_map(move |platform| {
        platform_skill_dir(home, platform).map(|skills_dir| (platform, skills_dir))
    })
}

fn platform_skill_dir(home: &Path, platform: &crate::setup_module::Platform) -> Option<PathBuf> {
    let skills_dir = platform.skills_dir.as_deref()?;
    Some(home.join(skills_dir))
}

fn ensure_platform_skills_dir(platform: &crate::setup_module::Platform, path: &Path) -> bool {
    if let Err(error) = std::fs::create_dir_all(path) {
        tracing::warn!(
            "Could not create skills dir for {}: {}",
            platform.name,
            error
        );
        return false;
    }

    true
}

#[cfg(unix)]
fn install_skill_link(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)?;
    Ok(())
}

#[cfg(not(unix))]
fn install_skill_link(target: &Path, link: &Path) -> Result<()> {
    copy_dir_recursive(target, link)
}

fn remove_paths(paths: &[PathBuf]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

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
