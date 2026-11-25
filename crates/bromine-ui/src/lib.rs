//! GPUI views for Bromine.
//!
//! Everything here draws. Anything that can be decided without a window lives in
//! `bromine-core` instead, where it can be tested without a GPU.

pub mod theme;

mod csv_grid;
mod editor;
mod json_view;
mod markdown_image;
mod markdown_row;
mod markdown_table;
mod markdown_view;
pub mod text_actions;
mod text_view;
mod workspace;

pub use csv_grid::{CsvGrid, ROW_HEIGHT};
pub use json_view::{JsonMode, JsonView};
pub use markdown_view::MarkdownView;
pub use text_actions::bind_text_editor_keys;
pub use text_view::TextView;
pub use workspace::{CloseTab, OpenFile, Quit, Save, Workspace};
