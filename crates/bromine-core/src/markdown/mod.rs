//! The Markdown model: one source line per row, with markers hidden.
//!
//! The source string is the truth. Every edit goes through the [`TextDocument`]
//! and the rows are rebuilt, so the text, the styled runs, and the source map
//! cannot drift apart. Notes-sized documents are cheap to reparse; a design
//! that kept an incremental tree would buy speed this app does not need and
//! lose the guarantee that row N is always line N.
//!
//! Bear's behaviour, the one this model exists to support, is that markers are
//! hidden while a block is inactive and revealed while the caret is inside it.
//! A row therefore has two renderings: [`Row::text`] and [`Row::runs`] describe
//! the inactive one, and [`Row::active_runs`] splices the hidden source back in
//! with the same styles. [`Row::display_to_source`] and
//! [`Row::source_to_display`] move between them.

mod parse;

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::{CoreError, TextDocument, UndoOutcome};

/// Load a Markdown file from disk.
pub fn load_markdown(path: &Path) -> Result<MarkdownDocument, CoreError> {
    MarkdownDocument::load(path)
}

/// A stable identifier for one block of the document.
///
/// Row order is line order, but several rows belong to one paragraph, list
/// item, code block, or table. The view keeps the block holding the caret
/// active, which is why rows carry an identifier rather than a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub usize);

/// What a block is, for spacing and for the styling its rows inherit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    ListItem {
        marker: ListMarker,
        depth: usize,
        task: Option<bool>,
    },
    Code {
        language: String,
        fenced: bool,
    },
    Table,
    Rule,
    Html,
    Blank,
}

/// One block, with the rows it owns.
#[derive(Debug, Clone)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    pub source: Range<usize>,
    pub rows: Range<usize>,
    /// Index into [`MarkdownDocument::tables`] when this is a table.
    pub table: Option<usize>,
    /// The alignment an HTML `align` attribute asked for.
    pub alignment: Alignment,
}

/// How a list item is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListMarker {
    pub ordered: bool,
    pub number: Option<u64>,
}

/// What a row is, independent of the block it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Blank,
    Paragraph,
    Heading(u8),
    /// The first line of a list item, where the bullet or number is drawn.
    ListItem,
    Code,
    /// An opening or closing code fence, hidden while the block is inactive.
    Fence,
    Rule,
    TableRow {
        header: bool,
    },
    /// The `| --- |` row, never drawn as a grid cell.
    TableDelimiter,
    Image,
    Html,
}

/// The styling of one visible run of text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunStyle {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strikethrough: bool,
    pub link: Option<String>,
    /// A hidden marker, only present while a block is active.
    pub marker: bool,
}

impl RunStyle {
    /// The style for source text that is hidden while the block is inactive.
    pub fn marker() -> Self {
        Self {
            marker: true,
            ..Self::default()
        }
    }
}

/// One visible run of a row, with the source bytes it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub text: String,
    pub source: Range<usize>,
    pub style: RunStyle,
}

/// Where an image points, resolved as far as opening the document can decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageLocation {
    /// A local file, already resolved against the Markdown file's directory.
    Local(PathBuf),
    /// An `http` or `https` URL.
    Remote(String),
    /// A scheme Bromine will not fetch, or a path that could not be resolved.
    Unresolved(String),
}

/// The image an image row draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub alt: String,
    pub destination: String,
    pub location: ImageLocation,
    /// A pixel width from an HTML `width` attribute.
    pub width: Option<u32>,
    /// A pixel height from an HTML `height` attribute.
    pub height: Option<u32>,
    /// Whether the alt text is a Markdown caption worth drawing below the
    /// image. HTML alt text is an accessibility label, not a caption.
    pub caption: bool,
}

/// One source line, rendered.
#[derive(Debug, Clone)]
pub struct Row {
    /// The line's source bytes, excluding the newline.
    pub source: Range<usize>,
    pub block: BlockId,
    pub kind: RowKind,
    /// The inactive rendering: the visible text with markers removed.
    pub text: String,
    /// Styled runs over `text`, each mapped back to `source`.
    pub runs: Vec<Run>,
    /// The source ranges hidden from `text`, in order.
    pub gaps: Vec<Range<usize>>,
    pub quote_depth: usize,
    pub list_depth: usize,
    /// The bullet or number, on the first line of a list item.
    pub list: Option<ListMarker>,
    pub task: Option<bool>,
    /// Present when this row draws an image.
    pub image: Option<ImageInfo>,
    /// The alignment an HTML `align` attribute asked for.
    pub alignment: Alignment,
}

