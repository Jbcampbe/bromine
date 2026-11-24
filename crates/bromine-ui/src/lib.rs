//! GPUI views for Bromine.
//!
//! Everything here draws. Anything that can be decided without a window lives in
//! `bromine-core` instead, where it can be tested without a GPU.

pub mod theme;

mod csv_grid;
mod json_view;
mod workspace;

pub use csv_grid::{CsvGrid, ROW_HEIGHT};
pub use json_view::{JsonMode, JsonView};
pub use workspace::{CloseTab, OpenFile, Workspace};
