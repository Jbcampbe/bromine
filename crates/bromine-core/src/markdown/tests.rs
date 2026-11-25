use std::path::Path;

use super::*;

fn parse(source: &str) -> MarkdownDocument {
    MarkdownDocument::from_str(Path::new("fixtures/notes.md"), source)
}

fn joined_runs(row: &Row) -> String {
    row.runs.iter().map(|run| run.text.as_str()).collect()
}

#[test]
fn a_heading_hides_its_hashes_and_maps_them_back() {
    let document = parse("# Title\n");
    let row = &document.rows()[0];

    assert_eq!(row.text, "Title");
    assert_eq!(row.kind, RowKind::Heading(1));
    assert_eq!(row.gaps, vec![0..2]);
    assert_eq!(row.display_to_source(0), 2);
    assert_eq!(row.source_to_display(0), 0, "the hashes snap to the start");
    assert_eq!(row.source_to_display(1), 0);
    assert_eq!(joined_runs(row), "Title");
}

#[test]
fn inline_markers_are_gaps_and_keep_their_styles() {
    let document = parse("A **bold** and *italic* `code` [link](https://x.test).\n");
    let row = &document.rows()[0];

    assert_eq!(row.text, "A bold and italic code link.");
    let bold = row.runs.iter().find(|run| run.text == "bold").unwrap();
    assert!(bold.style.bold);
    let italic = row.runs.iter().find(|run| run.text == "italic").unwrap();
    assert!(italic.style.italic);
    let code = row.runs.iter().find(|run| run.text == "code").unwrap();
    assert!(code.style.code);
    let link = row.runs.iter().find(|run| run.text == "link").unwrap();
    assert_eq!(link.style.link.as_deref(), Some("https://x.test"));

    assert!(
        row.gaps
            .iter()
            .any(|gap| &document.source()[gap.clone()] == "**"),
        "the bold markers are hidden"
    );
    assert!(
        row.gaps
            .iter()
            .any(|gap| document.source()[gap.clone()].contains("https://x.test")),
        "the destination is hidden"
    );
}

#[test]
fn every_visible_character_round_trips_through_the_source_map() {
    let document = parse(
        "# Title\n\nA **bold** word and `code`.\n\n- item\n- [x] task\n\n```rust\nfn main() {}\n```\n",
    );

    for row in document.rows() {
        for (display, character) in row.text.char_indices() {
            let source = row.display_to_source(display);
            let back = row.source_to_display(source);
            assert_eq!(
                back, display,
                "visible byte {display} ({character:?}) mapped to {source} and back to {back}"
            );
            assert_eq!(
                document.source()[source..].chars().next(),
                Some(character),
                "display text and source disagree at {display}"
            );
        }
    }
}

#[test]
fn list_markers_and_task_state_are_parsed() {
    let document = parse("- one\n- [x] done\n  - nested\n\n1. first\n2. second\n");

    let first = &document.rows()[0];
    assert_eq!(first.kind, RowKind::ListItem);
    assert_eq!(
        first.list,
        Some(ListMarker {
            ordered: false,
            number: None
        })
    );
    assert_eq!(first.text, "one");

    let task = &document.rows()[1];
    assert_eq!(task.kind, RowKind::ListItem);
    assert_eq!(task.task, Some(true));
    assert_eq!(task.text, "done");

    let nested = &document.rows()[2];
    assert_eq!(nested.list_depth, 2);
    assert_eq!(nested.text, "nested");

    let ordered = &document.rows()[4];
    assert_eq!(
        ordered.list,
        Some(ListMarker {
            ordered: true,
            number: Some(1)
        })
    );
}

#[test]
fn a_blockquote_carries_its_depth_and_hides_the_marker() {
    let document = parse("> quoted\n> > deeper\n");

    let first = &document.rows()[0];
    assert_eq!(first.quote_depth, 1);
    assert_eq!(first.text, "quoted");

    let second = &document.rows()[1];
    assert_eq!(second.quote_depth, 2);
    assert_eq!(second.text, "deeper");
}

