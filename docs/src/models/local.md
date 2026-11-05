# Local models

llama.cpp is built into Ghostwrite. A local model needs nothing else installed or started: no server, no port.

## Choosing one

Name a model on Hugging Face with its quantization, or a `.gguf` file, in the config:

```json
{ "local_model": "bartowski/Qwen2.5-Coder-14B-Instruct-GGUF:Q3_K_M" }
```

It's downloaded the first time (a cut-off download carries on where it stopped), into Ghostwrite's cache folder, and loaded as you start typing a request.

Or choose one with `:model`. It lists every local model on this machine:

- those Ghostwrite downloaded;
- those Hugging Face's tools downloaded, in `~/.cache/huggingface/hub`;
- those Ollama downloaded, in `~/.ollama/models`.

## How it runs

The model runs in a process of its own: this same program, started again. If llama.cpp fails, it can't take your unsaved work with it, and ending the process is the one sure way to give its memory back.

- It runs on the GPU with Metal on a Mac; elsewhere, on the CPU for now.
- It uses an 8-bit KV cache and a small context. Every reply is one small step, so it's sent only what that step needs, and earlier steps shrink to a line each. (`context_bytes` and `max_diff` are for the other backends.)
- It's held to the reply format by a grammar: it can't write anything but a well-formed item, nor name a file that isn't there.
- It's asked one item ahead at most.

The process ends when the app does, when `:model` switches away, and after 5 idle minutes, so its memory is only held while you use it.

## Memory

On a Mac a model's memory is wired: it can't be paged out, and with nearly all of memory wired macOS can panic. So:

- a model only loads if it fits;
- there's only one at a time, across every running Ghostwrite;
- if memory runs out later (to other GPU-heavy work, say), the model stops, before the Mac does.

Elsewhere, what's counted is the memory in use: nothing panics, but the machine slows down.

## Images and PDFs

Local models are told they can only read text. When one reads an image or a PDF, it's told so, rather than shown the file.

## Building without it

To build Ghostwrite without llama.cpp, and without needing `cmake` and a C++ compiler:

```sh
cargo build --release --no-default-features --features wasm-plugins
```
