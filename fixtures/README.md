# Fixtures

Shared sample data for core tests, UI development, and QA runs. These files are
the contract between the format engines and everything that displays them, so
treat them as inputs to fix code against, not as files to edit when a test
fails.

| File | What it exercises |
| :--- | :---------------- |
| `people.csv` | A small, clean, numeric-and-text table. The default demo file. |
| `quirky.csv` | Quoted commas, embedded newlines, an escaped quote, a ragged row, blank cells, and a bare row. |
| `scores.tsv` | Tab delimiter detection. |
| `empty.csv` | A zero-byte file. |
| `config.json` | A small, nested, pretty-printed JSON document. |
| `broken.json` | A JSON syntax error with a line and column to report. |
| `notes.md` | Headings, inline styles, quotes, lists and tasks, a fenced block, a table, and a local image. |
| `table.md` | A table on its own, with one alignment of each kind. |
| `remote.md` | A remote image, for the loading and network-failure paths. |
| `diagram.svg` | The local image `notes.md` points at. |
| `readme.txt` | Plain text with tabs and a very long line, for horizontal scrolling. |
| `crlf.txt` | CRLF line endings, so a save can be checked byte for byte. |
| `generated/` | Large files produced by `scripts/make-large-fixture.sh`. Not committed. |
