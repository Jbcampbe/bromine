//! Building rows and blocks from `pulldown-cmark`'s offset events.
//!
//! The one rule that hides markers is this: visible text, code, and HTML
//! events become runs, and every source byte a line holds that no run covers is
//! a gap. Heading hashes, blockquote markers, bullets, emphasis markers,
//! backticks, link brackets and destinations, pipes, task markers, and fences
//! are all syntax the parser does not emit as text, so they need no special
//! case each.

use std::ops::Range;
use std::path::Path;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::{
    Alignment, Block, BlockId, BlockKind, ImageInfo, ImageLocation, ListMarker, Row, RowKind, Run,
    RunStyle, TableBlock, TableCell,
};
use crate::TextDocument;

struct ListContext {
    ordered: bool,
    next: u64,
}

struct ItemContext {
    block: usize,
}

struct TableBuilder {
    alignments: Vec<Alignment>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    header_line: usize,
}

struct Builder<'a> {
    source: &'a str,
    document: &'a TextDocument,
    base_dir: &'a Path,
    line_count: usize,
    runs: Vec<Vec<Run>>,
    blocks: Vec<Block>,
    tables: Vec<TableBuilder>,
    table_ranges: Vec<Range<usize>>,
    current_table: Option<usize>,
    current_table_row: Option<(bool, Vec<TableCell>)>,
    current_cell: Option<Range<usize>>,
    quote_depth: usize,
    quote_ranges: Vec<(Range<usize>, usize)>,
    code_ranges: Vec<(Range<usize>, usize, bool)>,
    item_ranges: Vec<(Range<usize>, usize)>,
    image_spans: Vec<(Range<usize>, ImageInfo)>,
    image_stack: Vec<usize>,
    list_stack: Vec<ListContext>,
    item_stack: Vec<ItemContext>,
    bold: usize,
    italic: usize,
    strikethrough: usize,
    code: usize,
    link: Option<String>,
    html_block: bool,
    html_block_index: Option<usize>,
    hidden_html: usize,
}

/// Parse a text document into rows, blocks, and tables.
pub(super) fn parse(document: &TextDocument) -> (Vec<Row>, Vec<Block>, Vec<TableBlock>) {
    let source = document.text();
    let base_dir = document.path().parent().unwrap_or_else(|| Path::new("."));

    let mut builder = Builder {
        source,
        document,
        base_dir,
        line_count: document.line_count(),
        runs: vec![Vec::new(); document.line_count()],
        blocks: Vec::new(),
        tables: Vec::new(),
        table_ranges: Vec::new(),
        current_table: None,
        current_table_row: None,
        current_cell: None,
        quote_depth: 0,
        quote_ranges: Vec::new(),
        code_ranges: Vec::new(),
        item_ranges: Vec::new(),
        image_spans: Vec::new(),
        image_stack: Vec::new(),
        list_stack: Vec::new(),
        item_stack: Vec::new(),
        bold: 0,
        italic: 0,
        strikethrough: 0,
        code: 0,
        link: None,
        html_block: false,
        html_block_index: None,
        hidden_html: 0,
    };

    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_GFM);

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        builder.event(event, range);
    }

    builder.finish()
}

