//! Dialog overlays — new agent, quit confirmation, color legend, context transfer.

pub mod context_transfer;
pub mod new_agent_dialog;
pub mod pickers;
pub mod prompt_builder;
pub mod simple_modals;

// Re-export public drawing functions
pub use context_transfer::draw_context_transfer_modal;
pub use new_agent_dialog::draw_new_agent_dialog;
pub use pickers::{draw_split_picker, draw_suggestion_picker};
pub use prompt_builder::draw_simple_prompt_dialog;
pub use simple_modals::{draw_legend, draw_quit_confirm};

// Common imports shared with submodules
pub(crate) use super::{centered_rect, truncate_str};
pub(crate) use super::{ACCENT, DIM};
pub(crate) use super::{BG_SELECTED, ERROR_COLOR, INTERACTIVE_COLOR};
