# Configuring servers

Some servers need to be told about the project: which interpreter, which environment, which settings. With `"configured_by_code": true` on a server, the plugin's code says what it starts with and answers its settings.

```json
{
  "name": "python",
  "api": 1,
  "code": "python.wasm",
  "servers": [
    { "name": "basedpyright", "command": ["basedpyright-langserver", "--stdio"], "configured_by_code": true },
    { "name": "pylsp", "command": ["pylsp"], "configured_by_code": true }
  ],
  "access": { "project": true, "env": ["PATH", "VIRTUAL_ENV"] }
}
```

## The two calls

```rust
fn server_env(server: String, project: String, root: String) -> Vec<(String, String)>;
fn server_settings(server: String, project: String, root: String, section: String) -> Option<String>;
```

- **`server_env`**: environment variables to set over those the server would have (the login shell's). `project` is the project's folder and `root` the folder in it the server's started in, both as the editor knows them.
- **`server_settings`**: the answer to the server's `workspace/configuration` request for `section`, as JSON, if any.

`server` is the server's `name`, so one plugin can configure several.

## What the code can see

- With `"access": { "project": true }`, the project is mounted read-only at `/project`. The paths the editor passes are as it knows them, so map them under `/project` to look.
- The variables `access.env` lists are in `std::env`, as the server would have them. With `PATH` among them, `which(program)` finds a program on it.

The bundled Python plugin does this: it finds the project's virtualenv (`$VIRTUAL_ENV`, or a `.venv` or `venv` in the server's folder or one above it in the project), starts the server with it, and tells the server its interpreter.
