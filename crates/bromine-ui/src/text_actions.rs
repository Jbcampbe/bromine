//! The actions the text editors share, and the key bindings that reach them.
//!
//! The bindings are scoped to the `"TextEditor"` key context. That scope is
//! what keeps `Cmd+C` from suppressing the CSV grid's own copy listener: the
//! grid never enters the context, so the binding never matches there.

use gpui::{App, KeyBinding, actions};

actions!(
    text_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        DocumentStart,
        DocumentEnd,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectHome,
        SelectEnd,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Enter,
        Tab,
        Outdent,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        ToggleBold,
        ToggleItalic,
    ]
);

/// Bind every editing key in the `"TextEditor"` context.
///
/// A view opts in with `.key_context("TextEditor")`; nothing else in the app
/// sees these bindings.
pub fn bind_text_editor_keys(cx: &mut App) {
    const CONTEXT: &str = "TextEditor";
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("cmd-up", DocumentStart, Some(CONTEXT)),
        KeyBinding::new("cmd-down", DocumentEnd, Some(CONTEXT)),
        KeyBinding::new("alt-left", WordLeft, Some(CONTEXT)),
        KeyBinding::new("alt-right", WordRight, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(CONTEXT)),
        KeyBinding::new("shift-alt-left", SelectWordLeft, Some(CONTEXT)),
        KeyBinding::new("shift-alt-right", SelectWordRight, Some(CONTEXT)),
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("enter", Enter, Some(CONTEXT)),
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", Outdent, Some(CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        KeyBinding::new("cmd-z", Undo, Some(CONTEXT)),
        KeyBinding::new("shift-cmd-z", Redo, Some(CONTEXT)),
        KeyBinding::new("cmd-y", Redo, Some(CONTEXT)),
        KeyBinding::new("cmd-b", ToggleBold, Some(CONTEXT)),
        KeyBinding::new("cmd-i", ToggleItalic, Some(CONTEXT)),
    ]);
}