impl<'a> Builder<'a> {
    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.visible(&text, range, RunStyle::default()),
            Event::Code(text) => self.visible(
                &text,
                range,
                RunStyle {
                    code: true,
                    ..RunStyle::default()
                },
            ),
            Event::Html(text) => self.html(&text, range),
            Event::InlineHtml(text) => self.html(&text, range),
            Event::SoftBreak | Event::HardBreak => {}
            Event::Rule => {
                self.push_block(range.clone(), BlockKind::Rule);
            }
            Event::TaskListMarker(checked) => {
                if let Some(item) = self.item_stack.last()
                    && let Some(block) = self.blocks.get_mut(item.block)
                    && let BlockKind::ListItem { task, .. } = &mut block.kind
                {
                    *task = Some(checked);
                }
            }
            Event::FootnoteReference(text) => {
                self.visible(&text, range, RunStyle::default());
            }
            // Math and metadata are not part of the supported syntax; showing
            // their source as text is more honest than dropping it.
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.visible(&text, range, RunStyle::default());
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph => {
                // A paragraph inside a list item belongs to the item's block,
                // so the bullet, the text, and any task marker activate
                // together. A loose list's paragraph tags must not split it.
                if self.item_stack.is_empty() {
                    self.push_block(range, BlockKind::Paragraph);
                }
            }
            Tag::Heading { level, .. } => {
                self.push_block(range, BlockKind::Heading(heading_level(level)));
            }
            Tag::BlockQuote(_) => {
                self.quote_depth += 1;
                self.quote_ranges.push((range, self.quote_depth));
            }
            Tag::CodeBlock(kind) => {
                let (language, fenced) = match kind {
                    CodeBlockKind::Fenced(info) => (info.to_string(), true),
                    CodeBlockKind::Indented => (String::new(), false),
                };
                let index = self.push_block(range.clone(), BlockKind::Code { language, fenced });
                self.code_ranges.push((range, index, fenced));
            }
            Tag::HtmlBlock => {
                self.html_block = true;
                let index = self.push_block(range, BlockKind::Html);
                self.html_block_index = Some(index);
            }
            Tag::List(start) => {
                self.list_stack.push(ListContext {
                    ordered: start.is_some(),
                    next: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                let marker = match self.list_stack.last_mut() {
                    Some(list) if list.ordered => {
                        let number = list.next;
                        list.next += 1;
                        ListMarker {
                            ordered: true,
                            number: Some(number),
                        }
                    }
                    _ => ListMarker {
                        ordered: false,
                        number: None,
                    },
                };
                let depth = self.list_stack.len();
                let index = self.push_block(
                    range.clone(),
                    BlockKind::ListItem {
                        marker,
                        depth,
                        task: None,
                    },
                );
                self.item_ranges.push((range, index));
                self.item_stack.push(ItemContext { block: index });
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strikethrough += 1,
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                let destination = dest_url.to_string();
                let info = ImageInfo {
                    alt: String::new(),
                    location: resolve_image(self.base_dir, &destination),
                    destination,
                    width: None,
                    height: None,
                    caption: true,
                };
                self.image_spans.push((range, info));
                self.image_stack.push(self.image_spans.len() - 1);
            }
            Tag::Table(alignments) => {
                let alignments = alignments
                    .into_iter()
                    .map(Alignment::from_pulldown)
                    .collect();
                let index = self.tables.len();
                let header_line = self.document.line_of(range.start);
                self.tables.push(TableBuilder {
                    alignments,
                    header: Vec::new(),
                    rows: Vec::new(),
                    header_line,
                });
                self.table_ranges.push(range.clone());
                let block = self.push_block(range, BlockKind::Table);
                self.blocks[block].table = Some(index);
                self.current_table = Some(index);
            }
            Tag::TableHead | Tag::TableRow => {
                let header = matches!(tag, Tag::TableHead);
                self.current_table_row = Some((header, Vec::new()));
            }
            Tag::TableCell => {
                self.current_cell = Some(range);
            }
            Tag::FootnoteDefinition(_) | Tag::MetadataBlock(_) => {
                self.push_block(range, BlockKind::Html);
            }
            // Definition lists, superscript, and subscript are not enabled.
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => {}
            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {}
            TagEnd::HtmlBlock => {
                self.html_block = false;
                self.html_block_index = None;
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
            }
            TagEnd::Item => {
                self.item_stack.pop();
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strikethrough = self.strikethrough.saturating_sub(1),
            TagEnd::Link => self.link = None,
            TagEnd::Image => {
                self.image_stack.pop();
            }
            TagEnd::Table => {
                self.current_table = None;
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let (Some(table), Some((header, cells))) =
                    (self.current_table, self.current_table_row.take())
                    && let Some(builder) = self.tables.get_mut(table)
                {
                    if header {
                        builder.header = cells;
                    } else {
                        builder.rows.push(cells);
                    }
                }
            }
            TagEnd::TableCell => {
                if let (Some(cell), Some((_, cells))) =
                    (self.current_cell.take(), self.current_table_row.as_mut())
                {
                    cells.push(TableCell {
                        source: trim_range(self.source, cell),
                    });
                }
            }
            TagEnd::FootnoteDefinition | TagEnd::MetadataBlock(_) => {}
            TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// Walk an HTML event: hide the tags, keep the text between them.
    ///
    /// GitHub accepts a slice of HTML and sanitizes it for the page. Bromine is
    /// not a browser, so it does the same job for a document: tags become gaps
    /// in the source map, their contents become ordinary styled runs, and the
    /// few tags that change layout (`p`, `div`, `h1`-`h6`, `hr`, `img`, `br`)
    /// are applied to the row or block. The tags themselves are never executed.
    fn html(&mut self, text: &str, range: Range<usize>) {
        let mut cursor = 0;
        while cursor < text.len() {
            let rest = &text[cursor..];
            let Some(open) = rest.find('<') else {
                self.html_text(rest, range.start + cursor);
                break;
            };

            if open > 0 {
                self.html_text(&rest[..open], range.start + cursor);
            }
            let tag_start = cursor + open;
            let after = &text[tag_start..];

            if after.starts_with("<!--") {
                match after.find("-->") {
                    Some(end) => {
                        cursor = tag_start + end + 3;
                        continue;
                    }
                    None => break,
                }
            }

            let Some(close) = tag_end(after) else {
                // Not a well-formed tag; showing the remainder as text is more
                // honest than silently dropping it.
                self.html_text(after, range.start + tag_start);
                break;
            };

            let raw = &after[..=close];
            self.html_tag(
                raw,
                range.start + tag_start..range.start + tag_start + close + 1,
            );
            cursor = tag_start + close + 1;
        }
    }

    fn html_text(&mut self, text: &str, start: usize) {
        if self.hidden_html > 0 {
            return;
        }
        if text.trim().is_empty() {
            // Whitespace around block tags is layout. A space between inline
            // tags is content, so keep one, but indentation at the start of a
            // line is not.
            let line = self.document.line_of(start);
            let line_start = self.document.line_start(line).unwrap_or(0);
            let has_text_before = self
                .source
                .get(line_start..start)
                .is_some_and(|before| !before.trim().is_empty());
            if !text.contains('\n') && !text.is_empty() && has_text_before {
                self.visible(" ", start..start + 1, RunStyle::default());
            }
            return;
        }

        // Indentation and the trailing newline in a line of HTML are layout;
        // spaces inside an inline run are content.
        let content = if text.contains('\n') {
            text.trim_matches(|character: char| character.is_whitespace())
        } else {
            text
        };
        if content.is_empty() {
            return;
        }
        let leading = text.find(content).unwrap_or(0);
        self.visible(
            content,
            start + leading..start + leading + content.len(),
            RunStyle::default(),
        );
    }

    fn html_tag(&mut self, raw: &str, source: Range<usize>) {
        let inner = raw.trim_start_matches('<').trim_end_matches('>').trim();
        if inner.starts_with('!') || inner.starts_with('?') {
            return;
        }

        let closing = inner.starts_with('/');
        let inner = inner.trim_start_matches('/');
        let name_end = inner
            .find(|character: char| character.is_whitespace() || character == '/')
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if name.is_empty() {
            return;
        }

        // GitHub sanitizes these elements out of the page entirely. Showing
        // their contents would be worse than dropping them: a script is not
        // prose and a style block is not a document.
        let hidden_element = matches!(
            name.as_str(),
            "script" | "style" | "template" | "iframe" | "object" | "embed"
        );
        let self_closing = !closing && is_self_closing(raw);
        if self.hidden_html > 0 {
            if hidden_element {
                if closing {
                    self.hidden_html = self.hidden_html.saturating_sub(1);
                } else if !self_closing {
                    self.hidden_html += 1;
                }
            }
            return;
        }
        if hidden_element {
            if !closing && !self_closing {
                self.hidden_html += 1;
            }
            return;
        }

        if closing {
            match name.as_str() {
                "em" | "i" => self.italic = self.italic.saturating_sub(1),
                "strong" | "b" => self.bold = self.bold.saturating_sub(1),
                "code" => self.code = self.code.saturating_sub(1),
                "s" | "del" | "strike" => self.strikethrough = self.strikethrough.saturating_sub(1),
                "a" => self.link = None,
                "blockquote" => self.quote_depth = self.quote_depth.saturating_sub(1),
                _ => {}
            }
            return;
        }

        let attributes = parse_attributes(&inner[name_end..]);
        let attribute = |wanted: &str| {
            attributes
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.clone())
        };

        match name.as_str() {
            "p" | "div" => self.set_html_block(BlockKind::Paragraph, attribute("align").as_deref()),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse::<u8>().unwrap_or(1);
                self.set_html_block(BlockKind::Heading(level), attribute("align").as_deref());
            }
            "hr" => self.set_html_block(BlockKind::Rule, None),
            "img" => {
                let Some(destination) = attribute("src") else {
                    return;
                };
                let alt = attribute("alt").unwrap_or_default();
                let info = ImageInfo {
                    alt,
                    location: resolve_image(self.base_dir, &destination),
                    destination,
                    width: attribute("width").and_then(|value| parse_pixels(&value)),
                    height: attribute("height").and_then(|value| parse_pixels(&value)),
                    caption: false,
                };
                self.image_spans.push((source, info));
            }
            "br" => {
                // A row is one source line, so a `<br>` cannot split it. A
                // space keeps the words on either side from running together,
                // and pointing the run at the tag lets active mode reveal it.
                let line = self.document.line_of(source.start);
                let needs_space = self
                    .runs
                    .get(line)
                    .and_then(|runs| runs.last())
                    .is_some_and(|run| {
                        run.text
                            .chars()
                            .last()
                            .is_some_and(|character| !character.is_whitespace())
                    });
                if needs_space {
                    self.visible(" ", source.clone(), RunStyle::default());
                }
            }
            "em" | "i" => self.italic += 1,
            "strong" | "b" => self.bold += 1,
            "code" => self.code += 1,
            "s" | "del" | "strike" => self.strikethrough += 1,
            "a" => self.link = attribute("href"),
            "blockquote" => self.quote_depth += 1,
            _ => {}
        }
    }

    fn set_html_block(&mut self, kind: BlockKind, align: Option<&str>) {
        let Some(index) = self.html_block_index else {
            return;
        };
        let Some(block) = self.blocks.get_mut(index) else {
            return;
        };
        block.kind = kind;
        if let Some(alignment) = align.and_then(parse_alignment) {
            block.alignment = alignment;
        }
    }

    /// Add a visible event, splitting it across the source lines it covers.
    ///
    /// A code block's text event holds several lines at once, so this is not
    /// only a guard against a stray newline: it is the normal path for code.
    fn visible(&mut self, text: &str, range: Range<usize>, mut style: RunStyle) {
        if let Some(index) = self.image_stack.last().copied() {
            // Alt text is a caption, not body text. Keeping it out of the runs
            // is what stops an image-only line from rendering its alt twice.
            if let Some((_, info)) = self.image_spans.get_mut(index) {
                info.alt.push_str(text);
            }
            return;
        }

        style.bold |= self.bold > 0;
        style.italic |= self.italic > 0;
        style.code |= self.code > 0;
        style.strikethrough |= self.strikethrough > 0;
        style.link = self.link.clone();

        // Inline code's event range includes its backticks while its text does
        // not. Using the range unchanged would make the source map point at a
        // backtick for every character of the code, so the run is narrowed to
        // where the text actually is.
        let content = content_range(self.source, range, text);

        let parts: Vec<&str> = text.split('\n').collect();
        let mut byte = 0;
        for part in parts {
            let line_range = content.start + byte..content.start + byte + part.len();
            if !part.is_empty() {
                let line = self.document.line_of(line_range.start);
                if let Some(runs) = self.runs.get_mut(line) {
                    runs.push(Run {
                        text: part.to_string(),
                        source: line_range,
                        style: style.clone(),
                    });
                }
            }
            byte += part.len() + 1;
        }
    }

    fn push_block(&mut self, source: Range<usize>, kind: BlockKind) -> usize {
        let id = BlockId(self.blocks.len());
        self.blocks.push(Block {
            id,
            kind,
            source,
            rows: 0..0,
            table: None,
            alignment: Alignment::None,
        });
        id.0
    }

    fn finish(mut self) -> (Vec<Row>, Vec<Block>, Vec<TableBlock>) {
        let mut rows: Vec<Row> = (0..self.line_count)
            .map(|line| Row {
                source: self.document.line_range(line).unwrap_or(0..0),
                block: BlockId(usize::MAX),
                kind: RowKind::Blank,
                text: String::new(),
                runs: Vec::new(),
                gaps: Vec::new(),
                quote_depth: 0,
                list_depth: 0,
                list: None,
                task: None,
                image: None,
                alignment: Alignment::None,
            })
            .collect();

        self.tables.sort_by_key(|table| table.header_line);
        let mut tables: Vec<TableBlock> = Vec::new();
        for builder in &self.tables {
            let header_line = builder.header_line;
            let last_line = header_line.saturating_add(builder.rows.len() + 1);
            let lines = header_line..(last_line + 1).min(self.line_count);
            tables.push(TableBlock {
                source: self
                    .table_ranges
                    .iter()
                    .find(|range| self.document.line_of(range.start) == header_line)
                    .cloned()
                    .unwrap_or(0..0),
                lines,
                alignments: builder.alignments.clone(),
                header: builder.header.clone(),
                rows: builder.rows.clone(),
            });
        }

        // A table's block was created before sorting; resolve its index again
        // by the table's start line, which is stable.
        for block in &mut self.blocks {
            if matches!(block.kind, BlockKind::Table) {
                let line = self.document.line_of(block.source.start);
                block.table = tables.iter().position(|table| table.lines.start == line);
            }
        }

        for (line, row) in rows.iter_mut().enumerate() {
            let line_start = row.source.start;
            let line_end = row.source.end;

            let mut chosen: Option<usize> = None;
            for (index, block) in self.blocks.iter().enumerate() {
                let overlaps = if line_start == line_end {
                    // A blank line belongs to a block only when it sits
                    // strictly inside it, such as the empty line in a fenced
                    // block. A blank line at a block's edge is the gap between
                    // blocks and must stay blank.
                    block.source.start < line_start && line_start < block.source.end
                } else {
                    block.source.start < line_end && block.source.end > line_start
                };
                if overlaps {
                    chosen = Some(index);
                }
            }

            let Some(index) = chosen else {
                let id = BlockId(self.blocks.len());
                self.blocks.push(Block {
                    id,
                    kind: BlockKind::Blank,
                    source: row.source.clone(),
                    rows: line..line + 1,
                    table: None,
                    alignment: Alignment::None,
                });
                row.block = id;
                row.kind = RowKind::Blank;
                continue;
            };

            let block = self.blocks[index].clone();
            row.block = block.id;
            apply_kind(row, &block, line, &tables, self.document);
        }

        // Rows an image occupies carry the image, whether or not the image is
        // the only thing on the line. An image-only row becomes an image row;
        // one mixed with text keeps its paragraph kind and draws the image
        // beneath the text.
        for row in rows.iter_mut() {
            for (source, info) in &self.image_spans {
                let overlaps = source.start < row.source.end.max(row.source.start + 1)
                    && source.end > row.source.start;
                if overlaps {
                    row.image = Some(info.clone());
                    break;
                }
            }
            if row.image.is_some() && row.text.trim().is_empty() {
                row.kind = RowKind::Image;
            }
        }

        // Fill in the display text, the gaps, and the quote depth.
        for (line, row) in rows.iter_mut().enumerate() {
            let mut runs = std::mem::take(&mut self.runs[line]);
            runs.sort_by_key(|run| run.source.start);
            runs.retain(|run| !run.text.is_empty());

            row.text = runs.iter().map(|run| run.text.as_str()).collect();
            row.gaps = gaps_in(&row.source, &runs);
            row.runs = runs;
            row.quote_depth = self
                .quote_ranges
                .iter()
                .filter(|(range, _)| {
                    // A nested quote's range starts at its `>` marker, which
                    // is inside the line, so containment alone would miss it.
                    range.start < row.source.end.max(row.source.start + 1)
                        && range.end > row.source.start
                })
                .map(|(_, depth)| *depth)
                .max()
                .unwrap_or(0);

            // A line that held nothing but HTML tags has no visible content.
            // Marking it as HTML lets the view collapse it while the block is
            // inactive and reveal the tag when the block is being edited.
            // Code, fences, rules, and tables keep their own kinds: a `<hr>`
            // is a rule, not a tag line, and a code line that happens to start
            // with `<` is still code.
            if matches!(
                row.kind,
                RowKind::Paragraph | RowKind::Heading(_) | RowKind::ListItem
            ) && row.image.is_none()
                && row.text.trim().is_empty()
                && self.source[row.source.clone()]
                    .trim_start()
                    .starts_with('<')
            {
                row.kind = RowKind::Html;
            }
        }

        for block in &mut self.blocks {
            let mut start = usize::MAX;
            let mut end = 0;
            for (line, row) in rows.iter().enumerate() {
                if row.block == block.id {
                    start = start.min(line);
                    end = end.max(line + 1);
                }
            }
            block.rows = if start == usize::MAX {
                let line = self.document.line_of(block.source.start);
                line..(line + 1).min(self.line_count)
            } else {
                start..end
            };
        }

        (rows, self.blocks, tables)
    }
}

fn apply_kind(
    row: &mut Row,
    block: &Block,
    line: usize,
    tables: &[TableBlock],
    document: &TextDocument,
) {
    let first = document.line_of(block.source.start);
    let last = document
        .line_of(block.source.end.saturating_sub(1))
        .max(first);

    row.alignment = block.alignment;

    match &block.kind {
        BlockKind::Paragraph => row.kind = RowKind::Paragraph,
        BlockKind::Heading(level) => {
            row.kind = if line == first {
                RowKind::Heading(*level)
            } else {
                RowKind::Paragraph
            };
        }
        BlockKind::ListItem {
            marker,
            depth,
            task,
        } => {
            row.list_depth = *depth;
            row.kind = if line == first {
                row.list = Some(*marker);
                row.task = *task;
                RowKind::ListItem
            } else {
                RowKind::Paragraph
            };
        }
        BlockKind::Code { fenced, .. } => {
            let is_fence = |index: usize| {
                document.line_text(index).trim_start().starts_with("```")
                    || document.line_text(index).trim_start().starts_with("~~~")
            };
            row.kind = if line == first || (*fenced && line == last && is_fence(line)) {
                if *fenced {
                    RowKind::Fence
                } else {
                    RowKind::Code
                }
            } else {
                RowKind::Code
            };
        }
        BlockKind::Table => {
            let Some(table) = tables.iter().find(|table| table.lines.start == first) else {
                row.kind = RowKind::Paragraph;
                return;
            };
            if line == table.lines.start {
                row.kind = RowKind::TableRow { header: true };
            } else if line == table.lines.start + 1 {
                row.kind = RowKind::TableDelimiter;
            } else {
                row.kind = RowKind::TableRow { header: false };
            }
        }
        BlockKind::Rule => row.kind = RowKind::Rule,
        BlockKind::Html => row.kind = RowKind::Html,
        BlockKind::Blank => row.kind = RowKind::Blank,
    }
}

fn gaps_in(line: &Range<usize>, runs: &[Run]) -> Vec<Range<usize>> {
    let mut gaps = Vec::new();
    let mut cursor = line.start;
    for run in runs {
        if run.source.start > cursor {
            gaps.push(cursor..run.source.start);
        }
        cursor = cursor.max(run.source.end);
    }
    if cursor < line.end {
        gaps.push(cursor..line.end);
    }
    gaps
}

/// Narrow an event's range to the bytes its text actually occupies.
///
/// Most events already match. Inline code is the exception: its range includes
/// the backticks. Searching for the text inside the range is enough, and a text
/// the parser normalised (a code span with collapsed whitespace) falls back to
/// the original range rather than guessing.
fn content_range(source: &str, range: Range<usize>, text: &str) -> Range<usize> {
    if text.is_empty() {
        return range;
    }
    let Some(slice) = source.get(range.clone()) else {
        return range;
    };
    match slice.find(text) {
        Some(offset) => range.start + offset..range.start + offset + text.len(),
        None => range,
    }
}

fn trim_range(source: &str, range: Range<usize>) -> Range<usize> {
    let bytes = source.as_bytes();
    let mut start = range.start.min(bytes.len());
    let mut end = range.end.min(bytes.len());
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    start..end
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The index of the `>` that closes a tag, ignoring ones inside quotes.
///
/// An attribute value such as `alt="a > b"` is legal HTML, so a plain search
/// for the first `>` would cut the tag in half and leave the rest as text.
fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (index, character) in tag.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => quote = Some(character),
                '>' => return Some(index),
                _ => {}
            },
        }
    }
    None
}

