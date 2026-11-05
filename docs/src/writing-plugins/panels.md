# Beside a file

A plugin can show a panel beside files of its languages, right of the code, kept up to date from their language server, as Lean's goals are: a view of `blocks`. The manifest says what to ask the server; the code says what to show of the answers.

```json
{
  "views": [{ "name": "goals", "title": "Goals", "shows": "blocks", "place": "beside", "key": "space g" }],
  "panel": {
    "requests": ["$/lean/plainGoal", "$/lean/plainTermGoal"],
    "notifications": ["$/lean/fileProgress"]
  }
}
```

| Field | What it is |
| --- | --- |
| `title` | the heading at the panel's top; its view's title if it doesn't say. The mode line calls it by its view's title (`NOR · goals`) |
| `requests` | requests to the server about the cursor, sent again as it moves or the text changes |
| `notifications` | the server's own notifications about the file that the panel hears of |

A panel needs code, and at least one language. The user goes there with its view's `key` (or `focus_view` and its name), and `:preview` shows or hides it. See [Views](manifest.md#views).

## The three calls

```rust
fn panel_open(language: String) -> Panel;
fn panel_answered(state: String, answers: Vec<Option<String>>) -> Panel;
fn panel_notification(state: String, method: String, params: String) -> (Panel, bool);
```

- **`panel_open`**: the panel for a file of `language`, before its server's been asked anything.
- **`panel_answered`**: the server's answers to `requests` about the cursor, in that order, as JSON, each unless it failed or was empty.
- **`panel_notification`**: one of the `notifications`, its params as JSON. Answer `true` to ask the requests again.

Each answers a `Panel`:

| Field | What it is |
| --- | --- |
| `state` | what the plugin keeps about this file, as it likes |
| `blocks` | what to show |
| `working_on` | the first line the server is still working on, if it's said |

The editor holds `state` for each file and hands it back with the next call about its panel, so the code itself keeps nothing.

## Blocks

| Block | Shows as |
| --- | --- |
| `Block::Heading(text)` | a heading |
| `Block::Code(CodeBlock { text, stands_out })` | text in the code's font; lines starting with `stands_out`, if it's set, stand out |
| `Block::Good(text)` | something good, in a word ("No goals") |
| `Block::Muted(text)` | something quiet |

If a call fails, the panel keeps what it showed, with a muted line saying why.
