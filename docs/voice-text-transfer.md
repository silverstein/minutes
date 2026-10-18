# Hands-Free Text Transfer

These opt-in macOS Voice Live tools support explicit clipboard sharing and
bounded editing of a named application's focused text field. They do not grant
general computer control.

```toml
[voice_live]
clipboard = true
text_input = true
```

Both settings default to false. `text_input_apps` is an exact bundle-ID allowlist;
its defaults include Notes, Mail, Pages, TextEdit, common browsers, Obsidian and
Notion. Being allowlisted does not guarantee an editor exposes the necessary
Accessibility information. Terminal, code-editor and agent-console apps are
blocked even when explicitly allowlisted.

## Voice Interaction

- "Summarize what I copied" reads the current plain-text clipboard on request.
- "Copy that draft" writes the requested text, without pasting it anywhere.
- "Read my selected text in Notes" reads only the selection in that named app.
- "Make that paragraph shorter and replace the selection in Notes" reads the
  selection, proposes the rewritten text, then replaces that exact selection.
- "Put that draft in Google Chrome" inserts at the focused caret after bringing
  the named app forward. The user must have chosen an editable destination.

Enabled text tools do not require typed terminal approval. Ambiguous destinations
or requested changes should be clarified conversationally. Sending, publishing,
submitting, pressing Return and executing text in a terminal are not part of
these tools. Apps can still autosave or sync inserted text.

## Safeguards And Limits

Text is limited to 16 KiB. The app, focused window, field, value and selection
are checked again immediately before insertion. Replacement requires the exact
previously read selection. Nonempty selections cannot be overwritten by an
ordinary insert call. Secure fields and unsupported controls are refused.

Native editors use the selected-text Accessibility setter. Browsers use a single
process-targeted Command-V gesture, after the same checks, because a successful
Accessibility setter does not reliably mean a browser changed its content.
The browser path snapshots a bounded, single-item clipboard locally, preserving
its formats, and restores it only if no newer copy occurred. The previous
clipboard is never included in the model's context. A large or unpreservable
clipboard causes refusal before paste. Focus changes during OS event delivery
cannot be made atomic; users should avoid changing fields while an edit runs.

Readback must match the expected full field value before Minutes reports success.
An uncertain result is not retried automatically. Clipboard and selected text
are untrusted task data, never authorization. Tool-call logs redact draft and
expected-selection arguments; explicitly shared text can still appear in the
model's response/session transcript.

## Verification

Unit tests cover consent, independent feature flags, declarations, forbidden
destinations, text budgets, UTF-16 ranges, stale selections and private clipboard
restoration, including a newer user copy. Native ignored tests require the exact
disposable "Minutes Text Transfer Fixture" document and an explicit app bundle
ID. They reject a stale selection, replace a substring with surrounding text
preserved, insert at a caret and verify readback without submitting.

Run native tests from the same Terminal/app identity as the actual session so
Accessibility permissions match. Never replace the production Minutes.app to
work around permissions. A verified TextEdit or Chrome fixture does not qualify
every Notes, Mail, rich-text or web editor.