/// Whether the tag ends in `/` outside its quoted attributes.
fn is_self_closing(tag: &str) -> bool {
    let mut quote = None;
    let mut last = None;
    for character in tag.chars() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '"' | '\'' => quote = Some(character),
                '>' => break,
                _ if !character.is_whitespace() => last = Some(character),
                _ => {}
            },
        }
    }
    last == Some('/')
}

fn parse_attributes(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut attributes = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        while index < bytes.len() && (bytes[index].is_ascii_whitespace() || bytes[index] == b'/') {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }

        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
        {
            index += 1;
        }
        if index == name_start {
            index += 1;
            continue;
        }
        let name = input[name_start..index].to_ascii_lowercase();

        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        let mut value = String::new();
        if index < bytes.len() && bytes[index] == b'=' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && (bytes[index] == b'"' || bytes[index] == b'\'') {
                let quote = bytes[index];
                index += 1;
                let start = index;
                while index < bytes.len() && bytes[index] != quote {
                    index += 1;
                }
                value = decode_entities(&input[start..index]);
                if index < bytes.len() {
                    index += 1;
                }
            } else {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && bytes[index] != b'/'
                {
                    index += 1;
                }
                value = decode_entities(&input[start..index]);
            }
        }

        attributes.push((name, value));
    }

    attributes
}

