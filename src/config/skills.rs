use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A skill definition from the registry.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Unique skill identifier
    pub id: String,
    /// Display name
    pub name: String,
    /// Description of what the skill does
    pub description: String,
    /// Version string
    pub version: String,
    /// Author or source
    pub author: String,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
    /// Installation instructions per platform
    pub install_paths: Vec<SkillInstallPath>,
}

/// Platform-specific installation path for a skill.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInstallPath {
    pub platform: String,
    pub target_path: String,
    pub content: String,
}

/// Registry of available skills.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsRegistry {
    pub version: u32,
    pub skills: Vec<Skill>,
}

/// Installed skill record.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub id: String,
    pub platforms: Vec<String>,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

impl SkillsRegistry {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            version: 1,
            skills: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn fetch_from_registry() -> Result<Self> {
        Ok(Self::new())
    }

    /// Install a skill to selected platforms.
    #[allow(dead_code)]
    pub fn install_skill(&self, skill: &Skill, target_platforms: &[&str]) -> Result<Vec<String>> {
        let home = dirs::home_dir().context("No home directory")?;
        let mut installed = Vec::new();

        for install_path in &skill.install_paths {
            if !target_platforms.contains(&install_path.platform.as_str()) {
                continue;
            }

            let target = home.join(&install_path.target_path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }

            std::fs::write(&target, &install_path.content)?;
            installed.push(format!("{}:{}", install_path.platform, skill.id));
        }

        Ok(installed)
    }

    /// List installed skills from .canopy/skills.json.
    #[allow(dead_code)]
    pub fn list_installed() -> Result<Vec<InstalledSkill>> {
        let home = dirs::home_dir().context("No home directory")?;
        let skills_file = home.join(".canopy/skills.json");

        if !skills_file.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(&skills_file)?;
        let skills: Vec<InstalledSkill> = serde_json::from_str(&content)?;
        Ok(skills)
    }

    /// Save installed skills record to .canopy/skills.json.
    #[allow(dead_code)]
    pub fn save_installed(skills: &[InstalledSkill]) -> Result<()> {
        let home = dirs::home_dir().context("No home directory")?;
        let canopy_dir = home.join(".canopy");
        std::fs::create_dir_all(&canopy_dir)?;

        let content = serde_json::to_string_pretty(skills)?;
        std::fs::write(canopy_dir.join("skills.json"), content)?;
        Ok(())
    }
}

impl Default for SkillsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
pub fn extract_skills_from_platform(_platform: &str, _skills_dir: &Path) -> Result<Vec<String>> {
    Ok(Vec::new())
}