impl Row {
    /// The source offset for a byte position in [`Self::text`].
    ///
    /// A position at the end of a visible run maps to the end of that run's
    /// source, which is before any hidden marker that followed it. That is the
    /// caret position a reader typing at the end of the visible word expects.
    pub fn display_to_source(&self, display: usize) -> usize {
        if self.runs.is_empty() {
            return self.source.start;
        }

        let display = display.min(self.text.len());
        let mut cursor = 0;
        for (index, run) in self.runs.iter().enumerate() {
            let end = cursor + run.text.len();
            if display < end {
                return run.source.start + (display - cursor);
            }
            if display == end {
                // The boundary between two visible runs is also where a
                // hidden marker sits. The caret belongs to the next run, so
                // typing at the start of "bold" in `**bold**` edits the word
                // rather than the asterisks.
                return match self.runs.get(index + 1) {
                    Some(next) => next.source.start,
                    None => run.source.end,
                };
            }
            cursor = end;
        }
        self.source.end
    }

    /// The position in [`Self::text`] for a source offset.
    ///
    /// Offsets inside a hidden marker snap to the boundary where that marker
    /// was removed, so moving the caret through `**bold**` never lands inside
    /// the asterisks in the inactive rendering.
    pub fn source_to_display(&self, source: usize) -> usize {
        let mut cursor = 0;
        for run in &self.runs {
            if source < run.source.start {
                return cursor;
            }
            if source <= run.source.end {
                return cursor + (source - run.source.start).min(run.text.len());
            }
            cursor += run.text.len();
        }
        self.text.len()
    }

    /// The row's raw source line with the hidden markers spliced back in.
    ///
    /// The runs cover the whole line, so the caller can render this with the
    /// same text element it uses for the inactive form. Marker runs carry
    /// [`RunStyle::marker`] so the view can mute them.
    pub fn active_runs(&self, source: &str) -> Vec<Run> {
        let mut runs = Vec::new();
        let mut cursor = self.source.start;

        for run in &self.runs {
            if run.source.start > cursor {
                runs.push(Run {
                    text: source
                        .get(cursor..run.source.start)
                        .unwrap_or("")
                        .to_string(),
                    source: cursor..run.source.start,
                    style: RunStyle::marker(),
                });
            }
            runs.push(Run {
                text: source.get(run.source.clone()).unwrap_or("").to_string(),
                source: run.source.clone(),
                style: run.style.clone(),
            });
            cursor = run.source.end;
        }

        if cursor < self.source.end {
            runs.push(Run {
                text: source
                    .get(cursor..self.source.end)
                    .unwrap_or("")
                    .to_string(),
                source: cursor..self.source.end,
                style: RunStyle::marker(),
            });
        }

        runs
    }

    /// The inactive text with a source range replaced, for building selection
    /// runs without rebuilding the whole row.
    pub fn display_len(&self) -> usize {
        self.text.len()
    }

    /// Whether the row has nothing to draw while inactive.
    pub fn is_hidden_marker_line(&self) -> bool {
        matches!(self.kind, RowKind::Fence | RowKind::TableDelimiter)
    }
}

/// The alignment of one table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    #[default]
    None,
    Left,
    Center,
    Right,
}

impl Alignment {
    fn from_pulldown(alignment: pulldown_cmark::Alignment) -> Self {
        match alignment {
            pulldown_cmark::Alignment::None => Self::None,
            pulldown_cmark::Alignment::Left => Self::Left,
            pulldown_cmark::Alignment::Center => Self::Center,
            pulldown_cmark::Alignment::Right => Self::Right,
        }
    }

    fn marker(self, width: usize) -> String {
        let width = width.max(3);
        match self {
            Self::None => "-".repeat(width),
            Self::Left => format!(":{}", "-".repeat(width - 1)),
            Self::Center => format!(":{}:", "-".repeat(width - 2)),
            Self::Right => format!("{}:", "-".repeat(width - 1)),
        }
    }
}

/// One table cell's source range, trimmed of the padding around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCell {
    pub source: Range<usize>,
}

impl TableCell {
    /// The cell's text in `source`.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        source.get(self.source.clone()).unwrap_or("")
    }
}

