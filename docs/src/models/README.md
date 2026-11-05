# Models and backends

The model is reached through one of three backends:

- **[The Anthropic API](api.md)**: Claude, with an API key.
- **[Claude Code](claude-code.md)**: Claude through the `claude` CLI, under your login.
- **[A local model](local.md)**: any GGUF model, run by llama.cpp inside Ghostwrite.

They differ only in how a turn is sent. Every model gets the same prompt and the same read-only commands to look around the project, however small it is.

## Choosing a model

```text
:model
```

`:model` alone says which model is asked, and lists the ones that could be. `:model name` chooses one, with suggestions as you type and Tab to complete:

```text
:model opus-5.5
:model qwen3:4b
```

Names match loosely: `opus55` or `QWEN3` work too.

The list has Claude's models, if the API or Claude Code can be reached:

| `:model` name | Model |
| --- | --- |
| `opus-5.5` | Claude Opus 5.5 (the default) |
| `sonnet-5.5` | Claude Sonnet 5.5 |
| `haiku-5.5` | Claude Haiku 5.5 |
| `fable-5.1` | Claude Fable 5.1 |

Then every local model on this machine: the one named in the config, the ones Ghostwrite downloaded, and those Hugging Face's tools or Ollama downloaded.

Your choice is remembered. Before you've made one, Ghostwrite asks the `local_model` in the [config](../configuration/README.md) if there is one, and otherwise Claude: through the API if there's a key, else through Claude Code if it's installed. The `model` key in the config picks which Claude model that is.

## Effort

The environment variable `GHOSTWRITE_AI_EFFORT` sets how hard Claude thinks, through the API or Claude Code:

| Value | |
| --- | --- |
| `low` | the default: brief thinking, since you're waiting on each step |
| `medium`, `high`, `xhigh`, `max` | more thinking, for harder requests |

```sh
GHOSTWRITE_AI_EFFORT=high ghostwrite ~/code/thing
```

Anything else is taken as `low`. A local model doesn't use it.

## Usage

At the end of each story the status line shows the tokens it used: `done · 12.4k tokens in, 800 out`. Ghostwrite doesn't convert that into a price; your Anthropic console or Claude Code's own usage page has that.

`:timings` shows how long you've been kept waiting. See [Undo, clear and timings](../stories/managing.md#timings).

## With no backend

The editor works fully without any of them. `space a` then says what to do: give it an API key, install Claude Code, or choose a local model.
