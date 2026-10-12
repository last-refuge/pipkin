# Attach to a shared native engine

The native engine remains opt-in. Its default host is a private child of the desktop.
Use an explicit local endpoint when several desktops should share one independently owned
engine. No Pi checkout, Node process, server discovery, or TCP listener is involved.

## Start the server

On Linux, run this in a terminal, using the data/config directories you intend to share:

```sh
pipkin --engine-server "$XDG_RUNTIME_DIR/pipkin-native" \
  --engine-data-dir "$HOME/.local/share/pipkin/engine" \
  --engine-config-dir "$HOME/.config/pipkin/engine"
```

The endpoint directory is created privately if absent. An existing directory must belong to
you, deny access to group/other users, and not be a symlink. Choose an explicit private path
on systems without `XDG_RUNTIME_DIR`. Only one engine may own a data directory; close any
private native desktop already using those same engine files before starting the server.

Server extensions are configured here with repeated `--engine-extension` options. The
headless development binary offers the same host as
`pipkin-engine --data-dir <data> --config-dir <config> --listen <endpoint>`.

## Attach desktops

```sh
pipkin --engine native --engine-attach "$XDG_RUNTIME_DIR/pipkin-native"
```

Additional desktops can use the same endpoint. An optional `--data-dir` selects each desktop's
local cache, drafts and journal; it does not move the server's data. Local desktop state is
namespaced by endpoint, separately from private native and Pi hosts. Do not combine attachment
with `--engine-host` or `--engine-extension`; the server owns that configuration.

Closing an attached desktop disconnects it. Other clients and active runs continue. Any
attached client can stop a shared run, and the first accepted answer to a shared question
wins. Sign-in state stays private to the connection that started it. Connection loss uses the
normal durable catch-up/snapshot/request-lookup recovery path.

Stop the foreground server with Ctrl+C or SIGTERM. Attached clients cannot shut it down.
An orderly stop removes its socket and preserves durable state. SIGKILL can leave a stale
socket; the next owner can reclaim it under the endpoint lock. A live listener, symlink or
ordinary file is never replaced. Restarting the engine recovers durable work according to the
same native tool-replay rules as a private worker.

## Diagnose and bounds

```sh
pipkin --engine native --engine-attach "$XDG_RUNTIME_DIR/pipkin-native" --diagnose --probe
```

The probe authenticates the local peer, performs Hello/ListSessions, reports a count, and
disconnects. It does not launch or stop the server or send model requests.

The endpoint authenticates the OS user through both private filesystem permissions and
kernel peer credentials. Every process running as that same user is within this trust
boundary. It is not a remote-access service or a sandbox.

At most 32 client connections (including unfinished handshakes) are admitted concurrently.
Hello is limited to 4 KiB and three seconds total; later frames use the normal engine frame
limit and bounded queues. Slow writes have a five-second socket timeout. A closed/overloaded
connection is recovered using saved state, never by blindly resending a prompt.

Linux tests cover local socket use, ownership, restart, two desktop backends and the shipped
binary's foreground mode. macOS uses the corresponding peer-credential API but has not been
run for this feature. Windows attachment is not implemented; private native pipes remain
independent of Unix sockets. Full Pi/native differential and installed/live desktop
qualification remain open. Existing `--pi-dir` / `--pi-server-id` flags still select Pi; the
native equivalent is an explicitly selected endpoint, not an attempt to speak Pi's protocol.
