use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::tui::app::types::{AgentEntry, App, Focus, ProjectsPanelFocus, SidebarMode};

// ── Home: screensaver — arrows enter Preview ────────────────────────

pub fn handle_home_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Quit-confirmation overlay intercepts all keys
    if app.quit_confirm {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => app.running = false,
            _ => app.quit_confirm = false,
        }
        return Ok(());
    }

    // Shift+arrows cycle panel focus in Projects mode.
    if app.sidebar_mode == SidebarMode::Projects && modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Up | KeyCode::Left => {
                app.cycle_projects_panel_focus(false);
                app.focus = Focus::Preview;
                return Ok(());
            }
            KeyCode::Down | KeyCode::Right => {
                app.cycle_projects_panel_focus(true);
                app.focus = Focus::Preview;
                return Ok(());
            }
            _ => {}
        }
    }

    // Shift+arrows toggle RagInfo focus in Agents mode (only when chunks exist).
    if app.sidebar_mode == SidebarMode::Agents
        && modifiers.contains(KeyModifiers::SHIFT)
        && app.rag_info.total_chunks > 0
    {
        match code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                app.agents_rag_focused = !app.agents_rag_focused;
                app.focus = Focus::Preview;
                return Ok(());
            }
            _ => {}
        }
    }

    let has_project_preview = app.sidebar_mode == SidebarMode::Projects
        && (!app.projects.is_empty()
            || !app.global_rag_queue.is_empty()
            || app.rag_info.total_chunks > 0);

    match code {
        KeyCode::F(10) if !app.agents.is_empty() => {
            app.dismiss_brain();
            app.log_scroll = 0;
            app.focus = Focus::Preview;
        }
        KeyCode::Esc => {
            app.quit_confirm = true;
        }
        KeyCode::F(1) => {
            app.show_legend = true;
        }
        KeyCode::Down | KeyCode::Char('j') if !app.agents.is_empty() || has_project_preview => {
            app.dismiss_brain();
            if app.sidebar_mode == SidebarMode::Agents {
                app.selected = 0;
            } else {
                // Advance selection immediately so first keypress feels responsive.
                app.select_next();
            }
            app.log_scroll = 0;
            app.focus = Focus::Preview;
        }
        KeyCode::Up | KeyCode::Char('k') if !app.agents.is_empty() || has_project_preview => {
            app.dismiss_brain();
            if app.sidebar_mode == SidebarMode::Agents {
                app.selected = app.agents.len().saturating_sub(1);
            } else {
                app.select_prev();
            }
            app.log_scroll = 0;
            app.focus = Focus::Preview;
        }
        KeyCode::Enter if !app.agents.is_empty() || has_project_preview => {
            app.dismiss_brain();
            app.log_scroll = 0;
            app.focus = Focus::Preview;
        }
        KeyCode::Char('n') => app.open_new_agent_dialog(),
        _ => {}
    }
    Ok(())
}

// ── Preview: navigate agents, Enter → Focus ─────────────────────────

pub fn handle_preview_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // If playground is active, handle playground-specific inputs
    if app.playground_active {
        match code {
            KeyCode::Esc => {
                app.deactivate_playground();
            }
            KeyCode::Up => {
                if app.playground_selected > 0 {
                    app.playground_selected -= 1;
                }
            }
            KeyCode::Down => {
                if app.playground_selected + 1 < app.playground_results.len() {
                    app.playground_selected += 1;
                }
            }
            KeyCode::Backspace => {
                app.playground_query.pop();
                app.playground_last_search = std::time::Instant::now();
            }
            KeyCode::Char('t') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.open_rag_transfer_modal();
            }
            KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                app.playground_query.push(c);
                app.playground_last_search = std::time::Instant::now();
            }
            _ => {}
        }
        return Ok(());
    }

    if app.sidebar_mode == SidebarMode::Projects && modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Up | KeyCode::Left => {
                app.cycle_projects_panel_focus(false);
                return Ok(());
            }
            KeyCode::Down | KeyCode::Right => {
                app.cycle_projects_panel_focus(true);
                return Ok(());
            }
            _ => {}
        }
    }

    // Shift+arrows toggle RagInfo focus in Agents mode (only when chunks exist).
    if app.sidebar_mode == SidebarMode::Agents
        && modifiers.contains(KeyModifiers::SHIFT)
        && app.rag_info.total_chunks > 0
    {
        match code {
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right => {
                app.agents_rag_focused = !app.agents_rag_focused;
                return Ok(());
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Esc | KeyCode::Char('h') => {
            app.focus = Focus::Home;
        }
        KeyCode::Enter | KeyCode::Char('l') => {
            if app.sidebar_mode == SidebarMode::Projects {
                match app.projects_panel_focus {
                    ProjectsPanelFocus::RagInfo => app.activate_playground(),
                    ProjectsPanelFocus::RagQueue => app.toggle_rag_pause(),
                    ProjectsPanelFocus::Projects => {}
                }
                return Ok(());
            }
            // Agents mode: Enter on focused RagInfo → open playground.
            if app.agents_rag_focused {
                app.activate_playground();
                return Ok(());
            }
            // For Group entries: Enter activates the split and enters focus
            if let Some(AgentEntry::Group(idx)) = app.selected_agent() {
                let idx = *idx;
                if let Some(group) = app.split_groups.get(idx) {
                    let id = group.id.clone();
                    app.active_split_id = Some(id);
                    app.split_right_focused = false;
                }
                app.focus = Focus::Agent;
                return Ok(());
            }
            app.log_scroll = 0;
            app.focus = Focus::Agent;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
        }
        KeyCode::Char('e') => {
            app.open_edit_dialog();
        }
        KeyCode::Char('d') => {
            let _ = app.toggle_enable();
        }
        KeyCode::Char('r') => {
            let _ = app.rerun_selected();
        }
        KeyCode::Char('n') => app.open_new_agent_dialog(),
        KeyCode::F(4) => {
            if app.sidebar_mode == SidebarMode::Projects
                && app.projects_panel_focus == ProjectsPanelFocus::Projects
            {
                let _ = app.delete_selected_project();
            } else {
                let _ = app.delete_selected();
            }
        }
        KeyCode::F(10) => {
            app.focus = Focus::Home;
        }
        KeyCode::F(1) => {
            app.show_legend = true;
        }
        _ => {}
    }
    Ok(())
}

// ── Focus: PTY interaction or log scroll ────────────────────────────
