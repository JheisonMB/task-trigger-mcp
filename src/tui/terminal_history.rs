//! Terminal command history — per-session storage with global search for autocomplete.
//!
//! Each terminal session stores its command history in a TOML file at:
//!   `~/.canopy/terminals/<session-name>/history.toml`
//!
//! The autocomplete picker searches across ALL terminal sessions' histories.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum entries per session history file.
const MAX_ENTRIES: usize = 500;

// ── Data model ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEntry {
    pub cmd: String,
    pub cwd: String,
    pub last_run: DateTime<Utc>,
    pub count: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionHistory {
    #[serde(default)]
    pub commands: Vec<CommandEntry>,
}

impl SessionHistory {
    /// Record a command execution. Increments count if already present (same cmd+cwd).
    pub fn record(&mut self, cmd: &str, cwd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        let now = Utc::now();
        if let Some(entry) = self
            .commands
            .iter_mut()
            .find(|e| e.cmd == cmd && e.cwd == cwd)
        {
            entry.count += 1;
            entry.last_run = now;
        } else {
            self.commands.push(CommandEntry {
                cmd: cmd.to_string(),
                cwd: cwd.to_string(),
                last_run: now,
                count: 1,
            });
        }
        self.enforce_limit();
    }

    /// LRU eviction: keep the most recently used entries up to MAX_ENTRIES.
    fn enforce_limit(&mut self) {
        if self.commands.len() > MAX_ENTRIES {
            self.commands.sort_by(|a, b| b.last_run.cmp(&a.last_run));
            self.commands.truncate(MAX_ENTRIES);
        }
    }

    /// Filter commands matching a prefix (case-insensitive), ordered by count descending.
    pub fn filter(&self, prefix: &str) -> Vec<&CommandEntry> {
        let prefix_lower = prefix.to_lowercase();
        let mut matches: Vec<&CommandEntry> = self
            .commands
            .iter()
            .filter(|e| e.cmd.to_lowercase().starts_with(&prefix_lower))
            .collect();
        matches.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_run.cmp(&a.last_run)));
        matches
    }

    /// Get unique CWD paths from the history (for cd picker).
    pub fn known_directories(&self) -> Vec<String> {
        let mut dirs: HashMap<&str, u32> = HashMap::new();
        for entry in &self.commands {
            *dirs.entry(&entry.cwd).or_default() += entry.count;
        }
        let mut sorted: Vec<(&str, u32)> = dirs.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.into_iter().map(|(d, _)| d.to_string()).collect()
    }
}

// ── Persistence ─────────────────────────────────────────────────

fn history_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("terminals")
}

fn history_path(data_dir: &Path, session_name: &str) -> PathBuf {
    history_dir(data_dir)
        .join(session_name)
        .join("history.toml")
}

/// Load a session's history from disk.
pub fn load_history(data_dir: &Path, session_name: &str) -> SessionHistory {
    let path = history_path(data_dir, session_name);
    match fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_default(),
        Err(_) => SessionHistory::default(),
    }
}

