use anyhow::Result;
use ratatui::style::Color;

use super::at_picker::AtPicker;
use crate::tui::app::types::Focus;
use std::collections::HashMap;

/// Picker state for adding/removing sections
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SectionPickerMode {
    #[default]
    None,
    AddSection {
        selected: usize,
    },
    RemoveSection {
        selected: usize,
    },
    AddCustom {
        input: String,
    },
    /// Skills picker for the Tools section — entries are `(label, raw_name, prefix)`
    SkillsPicker {
        selected: usize,
        /// `(display_label, raw_name, prefix)` — `prefix` is "skill" or "global"
        entries: Vec<(String, String, String)>,
        /// `None` → create a new tools section on confirm; `Some(id)` → replace content of that section
        replace_id: Option<String>,
    },
}

/// New simplified prompt template dialog with dynamic sections
/// Now supports multiple instances of the same section type
pub struct SimplePromptDialog {
    /// Map of unique section IDs to their content
    pub sections: HashMap<String, String>,
    /// Ordered list of section IDs currently enabled
    pub enabled_sections: Vec<String>,
    /// Which section field is currently focused
    pub focused_section: usize,
    /// Previous focus before opening the dialog
    pub prev_focus: Option<Focus>,
    /// State for the section picker modal
    pub picker_mode: SectionPickerMode,
    /// Counter for generating unique IDs per section type
    pub section_counters: HashMap<String, usize>,
    /// Per-section cursor positions (char index)
    pub section_cursors: HashMap<String, usize>,
    /// Per-section scroll offsets (visual line)
    pub section_scrolls: HashMap<String, usize>,
    /// Active `@`-file picker (inline dropdown), if open.
    pub at_picker: Option<AtPicker>,
    /// Collapsed paste content: placeholder text is stored in `sections`,
    /// the real pasted content lives here and is used for building the prompt.
    pub collapsed_pastes: HashMap<String, String>,
}

impl SimplePromptDialog {
    pub fn new() -> Self {
        let mut counters = HashMap::new();
        counters.insert("instruction".to_string(), 1usize);
        let mut cursors = HashMap::new();
        cursors.insert("instruction".to_string(), 0usize);
        let mut scrolls = HashMap::new();
        scrolls.insert("instruction".to_string(), 0usize);
        let mut dialog = Self {
            sections: HashMap::new(),
            enabled_sections: vec!["instruction".to_string()],
            focused_section: 0,
            prev_focus: None,
            picker_mode: SectionPickerMode::None,
            section_counters: counters,
            section_cursors: cursors,
            section_scrolls: scrolls,
            at_picker: None,
            collapsed_pastes: HashMap::new(),
        };
        dialog
            .sections
            .insert("instruction".to_string(), String::new());
        dialog
    }

    /// Get cursor position for a section
    pub fn cursor(&self, section: &str) -> usize {
        self.section_cursors.get(section).copied().unwrap_or(0)
    }

    /// Get scroll offset for a section
    pub fn scroll(&self, section: &str) -> usize {
        self.section_scrolls.get(section).copied().unwrap_or(0)
    }

    /// Generate unique ID for a section instance
    fn generate_section_id(&mut self, section_name: &str) -> String {
        let counter = self
            .section_counters
            .entry(section_name.to_string())
            .or_insert(0);
        let id = if *counter == 0 {
            section_name.to_string()
        } else {
            format!("{}_{}", section_name, counter)
        };
        *counter += 1;
        id
    }

    /// Add a section instance (can be same type multiple times)
    pub fn add_section(&mut self, section_name: &str) {
        let unique_id = self.generate_section_id(section_name);
        self.enabled_sections.push(unique_id.clone());
        self.sections.insert(unique_id.clone(), String::new());
        self.section_cursors.insert(unique_id.clone(), 0);
        self.section_scrolls.insert(unique_id.clone(), 0);
        self.collapsed_pastes.remove(&unique_id);
        self.focused_section = self.enabled_sections.len() - 1;
    }