fn decode_entities(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(position) = rest.find('&') {
        decoded.push_str(&rest[..position]);
        let after = &rest[position + 1..];
        if let Some(end) = after.find(';').filter(|end| *end <= 10)
            && let Some(character) = decode_entity(&after[..end])
        {
            decoded.push(character);
            rest = &after[end + 1..];
            continue;
        }
        decoded.push('&');
        rest = after;
    }

    decoded.push_str(rest);
    decoded
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let digits = entity.strip_prefix('#')?;
            let hex = digits
                .strip_prefix('x')
                .or_else(|| digits.strip_prefix('X'));
            let number = if let Some(hex) = hex {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                digits.parse::<u32>().ok()?
            };
            char::from_u32(number)
        }
    }
}

fn parse_alignment(value: &str) -> Option<Alignment> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" => Some(Alignment::Left),
        "center" | "middle" => Some(Alignment::Center),
        "right" => Some(Alignment::Right),
        _ => None,
    }
}

/// Read a pixel dimension. Percentages and other units are ignored rather
/// than guessed at, because a browser's box model is not this view's.
fn parse_pixels(value: &str) -> Option<u32> {
    let digits: String = value
        .trim()
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse::<u32>().ok().filter(|pixels| *pixels > 0)
}

/// Resolve a Markdown image destination to something the view can load.
///
/// Relative paths are resolved against the Markdown file's directory. Only
/// `http` and `https` are fetched; every other scheme is reported as
/// unresolved rather than turning into a request or a filesystem read.
fn resolve_image(base_dir: &Path, destination: &str) -> ImageLocation {
    if destination.starts_with("//") {
        return ImageLocation::Unresolved(destination.to_string());
    }
    if let Some(scheme_end) = destination.find("://") {
        let scheme = destination[..scheme_end].to_ascii_lowercase();
        return match scheme.as_str() {
            "http" | "https" => ImageLocation::Remote(destination.to_string()),
            _ => ImageLocation::Unresolved(destination.to_string()),
        };
    }

    let path = Path::new(destination);
    if path.is_absolute() {
        ImageLocation::Local(path.to_path_buf())
    } else if destination.is_empty() {
        ImageLocation::Unresolved(destination.to_string())
    } else {
        ImageLocation::Local(base_dir.join(path))
    }
}
