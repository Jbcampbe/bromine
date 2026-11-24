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
| `generated/` | Large files produced by `scripts/make-large-fixture.sh`. Not committed. |