    /// Add a section with pre-existing content (used for context transfer and initial content)
    pub fn add_section_with_content(&mut self, section_name: &str, content: String) {
        let unique_id = self.generate_section_id(section_name);
        let cursor_pos = content.chars().count();
        self.enabled_sections.push(unique_id.clone());
        self.sections.insert(unique_id.clone(), content);
        self.section_cursors.insert(unique_id.clone(), cursor_pos);
        self.section_scrolls.insert(unique_id.clone(), 0);
        self.collapsed_pastes.remove(&unique_id);
        self.focused_section = self.enabled_sections.len() - 1;
    }

    /// Remove a specific section instance
    pub fn remove_section(&mut self, section_id: &str) {
        if section_id != "instruction" {
            self.enabled_sections.retain(|s| s != section_id);
            self.sections.remove(section_id);
            self.section_cursors.remove(section_id);
            self.section_scrolls.remove(section_id);
            self.collapsed_pastes.remove(section_id);
            if self.focused_section > 0 {
                self.focused_section = self.focused_section.saturating_sub(1);
            }
        }
    }

    /// Get available section types (these can always be added again)
    pub fn get_available_sections() -> Vec<(&'static str, &'static str)> {
        vec![
            ("instruction", "Instruction"),
            ("context", "Context"),
            ("resources", "Resources"),
            ("examples", "Examples"),
            ("constraints", "Constraints"),
            ("tools", "Tools"),
        ]
    }

    /// Return true if this section ID represents the read-only "tools" section.
    pub fn is_tools_section(section_id: &str) -> bool {
        section_id == "tools" || section_id.starts_with("tools_")
    }

    /// Collect all available skills for the skills picker.
    /// Returns `Vec<(display_label, raw_name, prefix)>`.
    pub fn collect_skills_for_picker(workdir: &std::path::Path) -> Vec<(String, String, String)> {
        let mut entries: Vec<(String, String, String)> = Vec::new();
        let add_from =
            |dir: &std::path::Path, prefix: &str, out: &mut Vec<(String, String, String)>| {
                let Ok(rd) = std::fs::read_dir(dir) else {
                    return;
                };
                for entry in rd.flatten() {
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let Some(raw_name) = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                    else {
                        continue;
                    };
                    if crate::skills_module::find_skill_instructions(&path).is_none() {
                        continue;
                    }
                    // Label uses skill:name format (what the agent sees)
                    let label = format!("skill:{raw_name}");
                    out.push((label, raw_name, prefix.to_string()));
                }
            };
        let project = workdir.join(".agents").join("skills");
        add_from(&project, "skill", &mut entries);
        if let Some(global) = dirs::home_dir().map(|h| h.join(".agents").join("skills")) {
            if global != project {
                add_from(&global, "global", &mut entries);
            }
        }
        entries
    }

    /// Set the content of a specific tools section to a single skill label.
    /// Used by the SkillsPicker to replace the skill in an existing tools section.
    pub fn set_tools_section_skill(&mut self, section_id: &str, label: &str) {
        self.sections
            .insert(section_id.to_string(), label.to_string());
    }

    /// Get section types available to add (can always add more instances)
    pub fn get_addable_sections(&self) -> Vec<(&'static str, &'static str)> {
        Self::get_available_sections()
    }

    /// Get section instances available to remove (not instruction)
    pub fn get_removable_sections(&self) -> Vec<(String, String)> {
        self.enabled_sections
            .iter()
            .filter(|s| *s != "instruction")
            .map(|section_id| {
                // Extract section name from ID (e.g., "context_1" -> "context")
                let section_name = section_id.split('_').next().unwrap_or(section_id.as_str());
                let label = Self::get_available_sections()
                    .into_iter()
                    .find(|(name, _)| *name == section_name)
                    .map(|(_, label)| label)
                    .unwrap_or(section_name);

                // Build display label with instance number
                let display = if section_id.contains('_') {
                    format!("{} {}", label, section_id.rsplit('_').next().unwrap_or(""))
                } else {
                    label.to_string()
                };
                (section_id.clone(), display)
            })
            .collect()
    }