/// Save a session's history to disk.
pub fn save_history(data_dir: &Path, session_name: &str, history: &SessionHistory) {
    let path = history_path(data_dir, session_name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = toml::to_string_pretty(history) {
        let _ = fs::write(&path, content);
    }
}

/// Load and merge histories from ALL terminal sessions for global search.
pub fn load_all_histories(data_dir: &Path) -> SessionHistory {
    let dir = history_dir(data_dir);
    let mut merged = SessionHistory::default();
    let Ok(entries) = fs::read_dir(&dir) else {
        return merged;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let hist_file = entry.path().join("history.toml");
        if let Ok(content) = fs::read_to_string(&hist_file) {
            if let Ok(hist) = toml::from_str::<SessionHistory>(&content) {
                for cmd in hist.commands {
                    if let Some(existing) = merged
                        .commands
                        .iter_mut()
                        .find(|e| e.cmd == cmd.cmd && e.cwd == cmd.cwd)
                    {
                        existing.count += cmd.count;
                        if cmd.last_run > existing.last_run {
                            existing.last_run = cmd.last_run;
                        }
                    } else {
                        merged.commands.push(cmd);
                    }
                }
            }
        }
    }
    merged
}

// ── Autocomplete picker state ───────────────────────────────────

/// The suggestion picker shown as an overlay when Tab is pressed.
#[derive(Debug)]
#[allow(dead_code)]
pub struct SuggestionPicker {
    /// Current input text (filters suggestions in real time).
    pub input: String,
    /// Whether we're in cd-directory mode vs command-history mode.
    pub mode: PickerMode,
    /// Filtered suggestion entries.
    pub items: Vec<SuggestionItem>,
    /// Currently highlighted index.
    pub selected: usize,
    /// Scroll offset for windowed rendering (first visible item index).
    pub scroll_offset: usize,
    /// For cd mode: the current directory being browsed.
    pub cd_current_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerMode {
    /// History-based command autocomplete.
    CommandHistory,
    /// Directory picker for `cd`.
    CdDirectory,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SuggestionItem {
    /// The text to insert (command or path).
    pub text: String,
    /// Display label (may include count, cwd abbreviation, etc.).
    pub label: String,
    /// Execution count (for sorting/display).
    pub count: u32,
}

impl SuggestionPicker {
    /// Create a new command history picker from session history.
    pub fn from_history(input: &str, session_history: &SessionHistory, cwd: &str) -> Self {
        let matches = session_history.filter(input);
        let items: Vec<SuggestionItem> = matches
            .into_iter()
            .map(|e| {
                let cwd_display = if e.cwd == cwd {
                    ".".to_string()
                } else {
                    abbreviate_path(&e.cwd)
                };
                SuggestionItem {
                    text: e.cmd.clone(),
                    label: format!("{}  ×{}  cwd:{}", e.cmd, e.count, cwd_display),
                    count: e.count,
                }
            })
            .collect();
        Self {
            input: input.to_string(),
            mode: PickerMode::CommandHistory,
            items,
            selected: 0,
            scroll_offset: 0,
            cd_current_dir: None,
        }
    }

    /// Create a directory picker for `cd` from CWD children + history dirs.
    pub fn for_cd(partial: &str, cwd: &str, global_history: &SessionHistory) -> Self {
        let mut items: Vec<SuggestionItem> = Vec::new();

        // 1. List CWD children (directories only)
        if let Ok(entries) = fs::read_dir(cwd) {
            let mut children: Vec<SuggestionItem> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        return None;
                    }
                    Some(SuggestionItem {
                        text: format!("./{name}"),
                        label: format!("./{name}"),
                        count: 0,
                    })
                })
                .collect();
            children.sort_by(|a, b| a.text.cmp(&b.text));
            items.extend(children);
        }

        // 2. Add history-derived directories (deduplicated, not already in CWD children)
        let history_dirs = global_history.known_directories();
        for dir in history_dirs {
            if dir == cwd {
                continue;
            }
            let abbreviated = abbreviate_path(&dir);
            if !items.iter().any(|i| i.text == dir || i.text == abbreviated) {
                items.push(SuggestionItem {
                    text: dir.clone(),
                    label: abbreviated,
                    count: 0,
                });
            }
        }

        // Filter by partial input
        let partial_lower = partial.to_lowercase();
        if !partial.is_empty() {
            items.retain(|i| i.text.to_lowercase().contains(&partial_lower));
        }

        Self {
            input: partial.to_string(),
            mode: PickerMode::CdDirectory,
            items,
            selected: 0,
            scroll_offset: 0,
            cd_current_dir: Some(PathBuf::from(cwd)),
        }
    }

    /// Maximum visible items in the picker window.
    const MAX_VISIBLE: usize = 10;

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    pub fn move_down(&mut self) {
        if !self.items.is_empty() && self.selected < self.items.len() - 1 {
            self.selected += 1;
            if self.selected >= self.scroll_offset + Self::MAX_VISIBLE {
                self.scroll_offset = self.selected + 1 - Self::MAX_VISIBLE;
            }
        }
    }

    /// Returns the slice of items currently visible in the scroll window.
    pub fn visible_items(&self) -> &[SuggestionItem] {
        let end = (self.scroll_offset + Self::MAX_VISIBLE).min(self.items.len());
        &self.items[self.scroll_offset..end]
    }

    /// Visible count for layout sizing.
    pub fn visible_count(&self) -> usize {
        self.visible_items().len()
    }

    /// Navigate into the selected directory (cd mode only).
    pub fn navigate_into(&mut self, base_cwd: &str) -> Option<String> {
        if self.mode != PickerMode::CdDirectory {
            return None;
        }
        let selected_path = self.selected_text()?.to_string();

        // ".." means go to parent
        if selected_path == ".." {
            return self.navigate_parent(base_cwd);
        }

        let current_dir = self.cd_current_dir.as_ref()?.to_string_lossy().to_string();

        let new_path = if let Some(stripped) = selected_path.strip_prefix("./") {
            format!("{}/{}", current_dir, stripped)
        } else if selected_path.starts_with('/') || selected_path.starts_with('~') {
            selected_path.clone()
        } else {
            format!("{}/{}", current_dir, selected_path)
        };

        let path = PathBuf::from(&new_path);
        if path.is_dir() {
            self.cd_current_dir = Some(path);
            self.refresh_items(base_cwd);
            return Some(new_path);
        }
        None
    }

    /// Navigate to parent directory (cd mode only).
    pub fn navigate_parent(&mut self, base_cwd: &str) -> Option<String> {
        if self.mode != PickerMode::CdDirectory {
            return None;
        }
        let current = self.cd_current_dir.as_ref()?;
        if let Some(parent) = current.parent() {
            if parent.to_string_lossy().is_empty() {
                return None;
            }
            let parent_path = parent.to_path_buf();
            let result = parent_path.to_string_lossy().to_string();
            self.cd_current_dir = Some(parent_path);
            self.refresh_items(base_cwd);
            return Some(result);
        }
        None
    }

    /// Refresh the items list based on current cd_current_dir.
    fn refresh_items(&mut self, _base_cwd: &str) {
        if self.mode != PickerMode::CdDirectory {
            return;
        }
        let Some(cwd_path) = self.cd_current_dir.as_ref() else {
            return;
        };
        let cwd = cwd_path.to_string_lossy().to_string();
        let mut items = Vec::new();

        // Show current directory as header hint
        self.input = abbreviate_path(&cwd);

        // Add parent entry if not at root
        if cwd_path
            .parent()
            .is_some_and(|p| !p.to_string_lossy().is_empty())
        {
            items.push(SuggestionItem {
                text: "..".to_string(),
                label: "../".to_string(),
                count: 0,
            });
        }

        // List subdirectories
        if let Ok(entries) = fs::read_dir(&cwd) {
            let mut children: Vec<SuggestionItem> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        return None;
                    }
                    Some(SuggestionItem {
                        text: format!("./{name}"),
                        label: format!("  {name}/"),
                        count: 0,
                    })
                })
                .collect();
            children.sort_by(|a, b| a.text.cmp(&b.text));
            items.extend(children);
        }

        self.items = items;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Get the currently selected item's text for insertion.
    pub fn selected_text(&self) -> Option<&str> {
        self.items.get(self.selected).map(|i| i.text.as_str())
    }
}