#[test]
fn a_fenced_code_block_marks_its_fences_and_language() {
    let document = parse("```rust\nfn main() {}\n```\n");

    assert_eq!(document.rows()[0].kind, RowKind::Fence);
    assert_eq!(document.rows()[1].kind, RowKind::Code);
    assert_eq!(document.rows()[1].text, "fn main() {}");
    assert_eq!(document.rows()[2].kind, RowKind::Fence);
    assert!(document.rows()[0].is_hidden_marker_line());

    let block = document
        .blocks()
        .iter()
        .find(|block| matches!(block.kind, BlockKind::Code { .. }))
        .expect("there is a code block");
    let BlockKind::Code { language, fenced } = &block.kind else {
        unreachable!();
    };
    assert_eq!(language, "rust");
    assert!(*fenced);
}

#[test]
fn a_table_exposes_cells_alignments_and_a_grid_structure() {
    let document = parse("| name | score |\n| :--- | ---: |\n| ada  | 412   |\n");

    let table = document.tables().first().expect("there is a table");
    assert_eq!(table.row_count(), 2);
    assert_eq!(table.column_count(), 2);
    assert_eq!(table.alignments, vec![Alignment::Left, Alignment::Right]);
    assert_eq!(table.cell_text(document.source(), 0, 0), "name");
    assert_eq!(table.cell_text(document.source(), 0, 1), "score");
    assert_eq!(table.cell_text(document.source(), 1, 0), "ada");
    assert_eq!(table.cell_text(document.source(), 1, 1), "412");

    assert_eq!(document.rows()[0].kind, RowKind::TableRow { header: true });
    assert_eq!(document.rows()[1].kind, RowKind::TableDelimiter);
    assert_eq!(document.rows()[2].kind, RowKind::TableRow { header: false });

    let position = document.cell_at_offset(table.cell(0, 0).unwrap().source.start);
    assert_eq!(
        position,
        Some(CellPosition {
            table: 0,
            row: 0,
            column: 0
        })
    );
}

#[test]
fn images_resolve_locally_and_only_http_urls_are_remote() {
    let document = parse(
        "![local](diagram.svg)\n\n![remote](https://example.com/a.png)\n\n![bad](ftp://example.com/a.png)\n",
    );

    let local = document.rows()[0].image.clone().expect("local image");
    assert_eq!(local.alt, "local");
    assert_eq!(
        local.location,
        ImageLocation::Local(Path::new("fixtures").join("diagram.svg"))
    );

    let remote = document.rows()[2].image.clone().expect("remote image");
    assert_eq!(
        remote.location,
        ImageLocation::Remote("https://example.com/a.png".to_string())
    );

    let bad = document.rows()[4].image.clone().expect("bad image");
    assert!(matches!(bad.location, ImageLocation::Unresolved(_)));
}

#[test]
fn an_image_only_line_becomes_an_image_row_with_alt_as_caption() {
    let document = parse("![alt text](diagram.svg)\n");

    let row = &document.rows()[0];
    assert_eq!(row.kind, RowKind::Image);
    assert_eq!(row.text, "");
    assert_eq!(row.image.as_ref().unwrap().alt, "alt text");
}

#[test]
fn active_runs_splice_every_hidden_byte_back_in() {
    let document = parse("# A **bold** title\n");
    let row = &document.rows()[0];
    let active = row.active_runs(document.source());
    let rendered: String = active.iter().map(|run| run.text.as_str()).collect();

    assert_eq!(rendered, "# A **bold** title");
    assert!(active.iter().any(|run| run.style.marker));
    assert!(active.iter().any(|run| run.style.bold));
}

