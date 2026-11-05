# The Anthropic API

Claude through the Messages API, streamed, with your API key.

## Setting the key

Get a key from [console.anthropic.com](https://console.anthropic.com), then:

```text
:api-key sk-ant-...
```

Ghostwrite keeps the key where your system keeps secrets, and uses it from then on:

| System | Where |
| --- | --- |
| macOS | the Keychain |
| Windows | the Credential Manager |
| Linux | the Secret Service, by way of `secret-tool` |

So the app needs no environment variable, which an app opened from the Dock or a desktop wouldn't see anyway. Where a program is handed the key, it's on its standard input, never on its command line, where any process could read it. `:api-key` lines are never kept in the command history.

| Command | What it does |
| --- | --- |
| `:api-key key` | save the key and ask Claude with it |
| `:api-key` | say whether a key is saved |
| `:api-key clear` | take it out |

`ANTHROPIC_API_KEY`, if it's set in Ghostwrite's environment, is used before the saved key.

## How it's asked

Every turn of the story is sent, with prompt caching, so a long story stays cheap. The [effort](README.md#effort) is sent with each request.