/// A table block, with its cells and alignments.
#[derive(Debug, Clone)]
pub struct TableBlock {
    /// The whole table in the source, fences and all.
    pub source: Range<usize>,
    /// The row indices the table occupies, header through last body row.
    pub lines: Range<usize>,
    pub alignments: Vec<Alignment>,
    pub header: Vec<TableCell>,
    pub rows: Vec<Vec<TableCell>>,
}

impl TableBlock {
    /// How many rows the grid shows, header included.
    pub fn row_count(&self) -> usize {
        self.rows.len() + 1
    }

    /// How many columns the grid shows.
    pub fn column_count(&self) -> usize {
        self.alignments
            .len()
            .max(self.header.len())
            .max(self.rows.iter().map(Vec::len).max().unwrap_or(0))
    }

    /// The cell at a grid position, where row zero is the header.
    pub fn cell(&self, row: usize, column: usize) -> Option<&TableCell> {
        if row == 0 {
            self.header.get(column)
        } else {
            self.rows.get(row - 1)?.get(column)
        }
    }

    /// The grid position holding a source offset.
    pub fn cell_at_offset(&self, offset: usize) -> Option<(usize, usize)> {
        for (column, cell) in self.header.iter().enumerate() {
            if cell.source.start <= offset && offset <= cell.source.end {
                return Some((0, column));
            }
        }
        for (index, row) in self.rows.iter().enumerate() {
            for (column, cell) in row.iter().enumerate() {
                if cell.source.start <= offset && offset <= cell.source.end {
                    return Some((index + 1, column));
                }
            }
        }
        None
    }

    /// The source range of the whole table's lines.
    pub fn cell_text<'a>(&self, source: &'a str, row: usize, column: usize) -> &'a str {
        self.cell(row, column)
            .map(|cell| cell.text(source))
            .unwrap_or("")
    }
}

/// A cell position in the grid, with row zero meaning the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPosition {
    pub table: usize,
    pub row: usize,
    pub column: usize,
}

/// The result of editing inside a table cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellEdit {
    /// The cell's source range after the edit and the reparse.
    pub cell: Range<usize>,
    /// Where the caret belongs after the inserted text.
    pub caret: usize,
}

/// A Markdown document: editable source plus the parsed rows and blocks.
#[derive(Debug, Clone)]
pub struct MarkdownDocument {
    text: TextDocument,
    rows: Vec<Row>,
    blocks: Vec<Block>,
    tables: Vec<TableBlock>,
}

impl MarkdownDocument {
    /// Build a model over a text document that has already been loaded.
    pub fn new(text: TextDocument) -> Self {
        let (rows, blocks, tables) = parse::parse(&text);
        Self {
            text,
            rows,
            blocks,
            tables,
        }
    }

