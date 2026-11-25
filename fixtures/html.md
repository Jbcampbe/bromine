<p align="center">
  <img src="diagram.svg" width="96" height="72" alt="The fixture diagram">
</p>

<h1 align="center">HTML and GFM</h1>

<p align="center">
  <strong>Raw HTML</strong> and <em>GitHub-flavoured Markdown</em><br>
  in one document, with <a href="https://github.com/Jbcampbe/bromine">a link</a>.
</p>

---

## Inline HTML

Text can carry <strong>bold</strong>, <em>italic</em>, <code>inline code</code>,
<del>struck text</del>, and a line break<br>without ending the paragraph.

<p align="right">
  This paragraph is right aligned, and its tags are hidden.
</p>

<div align="center">

Centered blocks work too, including <strong>nested inline tags</strong>.

</div>

## GFM constructs

| Feature | Supported |
| :--- | :--- |
| Tables | Yes |
| Strikethrough | ~~Yes~~ |
| Task lists | Yes |
| Footnotes | Yes[^1] |

- [x] Parse the HTML
- [ ] Render every tag

> [!NOTE]
> GitHub alerts keep their text and lose their marker.

<details>
<summary>Hidden tags, visible text</summary>

The summary and the body text are shown; the `<details>` accordion is not.

</details>

<script>
This script text is hidden, because GitHub removes the element entirely.
</script>

[^1]: Footnotes are collected and shown as text.
