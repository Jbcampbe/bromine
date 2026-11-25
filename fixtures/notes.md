# Notes on Bromine

Bromine is a small viewer for the formats developers meet in the wild. It reads
**CSV**, *JSON*, and Markdown, and it never quietly rewrites what it opened.

## Why a line view

A tree hides most of a file behind folds. The line a parser complained about is
usually what a reader wants first, so JSON is shown one formatted line at a time.
The same idea applies here: one source line, one row.

> A viewer that silently normalizes data is lying to the person reading it.

### Things it remembers

1. The source string is the truth.
2. Formatting never reorders keys.
3. An edit is not a save.

- Sorting moves the view, never the data.
- Markers are hidden until the caret arrives.
  - Nested items keep their indentation.
- [x] Show the row and column counts
- [ ] A markup tree

```rust
fn save(document: &mut TextDocument) -> Result<(), CoreError> {
    document.save()
}
```

| Feature | State | Notes |
| :------ | :---: | ----: |
| Grid    | done  | reads |
| Text    | done  | edits |
| Images  | done  | local |

![A small diagram](diagram.svg)

The paragraph after the image stays editable.