    /// Load a Markdown file from disk.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        TextDocument::load(path).map(Self::new)
    }

    /// Build a model from text, for tests and for callers that already have it.
    pub fn from_str(path: &Path, source: &str) -> Self {
        Self::new(TextDocument::from_str(path, source))
    }

    /// The editable source.
    pub fn text(&self) -> &TextDocument {
        &self.text
    }

    /// The source string, normalised to `\n`.
    pub fn source(&self) -> &str {
        self.text.text()
    }

    /// The file this document came from.
    pub fn path(&self) -> &Path {
        self.text.path()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn tables(&self) -> &[TableBlock] {
        &self.tables
    }

    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.get(id.0)
    }

    pub fn is_dirty(&self) -> bool {
        self.text.is_dirty()
    }

    pub fn is_lossy(&self) -> bool {
        self.text.is_lossy()
    }

    /// The row for a source offset.
    pub fn row_of_offset(&self, offset: usize) -> Option<&Row> {
        self.rows.get(self.text.line_of(offset))
    }

    /// The block containing a source offset.
    pub fn block_of_offset(&self, offset: usize) -> Option<BlockId> {
        self.row_of_offset(offset).map(|row| row.block)
    }

    /// The table a block draws, if it is a table.
    pub fn table_of_block(&self, id: BlockId) -> Option<&TableBlock> {
        self.tables.get(self.block(id)?.table?)
    }

    /// The grid cell holding a source offset, if any.
    pub fn cell_at_offset(&self, offset: usize) -> Option<CellPosition> {
        let block = self.block(self.block_of_offset(offset)?)?;
        let table = block.table?;
        let (row, column) = self.tables.get(table)?.cell_at_offset(offset)?;
        Some(CellPosition { table, row, column })
    }

    /// Apply an edit and reparse.
    ///
    /// Full reparsing is deliberate: it keeps row N equal to source line N, and
    /// a notes-sized file parses in microseconds.
    pub fn replace(
        &mut self,
        range: Range<usize>,
        text: &str,
        caret_before: usize,
        caret_after: usize,
        kind: crate::EditKind,
    ) -> usize {
        let caret = self
            .text
            .replace(range, text, caret_before, caret_after, kind);
        self.reparse();
        caret
    }

    pub fn break_undo_group(&mut self) {
        self.text.break_undo_group();
    }

    pub fn undo(&mut self) -> Option<UndoOutcome> {
        let outcome = self.text.undo()?;
        self.reparse();
        Some(outcome)
    }

    pub fn redo(&mut self) -> Option<UndoOutcome> {
        let outcome = self.text.redo()?;
        self.reparse();
        Some(outcome)
    }

    pub fn save(&mut self) -> Result<(), CoreError> {
        self.text.save()
    }

    fn reparse(&mut self) {
        let (rows, blocks, tables) = parse::parse(&self.text);
        self.rows = rows;
        self.blocks = blocks;
        self.tables = tables;
    }

    /// Replace part of a cell's content, escaping pipes and dropping newlines.
    ///
    /// `within` is relative to the cell's trimmed content. The caret is
    /// restored against the reparsed cell rather than a raw offset, so padding
    /// changes around the cell cannot leave it pointing at a pipe.
    pub fn replace_cell(
        &mut self,
        position: CellPosition,
        within: Range<usize>,
        text: &str,
        caret_before: usize,
        kind: crate::EditKind,
    ) -> Option<CellEdit> {
        let table = self.tables.get(position.table)?.clone();
        let cell = table.cell(position.row, position.column)?.source.clone();
        let start = (cell.start + within.start).min(cell.end);
        let end = (cell.start + within.end).min(cell.end).max(start);
        let escaped = escape_cell_text(text);
        let caret_after = start + escaped.len();

        self.text
            .replace(start..end, &escaped, caret_before, caret_after, kind);
        self.reparse();

        let cell = self
            .tables
            .get(position.table)?
            .cell(position.row, position.column)?
            .source
            .clone();
        let caret = (start + escaped.len()).min(cell.end).max(cell.start);
        Some(CellEdit { cell, caret })
    }

    /// Add an empty body row at `at`, where zero is the first body row.
    pub fn add_table_row(&mut self, table: usize, at: usize) -> Option<()> {
        let block = self.tables.get(table)?.clone();
        let mut header = self.row_texts(&block, 0);
        let mut rows: Vec<Vec<String>> = (1..block.row_count())
            .map(|row| self.row_texts(&block, row))
            .collect();
        let columns = block.column_count().max(1);
        header.resize(columns, String::new());
        for row in &mut rows {
            row.resize(columns, String::new());
        }
        rows.insert(at.min(rows.len()), vec![String::new(); columns]);
        self.rewrite_table(table, block.alignments.clone(), header, rows);
        Some(())
    }

    /// Remove a body row. The header is never removed.
    pub fn remove_table_row(&mut self, table: usize, row: usize) -> Option<()> {
        let block = self.tables.get(table)?.clone();
        if block.rows.is_empty() {
            return None;
        }
        let header = self.row_texts(&block, 0);
        let mut rows: Vec<Vec<String>> = (1..block.row_count())
            .map(|index| self.row_texts(&block, index))
            .collect();
        if row >= rows.len() {
            return None;
        }
        rows.remove(row);
        self.rewrite_table(table, block.alignments.clone(), header, rows);
        Some(())
    }

    /// Add an empty column at `at`, growing the delimiter row with it.
    pub fn add_table_column(&mut self, table: usize, at: usize) -> Option<()> {
        let block = self.tables.get(table)?.clone();
        let columns = block.column_count();
        let at = at.min(columns);
        let mut alignments = block.alignments.clone();
        alignments.resize(columns, Alignment::None);
        alignments.insert(at, Alignment::None);
        let mut header = self.row_texts(&block, 0);
        header.resize(columns, String::new());
        header.insert(at, String::new());
        let mut rows: Vec<Vec<String>> = (1..block.row_count())
            .map(|row| {
                let mut cells = self.row_texts(&block, row);
                cells.resize(columns, String::new());
                cells.insert(at, String::new());
                cells
            })
            .collect();
        rows.truncate(block.rows.len());
        self.rewrite_table(table, alignments, header, rows);
        Some(())
    }

    /// Remove a column, keeping at least one.
    pub fn remove_table_column(&mut self, table: usize, column: usize) -> Option<()> {
        let block = self.tables.get(table)?.clone();
        let columns = block.column_count();
        if columns <= 1 || column >= columns {
            return None;
        }
        let mut alignments = block.alignments.clone();
        alignments.resize(columns, Alignment::None);
        alignments.remove(column);
        let mut header = self.row_texts(&block, 0);
        header.resize(columns, String::new());
        header.remove(column);
        let mut rows: Vec<Vec<String>> = (1..block.row_count())
            .map(|row| {
                let mut cells = self.row_texts(&block, row);
                cells.resize(columns, String::new());
                cells.remove(column);
                cells
            })
            .collect();
        rows.truncate(block.rows.len());
        self.rewrite_table(table, alignments, header, rows);
        Some(())
    }

    /// Change one column's alignment.
    pub fn set_table_alignment(
        &mut self,
        table: usize,
        column: usize,
        alignment: Alignment,
    ) -> Option<()> {
        let block = self.tables.get(table)?.clone();
        let columns = block.column_count();
        let mut alignments = block.alignments.clone();
        alignments.resize(columns, Alignment::None);
        *alignments.get_mut(column)? = alignment;
        let header = self.row_texts(&block, 0);
        let rows: Vec<Vec<String>> = (1..block.row_count())
            .map(|row| self.row_texts(&block, row))
            .collect();
        self.rewrite_table(table, alignments, header, rows);
        Some(())
    }

    fn row_texts(&self, table: &TableBlock, row: usize) -> Vec<String> {
        (0..table.column_count())
            .map(|column| unescape_cell_text(table.cell_text(self.source(), row, column)))
            .collect()
    }

    fn rewrite_table(
        &mut self,
        table: usize,
        alignments: Vec<Alignment>,
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    ) {
        let Some(block) = self.tables.get(table) else {
            return;
        };
        let first = block.lines.start;
        let last = block.lines.end.saturating_sub(1).max(first);
        let Some(start) = self.text.line_range(first).map(|range| range.start) else {
            return;
        };
        let end = self
            .text
            .line_range(last)
            .map(|range| range.end)
            .unwrap_or(self.source().len());

        let rendered = render_table(&alignments, &header, &rows);
        self.text
            .replace(start..end, &rendered, start, start, crate::EditKind::Other);
        self.reparse();
    }
}

