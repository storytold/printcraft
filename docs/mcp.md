# MCP conventions

Start explicitly with `pdfcraft-cli mcp [--root DIR] [--compact]`. The server uses JSON lines on
stdin/stdout, opens no port, and stops on EOF. `--root` confines all reads and writes, including
commands invoked through the common tools. Logs stay on stderr.

| Tool | Arguments |
|---|---|
| `command_list` | `doc?`, `filter?`, `enabled_only?`; returns `{commands: [...]}` with mapped tool schemas |
| `command_run` | `id`, `params?` (the mapped task tool's arguments, including `doc` when required) |
| `command_batch` | `steps: [{id, params?}]`, `stop_on_error?` (default true) |
| `doc_inspect` | `doc?`; one document's info or `{documents: [...]}` for the session |
| `render_preview` | `doc?`, `page?` (1-based, default 1), `max_side?` (1–4096, default 1024) |

All existing task tools remain available. Compact mode lists these five tools alongside its
existing core set and `tool_search` / `tool_call`. There are no legacy-name aliases. The full
[automation guide](../crates/automation/README.md) describes the tools, resources, coordinate
system, root confinement, and errors.

```text
command_run   {"id":"file.open","params":{"path":"input.pdf"}}
              → {"doc":1,...}
command_list  {"doc":1,"filter":"rotate"}
command_run   {"id":"page.rotate","params":{"doc":1,"degrees":90}}
render_preview {"doc":1,"page":1,"max_side":512}
```

Every tool has a title and the four MCP annotation hints. Unknown top-level argument keys return
JSON-RPC `-32602`; command parameter keys that do not belong to the mapped task tool are ignored
with warnings. Task failures and escaped tool panics return `isError: true`, and the server keeps
serving. A failed batch includes per-step errors and is marked `isError: true`.

Exports complete synchronously. A call's `progressToken` and cancellation notifications are
harmlessly ignored; the server does not provide background export progress or cancellation.

## Resources and protocol versions

`pdfcraft://document` returns the session's document information (as `doc_inspect` without
arguments), including an empty `documents` array before any PDF is opened. `pdfcraft://commands`
returns the command catalog (as `command_list`). Existing per-document info, text and page-image
resources and their templates are preserved.

Unknown top-level argument keys are refused with JSON-RPC `-32602` (invalid params) instead of
being ignored, so a misspelled argument is reported rather than silently dropped. Clients that
sent extra keys before get an error now.

## Related documentation

- [SDK & REST Service Guide](sdk.md) — Programmatic SDK (Rust, Python, TypeScript) and on-premise REST daemon for application developers.
