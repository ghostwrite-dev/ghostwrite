# Abbreviations and tokens

Both need no code.

## Abbreviations

A language can be typed with abbreviations: a backslash, then a name, turns into a symbol as you type. Name a table in the manifest:

```json
{
  "name": "lean",
  "extensions": ["lean"],
  "abbreviations": "abbreviations.tsv"
}
```

The table is lines of an abbreviation, a tab and its symbol:

```text
alpha	α
to	→
N	ℕ
<>	⟨⟩
```

Typing `\alpha` gives `α`. An abbreviation turns into its symbol as soon as no longer one starts with it, or when a space or another char that can't go on to one is typed. Tab finishes it alone, and `\\` types a backslash. The abbreviation being typed is underlined.

A table with no tabs in it is refused. A plugin with abbreviations needs a folder of its own.

## Tokens

Language servers color code with semantic tokens: kinds of name like `variable`, `type`, `parameter`. A server can also name kinds of its own, which the editor doesn't know. `"tokens"` says which scope each shows as:

```json
{ "tokens": { "leanSorryLike": "constant.builtin" } }
```

The scope has to be one the editor knows; see the list under [Grammars and queries](grammars.md#highlights).
