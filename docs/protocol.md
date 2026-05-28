# sweetpapers control protocol

The daemon binds a Unix socket at `$XDG_RUNTIME_DIR/sweetpapers.sock` and
speaks newline-delimited JSON. One line per request, one line per response,
then the connection closes.

The CLI subcommands all wrap this protocol. Each one opens the socket, sends
one request, prints the reply, and exits. Other tools can talk to the socket
directly: Qt apps via `QLocalSocket`, status-bar widgets, shell scripts
piping through `socat`.

## Request

Every request is tagged by `"cmd"`:

```jsonc
{"cmd": "status"}
{"cmd": "list"}
{"cmd": "pack",      "name": "Background1"}
{"cmd": "next"}                                  // or "prev"
{"cmd": "pause"}                                 // or "resume"
{"cmd": "reload"}
{"cmd": "interval", "secs": 120}
{"cmd": "thumbnail", "name": "Background1", "force": false}
```

## Response

Every response has an `"ok"` boolean. Success responses include a `"data"`
payload; failures include an `"error"` string.

```jsonc
{"ok": true,  "data": <payload>}
{"ok": false, "error": "no such pack: 'Bckground1'"}
```

### Payloads

**`status`**

```jsonc
{
  "pack": "Background1",
  "paused": false,
  "interval_secs": 300,
  "current_directory": "Oceans",
  "monitors": [
    {"screen": "1", "name": "DP-3",     "image": "/home/…/Oceans/1.jpg"},
    {"screen": "2", "name": "HDMI-A-1", "image": "/home/…/Oceans/2.jpg"}
  ]
}
```

**`list`**

```jsonc
{"packs": [
  {"name": "Background1", "thumbnail": "/home/…/.cache/sweetpapers/thumbs/Background1.jpg"},
  {"name": "Nightscape",  "thumbnail": null}
]}
```

`thumbnail` is `null` if the pack has no images or generation failed.
Clients should fall back to a placeholder.

**`thumbnail`**

```jsonc
{"name": "Background1", "path": "/home/…/.cache/sweetpapers/thumbs/Background1.jpg"}
```

**`pack` / `next` / `prev` / `pause` / `resume` / `reload` / `interval`**

No `data` payload on success. Receiving `ok: true` is the acknowledgement.

## Authoritative types

The Rust types live in [`src/proto.rs`](../src/proto.rs) (`Request`,
`Response`, `StatusData`, `ListData`, `PackEntry`, `MonitorState`,
`ThumbnailData`).

## Debugging from the shell

```sh
echo '{"cmd":"status"}' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/sweetpapers.sock
```

Or use the CLI with `--json`:

```sh
sweetpapers status --json
```
