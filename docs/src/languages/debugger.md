# The debugger

Ghostwrite talks to any debug adapter (the Debug Adapter Protocol). By default that's `lldb-dap`, through Xcode on a Mac, which debugs C, C++, Rust, Swift and anything else LLDB can.

## Debugging a program

```text
:break
:debug target/debug/myprogram --verbose
```

1. `:break` puts a breakpoint on the cursor's line, or takes one away.
2. `:debug program args` builds nothing: it runs the program you name, with its arguments, in the debugger.
3. When it stops, the cursor goes there and the line is marked. The calls, the variables and what the program printed show in the side view. `space g` goes there, and again back to the code.
4. Go on with:

| Command | What it does |
| --- | --- |
| `:over` | run to the next line |
| `:step` | step into the call |
| `:finish` | step out of it |
| `:continue` | on to the next breakpoint |
| `:stop` | stop debugging |

These are commands so that you can give them keys of your own. For example:

```json
{
  "keys": {
    "space n": ":over",
    "space i": ":step",
    "space o": ":finish",
    "space c": ":continue"
  }
}
```

## Another debugger

`"debugger"` in the [config](../configuration/README.md) is the adapter's command and its arguments:

```json
{ "debugger": ["lldb-dap"] }
```

The default is `["/usr/bin/xcrun", "lldb-dap"]` on macOS, and `["lldb-dap"]` elsewhere.