/// Escape the characters that would break a table cell.
pub fn escape_cell_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_was_backslash = false;
    for character in text.chars() {
        match character {
            '\n' | '\r' => out.push(' '),
            '|' if !previous_was_backslash => out.push_str("\\|"),
            other => out.push(other),
        }
        previous_was_backslash = character == '\\';
    }
    out
}

fn unescape_cell_text(text: &str) -> String {
    text.replace("\\|", "|")
}

/// Render a table back to Markdown with consistent padding.
fn render_table(alignments: &[Alignment], header: &[String], rows: &[Vec<String>]) -> String {
    let columns = alignments
        .len()
        .max(header.len())
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);

    let mut widths = vec![3usize; columns];
    for (column, width) in widths.iter_mut().enumerate() {
        let mut longest = header
            .get(column)
            .map(|cell| cell.chars().count())
            .unwrap_or(0);
        for row in rows {
            longest = longest.max(
                row.get(column)
                    .map(|cell| cell.chars().count())
                    .unwrap_or(0),
            );
        }
        *width = longest.max(3);
    }

    let mut out = String::new();
    out.push_str(&render_table_line(header, &widths, columns));
    out.push('\n');
    let markers: Vec<String> = (0..columns)
        .map(|column| {
            alignments
                .get(column)
                .copied()
                .unwrap_or(Alignment::None)
                .marker(widths[column])
        })
        .collect();
    out.push_str(&render_table_line(&markers, &widths, columns));
    for row in rows {
        out.push('\n');
        out.push_str(&render_table_line(row, &widths, columns));
    }
    out
}

fn render_table_line(cells: &[String], widths: &[usize], columns: usize) -> String {
    let mut line = String::new();
    for (column, width) in widths.iter().enumerate().take(columns) {
        let cell = cells.get(column).map(String::as_str).unwrap_or("");
        line.push_str("| ");
        line.push_str(cell);
        let padding = width.saturating_sub(cell.chars().count());
        for _ in 0..padding {
            line.push(' ');
        }
        line.push(' ');
    }
    line.push('|');
    line
}

#[cfg(test)]
mod tests;