    /// Get the content for a section
    pub fn get_section_content(&self, section_name: &str) -> String {
        self.sections.get(section_name).cloned().unwrap_or_default()
    }

    /// Set the content for a section
    pub fn set_section_content(&mut self, section_name: &str, content: String) {
        self.sections.insert(section_name.to_string(), content);
    }

    /// Get the real content for a section, resolving any collapsed paste.
    pub fn section_content_for_build(&self, section_id: &str) -> Option<&str> {
        self.collapsed_pastes
            .get(section_id)
            .map(|s| s.as_str())
            .or_else(|| self.sections.get(section_id).map(|s| s.as_str()))
    }

    /// Build the final prompt from the filled sections with structured format
    /// Supports multiple instances of each section type
    pub fn build_prompt(&self) -> Result<String> {
        let mut result = String::new();

        // Context sections
        let mut ctx_count = 0;
        for section_id in &self.enabled_sections {
            if section_id.starts_with("context") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if ctx_count == 0 {
                            result.push_str("# [CONTEXT]: Project Background\n");
                            result.push_str("<context>\n");
                        }
                        ctx_count += 1;
                        result.push_str(&format!("  <context_{}>\n", ctx_count));
                        for line in trimmed.lines() {
                            result.push_str(&format!("    {}\n", line));
                        }
                        result.push_str(&format!("  </context_{}>\n\n", ctx_count));
                    }
                }
            }
        }
        if ctx_count > 0 {
            result.push_str("</context>\n\n");
        }

        // Instruction sections
        result.push_str("# [INSTRUCTIONS]: Execution Logic\n");
        result.push_str("<instruction_set>\n");
        let mut instr_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "instruction" || section_id.starts_with("instruction_") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        instr_count += 1;
                        result.push_str(&format!("  <instruction_{}>\n", instr_count));
                        for line in trimmed.lines() {
                            result.push_str(&format!("    {}\n", line));
                        }
                        result.push_str(&format!("  </instruction_{}>\n\n", instr_count));
                    }
                }
            }
        }
        result.push_str("</instruction_set>\n\n");

        // Resources sections
        let mut resources_count = 0;
        for section_id in &self.enabled_sections {
            if section_id.starts_with("resources") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if resources_count == 0 {
                            result.push_str("# [RESOURCES]: Knowledge Base & Data\n");
                            result.push_str("<resources>\n");
                        }
                        resources_count += 1;
                        result.push_str(&format!("  <resource_{}>\n", resources_count));
                        for line in trimmed.lines() {
                            result.push_str(&format!("    {}\n", line));
                        }
                        result.push_str(&format!("  </resource_{}>\n\n", resources_count));
                    }
                }
            }
        }
        if resources_count > 0 {
            result.push_str("</resources>\n\n");
        }

        // Examples sections
        let mut examples_count = 0;
        for section_id in &self.enabled_sections {
            if section_id.starts_with("examples") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if examples_count == 0 {
                            result.push_str("# [EXAMPLES]: Multi-Shot Learning\n");
                            result.push_str("<examples>\n");
                        }
                        examples_count += 1;
                        result.push_str(&format!("  <example_{}>\n", examples_count));
                        for line in trimmed.lines() {
                            result.push_str(&format!("    {}\n", line));
                        }
                        result.push_str(&format!("  </example_{}>\n\n", examples_count));
                    }
                }
            }
        }
        if examples_count > 0 {
            result.push_str("</examples>\n\n");
        }

        // Constraints sections
        let mut constraints_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "constraints" || section_id.starts_with("constraints_") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if constraints_count == 0 {
                            result.push_str("# [CONSTRAINTS]: Behavioral Boundaries\n");
                            result.push_str("<constraints>\n");
                        }
                        constraints_count += 1;
                        result.push_str(&format!("  <constraint_{}>\n", constraints_count));
                        for line in trimmed.lines() {
                            result.push_str(&format!("    {}\n", line));
                        }
                        result.push_str(&format!("  </constraint_{}>\n\n", constraints_count));
                    }
                }
            }
        }
        if constraints_count > 0 {
            result.push_str("</constraints>\n\n");
        }

        // Tools sections
        let mut tools_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "tools" || section_id.starts_with("tools_") {
                if let Some(content) = self.section_content_for_build(section_id) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if tools_count == 0 {
                                result.push_str("# [TOOLS]: Skills & Capabilities\n");
                                result.push_str("<tools>\n");
                            }
                            tools_count += 1;
                            result.push_str(&format!("  <skill_{}>\n", tools_count));
                            result.push_str(&format!("    {}\n", trimmed));
                            result.push_str(&format!("  </skill_{}>\n\n", tools_count));
                        }
                    }
                }
            }
        }
        if tools_count > 0 {
            result.push_str("</tools>\n\n");
        }

        Ok(result)
    }

    /// Replace the `@`-trigger with `@rel_path` in the section text and add the full path
    /// to the resources section (creating one if needed).
    /// Skills are treated as normal file resources — no special content injection.
    pub fn insert_at_completion(
        &mut self,
        section_id: &str,
        rel_path: &str,
        full_path: &str,
        field_width: usize,
    ) {
        let Some(trigger_pos) = self.at_picker.as_ref().map(|p| p.trigger_pos) else {
            return;
        };
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        // The `@` is at trigger_pos; cursor is currently at trigger_pos + 1
        // (we never insert query chars into the text, only into picker.query).
        let replacement: String = format!("@{}", rel_path);
        let new_chars: Vec<char> = chars[..trigger_pos]
            .iter()
            .chain(replacement.chars().collect::<Vec<_>>().iter())
            .chain(chars[(trigger_pos + 1)..].iter())
            .cloned()
            .collect();
        let new_cursor = trigger_pos + replacement.chars().count();
        self.set_section_content(section_id, new_chars.into_iter().collect());
        self.section_cursors
            .insert(section_id.to_string(), new_cursor);
        self.update_section_scroll(section_id, field_width);

        // Add as a resource (skills and files treated uniformly)
        let existing_resources = self
            .enabled_sections
            .iter()
            .find(|id| id.starts_with("resources"))
            .cloned();
        if let Some(res_id) = existing_resources {
            let res_content = self.get_section_content(&res_id);
            let new_res_content = if res_content.is_empty() {
                full_path.to_string()
            } else {
                format!("{}\n{}", res_content, full_path)
            };
            self.set_section_content(&res_id, new_res_content);
        } else {
            self.add_section_with_content("resources", full_path.to_string());
        }
        // NOTE: focused_section is intentionally NOT restored here.
        // The caller (event handler) owns that responsibility and restores it
        // explicitly after this function returns.
    }

    /// Colorize `@word` tokens in rendered section text with a custom accent color.
    pub fn get_file_reference_with_styling(
        &self,
        text: &str,
        accent: Color,
    ) -> Vec<(String, Option<Color>)> {
        let mut result = Vec::new();
        let mut current_pos = 0;

        while let Some(at_pos) = text[current_pos..].find('@') {
            let absolute_pos = current_pos + at_pos;
            if absolute_pos > current_pos {
                result.push((text[current_pos..absolute_pos].to_string(), None));
            }
            let remaining = &text[absolute_pos..];
            let ref_end = remaining
                .find(|c: char| c.is_whitespace() || c == ',' || c == '!' || c == '?' || c == '│')
                .unwrap_or(remaining.len());
            let file_ref = &remaining[..ref_end];
            if file_ref.len() > 1
                && file_ref[1..].chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/'
                })
            {
                result.push((file_ref.to_string(), Some(accent)));
            } else {
                result.push((file_ref.to_string(), None));
            }
            current_pos = absolute_pos + ref_end;
        }
        if current_pos < text.len() {
            result.push((text[current_pos..].to_string(), None));
        }
        result
    }

    /// Count visual (wrapped) lines for a text given a field width
    pub fn visual_line_count(text: &str, field_width: usize) -> usize {
        if field_width == 0 {
            return 1;
        }
        let mut count = 0;
        for line in text.lines() {
            if line.is_empty() {
                count += 1;
            } else {
                count += line.chars().count().div_ceil(field_width);
            }
        }
        count.max(1)
    }

    /// Visual lines occupied by the first `char_idx` chars of text.
    fn visual_lines_to_cursor(text: &str, char_idx: usize, field_width: usize) -> usize {
        let prefix: String = text.chars().take(char_idx).collect();
        Self::visual_line_count(&prefix, field_width).max(1)
    }

    /// Max visible lines for a section type (instruction=5, others=3)
    pub fn max_visible_lines(section_id: &str) -> usize {
        if section_id == "instruction" || section_id.starts_with("instruction_") {
            5
        } else {
            3
        }
    }

    /// Update scroll for a section so the cursor stays visible.
    pub fn update_section_scroll(&mut self, section_id: &str, field_width: usize) {
        let max_vis = Self::max_visible_lines(section_id);
        let text = self
            .sections
            .get(section_id)
            .map(|s| s.as_str())
            .unwrap_or("");
        let cur = self.cursor(section_id);
        let cursor_visual_line =
            Self::visual_lines_to_cursor(text, cur, field_width).saturating_sub(1);

        let scroll = self
            .section_scrolls
            .entry(section_id.to_string())
            .or_insert(0);
        if cursor_visual_line < *scroll {
            *scroll = cursor_visual_line;
        } else if cursor_visual_line >= *scroll + max_vis {
            *scroll = cursor_visual_line + 1 - max_vis;
        }
    }

    /// Move cursor left one char in the given section.
    pub fn move_cursor_left(&mut self, section_id: &str, field_width: usize) {
        let cur = self.cursor(section_id);
        if cur > 0 {
            self.section_cursors.insert(section_id.to_string(), cur - 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Move cursor right one char in the given section.
    pub fn move_cursor_right(&mut self, section_id: &str, field_width: usize) {
        let len = self
            .sections
            .get(section_id)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let cur = self.cursor(section_id);
        if cur < len {
            self.section_cursors.insert(section_id.to_string(), cur + 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Move cursor up one visual line in the given section.
    pub fn move_cursor_up(&mut self, section_id: &str, field_width: usize) {
        let cur = self.cursor(section_id);
        self.section_cursors
            .insert(section_id.to_string(), cur.saturating_sub(field_width));
        self.update_section_scroll(section_id, field_width);
    }

    /// Move cursor down one visual line in the given section.
    pub fn move_cursor_down(&mut self, section_id: &str, field_width: usize) {
        let len = self
            .sections
            .get(section_id)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let cur = self.cursor(section_id);
        self.section_cursors
            .insert(section_id.to_string(), (cur + field_width).min(len));
        self.update_section_scroll(section_id, field_width);
    }

    /// Insert a character at cursor position in any section.
    pub fn insert_char_at_cursor(&mut self, section_id: &str, ch: char, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id).min(chars.len());
        let mut new_chars = chars;
        new_chars.insert(cur, ch);
        let new_content: String = new_chars.into_iter().collect();
        self.set_section_content(section_id, new_content);
        self.section_cursors.insert(section_id.to_string(), cur + 1);
        self.update_section_scroll(section_id, field_width);
    }

    /// Delete the character before cursor in any section.
    pub fn backspace_at_cursor(&mut self, section_id: &str, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id);
        if cur > 0 && cur <= chars.len() {
            let mut new_chars = chars;
            new_chars.remove(cur - 1);
            let new_content: String = new_chars.into_iter().collect();
            self.set_section_content(section_id, new_content);
            self.section_cursors.insert(section_id.to_string(), cur - 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Insert a newline at cursor position in any section.
    pub fn insert_newline_at_cursor(&mut self, section_id: &str, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id).min(chars.len());
        let before: String = chars[..cur].iter().collect();
        let after: String = chars[cur..].iter().collect();
        let new_content = format!("{}\n{}", before, after);
        self.set_section_content(section_id, new_content);
        self.section_cursors.insert(section_id.to_string(), cur + 1);
        self.update_section_scroll(section_id, field_width);
    }

    /// Insert text at cursor position in any section.
    pub fn insert_text_at_cursor(&mut self, section_id: &str, text: &str, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id).min(chars.len());
        let before: String = chars[..cur].iter().collect();
        let after: String = chars[cur..].iter().collect();
        let new_content = format!("{}{}{}", before, text, after);
        self.set_section_content(section_id, new_content);
        self.section_cursors
            .insert(section_id.to_string(), cur + text.chars().count());
        self.update_section_scroll(section_id, field_width);
    }

    /// Insert pasted text. If it spans multiple lines, collapse it to a
    /// `[Pasted ~N lines]` placeholder while keeping the real text for `build_prompt`.
    pub fn insert_collapsed_paste_at_cursor(
        &mut self,
        section_id: &str,
        text: &str,
        field_width: usize,
    ) {
        let line_count = text.lines().count();
        // Collapse if more than one line or very long single line (>200 chars)
        if line_count > 1 || text.chars().count() > 200 {
            // Expand any existing collapsed paste first so we don't lose data
            self.expand_collapsed_paste(section_id);
            let content = self.get_section_content(section_id);
            let chars: Vec<char> = content.chars().collect();
            let cur = self.cursor(section_id).min(chars.len());
            let before: String = chars[..cur].iter().collect();
            let after: String = chars[cur..].iter().collect();

            let real_content = format!("{}{}{}", before, text, after);
            let placeholder = format!("[Pasted ~{} lines]", line_count.max(1));
            let display_content = format!("{}{}{}", before, placeholder, after);

            self.collapsed_pastes
                .insert(section_id.to_string(), real_content);
            self.set_section_content(section_id, display_content);
            self.section_cursors
                .insert(section_id.to_string(), cur + placeholder.chars().count());
            self.update_section_scroll(section_id, field_width);
        } else {
            // Short paste: insert normally
            self.insert_text_at_cursor(section_id, text, field_width);
        }
    }

    /// Expand a collapsed paste for the given section, restoring real content.
    pub fn expand_collapsed_paste(&mut self, section_id: &str) {
        if let Some(real) = self.collapsed_pastes.remove(section_id) {
            self.set_section_content(section_id, real);
        }
    }
}

/// Snapshot of `SimplePromptDialog` state used to persist the prompt builder
/// per workdir across openings within the same canopy session.
#[derive(Clone)]
pub struct PromptBuilderSession {
    pub sections: HashMap<String, String>,
    pub enabled_sections: Vec<String>,
    pub focused_section: usize,
    pub section_counters: HashMap<String, usize>,
    pub section_cursors: HashMap<String, usize>,
    pub section_scrolls: HashMap<String, usize>,
    pub collapsed_pastes: HashMap<String, String>,
}

impl PromptBuilderSession {
    pub fn from_dialog(dialog: &SimplePromptDialog) -> Self {
        Self {
            sections: dialog.sections.clone(),
            enabled_sections: dialog.enabled_sections.clone(),
            focused_section: dialog.focused_section,
            section_counters: dialog.section_counters.clone(),
            section_cursors: dialog.section_cursors.clone(),
            section_scrolls: dialog.section_scrolls.clone(),
            collapsed_pastes: dialog.collapsed_pastes.clone(),
        }
    }

    pub fn restore_into(&self, dialog: &mut SimplePromptDialog) {
        dialog.sections = self.sections.clone();
        dialog.enabled_sections = self.enabled_sections.clone();
        dialog.focused_section = self.focused_section;
        dialog.section_counters = self.section_counters.clone();
        dialog.section_cursors = self.section_cursors.clone();
        dialog.section_scrolls = self.section_scrolls.clone();
        dialog.collapsed_pastes = self.collapsed_pastes.clone();
        // Reset transient UI state
        dialog.picker_mode = SectionPickerMode::None;
        dialog.at_picker = None;
    }
}