#[test]
fn an_edit_reparses_and_keeps_row_n_equal_to_line_n() {
    let mut document = parse("# One\n\ntext\n");
    document.replace(2..5, "Two", 2, 5, crate::EditKind::Other);

    assert_eq!(document.rows()[0].text, "Two");
    assert_eq!(document.text().line_count(), 4);
    assert_eq!(document.rows().len(), 4);

    document.undo().expect("the edit is undone");
    assert_eq!(document.rows()[0].text, "One");
}

#[test]
fn editing_a_cell_escapes_pipes_and_restores_the_caret() {
    let mut document = parse("| a | b |\n| - | - |\n| c | d |\n");
    let position = document
        .cell_at_offset(document.tables()[0].cell(1, 0).unwrap().source.start)
        .unwrap();

    let edit = document
        .replace_cell(position, 0..1, "a|b", 0, crate::EditKind::Typing)
        .expect("the cell is edited");

    assert_eq!(
        document.tables()[0].cell_text(document.source(), 1, 0),
        "a\\|b"
    );
    assert_eq!(
        edit.cell.start + 4,
        edit.caret,
        "the caret follows the escape"
    );
    assert_eq!(&document.source()[edit.caret - 1..edit.caret], "b");
}

#[test]
fn table_structure_edits_regenerate_padding_and_the_delimiter_row() {
    let mut document = parse("| a | b |\n| - | - |\n| c | d |\n");

    document.add_table_row(0, 1).expect("a row can be added");
    assert_eq!(document.tables()[0].row_count(), 3);
    assert_eq!(document.tables()[0].cell_text(document.source(), 2, 0), "");

    document
        .add_table_column(0, 1)
        .expect("a column can be added");
    assert_eq!(document.tables()[0].column_count(), 3);

    document
        .set_table_alignment(0, 0, Alignment::Center)
        .expect("the alignment is set");
    assert_eq!(document.tables()[0].alignments[0], Alignment::Center);
    let delimiter = document.source().lines().nth(1).unwrap();
    assert!(
        delimiter
            .split('|')
            .nth(1)
            .is_some_and(|cell| cell.trim().starts_with(':') && cell.trim().ends_with(':')),
        "the delimiter row carries the alignment: {delimiter}"
    );

    document
        .remove_table_column(0, 0)
        .expect("a column can be removed");
    assert_eq!(document.tables()[0].column_count(), 2);

    document
        .remove_table_row(0, 0)
        .expect("a row can be removed");
    assert_eq!(document.tables()[0].row_count(), 2);

    assert!(
        document.source().lines().all(|line| line.starts_with('|')),
        "the regenerated table is still a table: {}",
        document.source()
    );
}

#[test]
fn text_that_looks_like_markdown_inside_a_fence_is_not_parsed() {
    let document = parse("```\n# not a heading\n- not a list\n```\n");
    assert_eq!(document.rows()[1].kind, RowKind::Code);
    assert_eq!(document.rows()[1].text, "# not a heading");
    assert_eq!(document.rows()[2].kind, RowKind::Code);
    assert!(document.blocks().iter().all(|block| !matches!(
        block.kind,
        BlockKind::Heading(_) | BlockKind::ListItem { .. }
    )));
}

#[test]
fn the_notes_fixture_parses_every_kind_it_documents() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/notes.md");
    let document = MarkdownDocument::load(&path).expect("notes.md loads");

    assert!(
        document
            .blocks()
            .iter()
            .any(|block| matches!(block.kind, BlockKind::Heading(1)))
    );
    assert!(
        document
            .blocks()
            .iter()
            .any(|block| matches!(block.kind, BlockKind::ListItem { .. }))
    );
    assert!(
        document
            .blocks()
            .iter()
            .any(|block| matches!(block.kind, BlockKind::Code { .. }))
    );
    assert!(!document.tables().is_empty());
    assert!(document.rows().iter().any(|row| row.image.is_some()));

    for row in document.rows() {
        for (display, _) in row.text.char_indices() {
            let source = row.display_to_source(display);
            assert_eq!(row.source_to_display(source), display);
        }
    }
}