/// Abbreviate a path (replace home dir with ~).
fn abbreviate_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_filter() {
        let mut hist = SessionHistory::default();
        hist.record("cargo build", "/home/user/project");
        hist.record("cargo test", "/home/user/project");
        hist.record("cargo build", "/home/user/project");

        assert_eq!(hist.commands.len(), 2);
        let entry = hist
            .commands
            .iter()
            .find(|e| e.cmd == "cargo build")
            .unwrap();
        assert_eq!(entry.count, 2);

        let matches = hist.filter("cargo");
        assert_eq!(matches.len(), 2);
        // cargo build should be first (higher count)
        assert_eq!(matches[0].cmd, "cargo build");
    }

    #[test]
    fn test_filter_case_insensitive() {
        let mut hist = SessionHistory::default();
        hist.record("Cargo Build", "/tmp");
        let matches = hist.filter("cargo");
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_record_empty_ignored() {
        let mut hist = SessionHistory::default();
        hist.record("", "/tmp");
        hist.record("  ", "/tmp");
        assert!(hist.commands.is_empty());
    }

    #[test]
    fn test_known_directories() {
        let mut hist = SessionHistory::default();
        hist.record("ls", "/home/user/a");
        hist.record("ls", "/home/user/a");
        hist.record("pwd", "/home/user/b");

        let dirs = hist.known_directories();
        assert_eq!(dirs[0], "/home/user/a"); // higher count
        assert_eq!(dirs.len(), 2);
    }

    #[test]
    fn test_lru_eviction() {
        let mut hist = SessionHistory::default();
        for i in 0..600 {
            hist.record(&format!("cmd-{i}"), "/tmp");
        }
        assert!(hist.commands.len() <= MAX_ENTRIES);
    }

    #[test]
    fn test_picker_from_history() {
        let mut hist = SessionHistory::default();
        hist.record("cargo build", "/project");
        hist.record("cargo test", "/project");
        hist.record("cargo clippy", "/other");

        let picker = SuggestionPicker::from_history("cargo", &hist, "/project");
        assert_eq!(picker.items.len(), 3);
        assert_eq!(picker.mode, PickerMode::CommandHistory);
    }
}