#[test]
fn an_html_block_hides_its_tags_and_keeps_its_text_and_styles() {
    let document = parse("<h2 align=\"center\">A <em>small</em> <code>x</code> note</h2>\n");
    let row = &document.rows()[0];

    assert_eq!(row.kind, RowKind::Heading(2));
    assert_eq!(row.alignment, Alignment::Center);
    assert_eq!(row.text, "A small x note");
    assert!(!row.text.contains('<'), "tags are not display text");

    let italic = row
        .runs
        .iter()
        .find(|run| run.text == "small")
        .expect("the italic text is a run");
    assert!(italic.style.italic);
    let code = row
        .runs
        .iter()
        .find(|run| run.text == "x")
        .expect("the code text is a run");
    assert!(code.style.code);
}

#[test]
fn html_links_and_strikethrough_keep_their_styles() {
    let document = parse("<p><a href=\"https://example.com\">link</a> <del>gone</del></p>\n");
    let row = &document.rows()[0];

    let link = row
        .runs
        .iter()
        .find(|run| run.text == "link")
        .expect("the link text is a run");
    assert_eq!(link.style.link.as_deref(), Some("https://example.com"));
    let struck = row
        .runs
        .iter()
        .find(|run| run.text == "gone")
        .expect("the struck text is a run");
    assert!(struck.style.strikethrough);
}

#[test]
fn an_html_attribute_value_may_contain_a_greater_than_sign() {
    let document = parse("<p title=\"a > b\">Text</p>\n");
    let row = &document.rows()[0];

    assert_eq!(row.text, "Text");
    assert!(!row.text.contains('>'), "the tag must end at its own `>`");
}

#[test]
fn a_self_closing_hidden_element_does_not_swallow_the_document() {
    let document = parse("<iframe src=\"x\" />\nVisible\n");
    let text = document
        .rows()
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Visible"), "{text:?}");
}

#[test]
fn an_html_break_keeps_the_words_on_either_side_apart() {
    let document = parse("<p>A<br>B</p>\n");
    let row = &document.rows()[0];

    assert_eq!(row.text, "A B");
    assert!(!row.text.contains('<'), "the break tag stays hidden");
}

#[test]
fn an_html_image_becomes_an_image_row_with_its_dimensions() {
    let document = parse(
        "<p align=\"center\">\n  <img src=\"diagram.svg\" width=\"112\" alt=\"A diagram\">\n</p>\n",
    );

    assert_eq!(document.rows()[0].kind, RowKind::Html);
    assert_eq!(document.rows()[0].alignment, Alignment::Center);
    let row = &document.rows()[1];
    assert_eq!(row.kind, RowKind::Image);
    let image = row.image.as_ref().expect("the image tag is captured");
    assert_eq!(image.alt, "A diagram");
    assert_eq!(image.width, Some(112));
    assert_eq!(image.height, None);
    assert!(!image.caption, "HTML alt text is not a caption");
    assert!(matches!(
        &image.location,
        ImageLocation::Local(path) if path.ends_with("fixtures/diagram.svg")
    ));
    assert_eq!(document.rows()[2].kind, RowKind::Html);
}

#[test]
fn html_attribute_entities_are_decoded_before_an_image_is_fetched() {
    let document =
        parse("<img src=\"https://img.example/badge?logo=apple&amp;label=a%2Bb\" alt=\"Badge\">\n");
    let image = document.rows()[0]
        .image
        .as_ref()
        .expect("the remote image is captured");

    assert_eq!(
        image.destination,
        "https://img.example/badge?logo=apple&label=a%2Bb"
    );
    assert_eq!(
        image.location,
        ImageLocation::Remote("https://img.example/badge?logo=apple&label=a%2Bb".into())
    );
}

#[test]
fn a_blank_line_after_a_rule_stays_blank() {
    let document = parse("---\n\nAfter\n");

    assert_eq!(document.rows()[0].kind, RowKind::Rule);
    assert_eq!(document.rows()[1].kind, RowKind::Blank);
    assert_eq!(document.rows()[2].kind, RowKind::Paragraph);
}

#[test]
fn the_project_readme_has_no_visible_html_or_unresolved_images() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    let document = MarkdownDocument::load(&path).expect("the README loads");

    // Code spans are the one place a tag is allowed to stay visible: the README
    // writes `<img>` and `<version>` inside backticks on purpose. Anything
    // outside code is a tag that escaped into the prose.
    let leaked = document
        .rows()
        .iter()
        .flat_map(|row| row.runs.iter())
        .filter(|run| !run.style.code)
        .filter(|run| contains_html_tag(&run.text))
        .map(|run| run.text.as_str())
        .collect::<Vec<_>>();
    assert!(
        leaked.is_empty(),
        "raw HTML tags must not reach the display text: {leaked:?}"
    );
    assert!(
        document
            .rows()
            .iter()
            .any(|row| row.alignment == Alignment::Center),
        "the README header uses centered HTML"
    );

    let image = document
        .rows()
        .iter()
        .find_map(|row| row.image.as_ref())
        .expect("the README draws images");
    assert!(
        matches!(&image.location, ImageLocation::Local(path) if path.ends_with("assets/icon.svg")),
        "the logo resolves beside the README: {:?}",
        image.location
    );
    assert!(
        matches!(&image.location, ImageLocation::Local(path) if path.exists()),
        "the resolved logo exists on disk: {:?}",
        image.location
    );
}

#[test]
fn the_html_fixture_keeps_its_content_and_drops_its_scripts() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/html.md");
    let document = MarkdownDocument::load(&path).expect("the HTML fixture loads");

    let text = document
        .rows()
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("HTML and GFM"), "{text:?}");
    assert!(text.contains("Hidden tags, visible text"), "{text:?}");
    assert!(
        text.contains("The summary and the body text are shown"),
        "{text:?}"
    );
    assert!(
        !text.contains("This script text is hidden"),
        "a script's body is not prose: {text:?}"
    );
    assert!(
        text.contains("GitHub alerts keep their text"),
        "an alert keeps its text: {text:?}"
    );
    assert!(
        text.contains("Footnotes are collected and shown as text"),
        "a footnote definition stays readable: {text:?}"
    );

    let leaked = document
        .rows()
        .iter()
        .flat_map(|row| row.runs.iter())
        .filter(|run| !run.style.code)
        .filter(|run| contains_html_tag(&run.text))
        .map(|run| run.text.as_str())
        .collect::<Vec<_>>();
    assert!(leaked.is_empty(), "tags leaked into the text: {leaked:?}");

    let image = document
        .rows()
        .iter()
        .find_map(|row| row.image.as_ref())
        .expect("the fixture draws an image");
    assert_eq!(image.width, Some(96));
    assert_eq!(image.height, Some(72));
    assert!(
        document
            .rows()
            .iter()
            .any(|row| row.alignment == Alignment::Center),
        "the fixture centers its header"
    );
    assert!(
        document.rows().iter().any(|row| row.task == Some(true)),
        "the fixture has a checked task"
    );
    assert!(
        document
            .rows()
            .iter()
            .flat_map(|row| row.runs.iter())
            .any(|run| run.style.strikethrough),
        "the fixture has struck text"
    );
    assert_eq!(document.tables().len(), 1, "the fixture has one table");
}

/// Whether text still holds an opening tag, a closing tag, or a comment.
///
/// A bare `<` in prose is not a tag, so the next character has to be a letter,
/// `/`, or `!`.
fn contains_html_tag(text: &str) -> bool {
    text.char_indices().any(|(index, character)| {
        character == '<'
            && text[index + 1..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_alphabetic() || next == '/' || next == '!')
    })
}
