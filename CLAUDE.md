# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

IonConnect — Rust rewrite of Barrier/Input Leap: shares mouse, keyboard, and clipboard between machines on the same LAN (Windows 11 and Ubuntu, X11 and Wayland). One machine is the **server** (owns the physical mouse/keyboard, captures input, decides hand-off), the others are **clients** (receive and inject input).

Current known limitation: the server role only works on Ubuntu X11 today — capture-with-edge-detection isn't implemented for Windows or Wayland yet, though both work fine as clients (`README.md` has the full roadmap/limitations list, keep it in sync if you close one of these).

Code comments and commit messages in this repo are written in **Spanish** — match that when editing existing files.

## Commands

```bash
# build the two shippable binaries
cargo build --release -p ionconnect-gui -p ionconnect-core

# run every crate's tests except the GUI (Tauri needs a display/webkit env)
cargo test --workspace --exclude ionconnect-gui

# single crate / single test
cargo test -p ionconnect-core
cargo test -p ionconnect-core authenticated_peer_receives_routed_mouse_move

# lint — workspace enables clippy::all + clippy::pedantic as warnings (Cargo.toml [workspace.lints])
cargo clippy --workspace --exclude ionconnect-gui

# run a node locally against ~/.config/ionconnect/config.toml
cargo run -p ionconnect-core

# the X11 tests that need a real X server are #[ignore]d — run them in a
# nested display, never against your own session (they inject keys/clicks,
# take the clipboard and start/cancel drags)
Xephyr :80 -screen 1024x768 -ac &
DISPLAY=:80 cargo test -p ionconnect-input --test x11_smoke -- --ignored --test-threads=1
DISPLAY=:80 cargo test -p ionconnect-input --test x11_drag -- --ignored --test-threads=1      # needs python3-gi (GTK 3 + 4)
DISPLAY=:80 cargo test -p ionconnect-clipboard --test x11_files -- --ignored --test-threads=1
```

Linux build dependencies (for the GUI/Tauri crate and packaging): `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `libdbus-1-dev`, `build-essential`.

Some tests exercise real resources rather than mocks: real TLS handshakes over loopback (`crypto/tests/loopback.rs`), real X11 via Xephyr (`input/tests/x11_smoke.rs`), a real server+client pair running concurrently (`core/src/server.rs` tests). Keep that pattern — this codebase prefers integration-style tests over mocking its own dependencies where a real resource is available in CI.

## Architecture

Cargo workspace, one crate per responsibility (`Cargo.toml` lists members). Dependency direction flows roughly top-to-bottom:

| Crate | Responsibility |
|---|---|
| `shared` | Common types (`DeviceId`, `KeyModifiers`) and `file://` URI helpers used everywhere |
| `protocol` | Binary wire protocol — `Message` enum, encode/decode (`protocol/src/message.rs`, `codec.rs`) |
| `crypto` | Mutual TLS 1.3 + TOFU (trust-on-first-use) trust by certificate fingerprint |
| `network` | Tokio transport: TCP+TLS framing/reconnect/backoff/mDNS discovery, plus a **separate encrypted UDP channel** for high-frequency `MouseMove` deltas (`network/src/udp_codec.rs`) |
| `input` | Per-OS capture/injection backends: `input/src/x11`, `input/src/win32`, `input/src/wayland` |
| `screen` | Multi-monitor geometry (`MonitorGeometry`, `VirtualDesktop`) and screen-edge layout (`Layout`, `ScreenEdge`) — pure logic, no I/O |
| `clipboard` | Clipboard sync (text and copied file lists) with loop-guard to avoid re-broadcasting a change that was just applied remotely |
| `config` | TOML config with hot-reload watcher (role, peers, listen port, pairing mode) |
| `ipc` | Local GUI↔core channel authenticated by token |
| `core` | Orchestrator binary (`ionconnect-core`) — wires capture→network→injection together; see below |
| `gui` | Tauri control-panel app (`ionconnect-gui`) |

### `core`: server vs client

`core/src/main.rs` reads `Settings.role` and dispatches to `server::run_server` or `client::run_client`. Both load/generate a TLS identity (`core/src/identity.rs`) and a file-backed trust store (`core/src/trust_store.rs`) from `~/.config/ionconnect/`.

**Server** (`core/src/server.rs`):
- Builds a `Layout` linking the local desktop to each configured peer by `ScreenEdge`, wrapped in `HandoffState` (`core/src/handoff.rs`) — a pure state machine, no knowledge of X11/network/sockets, that turns cursor position reports into `ForwardTo`/`ReturnLocal` decisions.
- Accepts TCP+TLS connections per peer (`accept_connections`/`handle_peer_connection`), authenticates, registers the peer in `Routing` (fan-out message dispatch) and `UdpPeers`.
- Input capture is backend-specific and lives in `core/src/input_session.rs` (X11 blocking thread, or inline Wayland portal session via `tokio::select!` alongside the accept loop — Wayland's `reis`-based session isn't `Send`, so it can't be `tokio::spawn`ed). Both loops share `SessionState`, which holds the duplicate filter, the key repeater, and the `MouseMove` rate limiter (`MOUSE_SEND_INTERVAL`, ~250Hz, with the last position flushed on the idle tick so a movement's endpoint is never dropped).
- Also runs the clipboard poll/broadcast loop.

**Client** (`core/src/client.rs`):
- Server address: `server_address` (accepts `host:port`, resolved via `lookup_host`) wins if set; otherwise `resolve_server_addresses` browses mDNS (5s, configured `peers` first, IPv4 first) on **every** reconnect attempt and tries each candidate, so a server that changes IP/machine is found again. Needs `discovery_enabled`.
- Connects with exponential backoff (`connect_with_backoff`), authenticates, reports its real display geometry (`DisplayGeometry` message) so the server can replace its assumed geometry.
- `session_loop` is one `tokio::select!` over: reliable TCP+TLS messages (clicks, key events, clipboard), the UDP `MouseMove` stream, and a channel carrying already-detected clipboard changes. Clipboard *reading* deliberately happens in a separate task on a blocking thread — it is a synchronous call into the X server/compositor that can stall for hundreds of milliseconds, and doing it inline froze input injection.
- **Injection is decoupled from the network loop** (`injection_task`): the session loop never awaits an injection — it publishes motion into a `tokio::sync::watch` (only the freshest absolute position survives; dedup is by value against the last injected position, not the watch's "seen" flag, which turns `Err` on sender close) and discrete events (keys/clicks) into a bounded ordered `mpsc`. The task's `select!` is `biased` toward the discrete queue so a key press never waits behind a burst of motion, but injects the freshest not-yet-applied position *before* each discrete event so clicks land where the cursor is now. This exists because each Wayland-portal injection is a D-Bus round trip; awaiting it inline made the keyboard queue behind stale motion.
- On each UDP wakeup the socket is still **drained to the freshest datagram** (`accept_mouse_move` + `try_recv_from` loop) before publishing to the watch — the watch would coalesce anyway, but draining avoids waking the loop once per stale datagram.
- `create_injector` tries both Linux backends (preferred one first) rather than committing to one: `XDG_SESSION_TYPE=wayland` does not guarantee the `RemoteDesktop` portal is usable, and a Wayland session usually still has an XWayland that XTEST can reach. On GNOME (`XDG_CURRENT_DESKTOP` contains `GNOME`) X11/XTEST is preferred even in a Wayland session, because mutter relays XWayland's XTEST system-wide and a local XTEST call beats the portal's per-event D-Bus round trip; other compositors don't guarantee that relay, so they keep preferring the portal. `IONCONNECT_INJECT_BACKEND=x11|wayland` forces one without recompiling.
- Injection goes through `ClientInjector`, **not** the sync `InputInjector` trait directly. The Wayland portal backend is held in its own variant so its `inject_async` can be awaited: its sync `InputInjector` impl has to `block_in_place` to await the D-Bus call, which costs a thread handoff per event. Don't collapse this back into `Box<dyn InputInjector>`.
- `HeldInput` lives inside `injection_task` (it tracks what was actually injected): pressed-but-not-yet-released keys/buttons are force-released when the task shuts down — after draining any queued releases so nothing is released twice — to avoid stuck modifiers/buttons at the OS level.

### XI2 raw events must be selected on the master device

`X11Capture` selects raw events with `deviceid = 1` (`XIAllMasterDevices`), not `0` (`XIAllDevices`). With `0` the server delivers **every raw event twice** — once for the slave that generated it, once for its master (measured: 2 raw presses vs 1). That is not a cosmetic duplicate: each `XI_RawMotion` delta was accumulated twice, so the remote cursor moved at double the physical speed. `capture_reports_a_key_press_and_release` asserts the exact event sequence and fails if this regresses.

Cooked (`KEY_PRESS`/`BUTTON_PRESS`) selection is not an alternative — it follows normal focus-based window propagation, so a focused app consumes the events before they reach our root-window selection. Raw events are delivered regardless of focus or grabs, which is also why capture keeps working after the pointer is grabbed and confined at the edge.

### Key repeat is synthesized, not captured

Neither capture backend delivers OS auto-repeat: X11's `XI_RawKeyPress` reports a physical press exactly once (measured against a real X server: 1 raw event vs 22 cooked ones over 1.5s while holding a key), and the Wayland portal's EIS stream behaves the same. So `core/src/key_repeat.rs` synthesizes repeats — pure logic, driven by the capture loop's idle timeout — using the delay/interval/per-key bitmask read from the local X server (`X11Control::key_repeat_settings`, which excludes modifiers). On the receiving end, `X11Injector`/`WaylandPortalInjector` turn a press of an already-held key into release+press: a key held via XTEST makes the *receiving* server start its own auto-repeat, so plain repeated presses would double the rate; release+press delivers one repeat and resets that timer.

### Why MouseMove has its own UDP transport

Continuous mouse deltas are latency-sensitive and loss-tolerant, so they bypass the reliable TCP+TLS `Connection` and go over a session-scoped encrypted UDP channel instead (`network/src/udp_codec.rs` + `core/src/udp_peers.rs`). The UDP key is derived via TLS `export_keying_material` right after the TLS handshake — no extra round trip, and it inherits the mutual TOFU authentication of the TLS session. Sequence numbers (`seq`) double as the AEAD nonce source and as a freshness check (`network::is_newer`) so old/replayed/reordered datagrams are dropped rather than applied. `seq` resets to 0 each session, which is why the UDP socket is rebound and the key rederived on every reconnect.

### Pairing: the default config cannot pair

TLS here is mutual, so **both** ends must trust each other's certificate fingerprint. `Settings::default()` ships `pairing_mode = RejectUnknown` and the trust store starts empty, so two fresh installs can never pair — the client loops forever on `certificado desconocido rechazado (posible MITM)` while the server waits. There is no pairing window in the GUI yet; the only ways out are flipping `pairing_mode` to `auto-trust-on-first-use` on the side that needs to learn, or appending the peer's fingerprint by hand.

The store is `~/.config/ionconnect/trusted_fingerprints`: one 64-char lowercase hex SHA-256 of the peer's cert DER per line. `parse_hex_fingerprint` drops any line whose length isn't exactly 64, and the file is written **without a trailing newline** — appending with a plain `echo` silently corrupts the last entry into a 128-char line that is then ignored. A machine's own fingerprint is `openssl x509 -in ~/.config/ionconnect/identity.crt -outform DER | sha256sum`.

Note the server also rejects, *after* the TLS handshake, any peer whose `DeviceId` is not in its configured `peers` list — so a trusted fingerprint alone is not enough to be routed input.

### Screen geometry assumption

Peer screen geometry is assumed equal to the local machine's until the peer reports its real resolution via `DisplayGeometry` (there's no protocol-level geometry exchange at connect time yet — see `README.md` roadmap). `HandoffState::clamp_to_active_desktop` prevents cursor coordinates from drifting unbounded past a peer's real bounds on axes that have no linked neighbor.

### Platform-conditional compilation

Non-macOS Unix (`input_session`, X11/Wayland capture+inject) is gated behind `#[cfg(all(unix, not(target_os = "macos")))]`; Windows-only paths behind `#[cfg(windows)]`. When touching `core/src/client.rs` or `server.rs`, check both branches compile — there's no macOS backend at all currently.

### `gui`: lifecycle is tied to the window, not a background service

`gui/src-tauri/src/main.rs` spawns `ionconnect-core` as a child process (`commands::start_core`/`state::AppState`) — there is **no tray icon and no background daemon**. `WindowEvent::CloseRequested` is vetoed and forwarded to the frontend as a JS confirmation instead of closing immediately; only after the user confirms does `confirm_close` let the app exit. On `RunEvent::Exit`, the GUI sends the `core` child a graceful kill (SIGTERM equivalent) so it has time to broadcast `Disconnect` to peers and release input — killing `core` abruptly leaves the peer stuck retrying or with the mouse grabbed. Because of this, a leftover systemd user service (`ionconnect-core.service`) fights the GUI for the same listen port (`Address already in use`); `install.sh` disables it automatically, but older installs need `systemctl --user disable --now ionconnect-core.service` by hand (see `README.md`).

Transfer feedback is a separate floating window (`gui/src-tauri/src/toast.rs` + `gui/dist/toast.{html,css,js}`): transparent, always-on-top, **non-focusable** (so showing it never steals the keyboard) and click-through, placed top-right of the monitor under the cursor. It is fed by structured lines `core` prints to stdout — `@@ionconnect-transfer {json}` from `core/src/transfer_events.rs` (`TransferToast`: start/progress/done/error; dropping it unclosed emits an error) — only when the GUI sets `IONCONNECT_GUI_EVENTS=1`. These are deliberately not parsed from `tracing` output, which `RUST_LOG` can silence.

Drag-and-drop is also handled at the Tauri window level (`WindowEvent::DragDrop`), not in the webview, and forwarded to the frontend as dropped file paths.

### File transfer

Files are sent over the same reliable TCP+TLS channel as clicks/keys (not UDP): `FileOffer` → `FileChunk`* → `FileEnd` (or `FileAbort` on a read failure), implemented in `core/src/file_transfer.rs`. There's no separate flow-control — the bounded `mpsc` that messages go out through already backpressures the sender, and TCP guarantees order/delivery. Chunks (`CHUNK_SIZE`, 128 KiB) are written to `<name>.<id>.part` and renamed to their final name (without clobbering an existing file) once `FileEnd` arrives.

Several files/folders moved in one user action travel as a **batch**: every `FileOffer` carries a `FileBatch { id, kind }` and a **relative** path with `/` separators (sanitized component by component on receipt — `sanitize_relative` — so it can never escape the destination), folders are announced with `mime = MIME_DIRECTORY` (no chunks), and `FileBatchEnd` closes it. Symlinks are skipped when walking. The batch kind picks the destination:

- `FileBatchKind::Downloads` → `default_transfer_dir()`, an `ionconnect` folder on the receiving user's **Desktop** (via `dirs::desktop_dir()`, falls back to `~/Desktop`, then cwd). Used for files dragged across the screen edge and for folders sent from the GUI. A single file sent from the GUI still travels without a batch. Inside it, top-level files are sorted into a subfolder by extension (`category_subdir`: `Imágenes`, `Videos`, `Audios`, `Documentos`, `Comprimidos`, `Instaladores`, `Otros`) and top-level folders go to `Carpetas` (`FOLDERS_SUBDIR`) — only for this destination, never for clipboard batches.
- `FileBatchKind::Clipboard` → a per-batch subfolder of `clipboard_cache_dir()` (`<cache>/ionconnect/clipboard`), then published on the receiver's clipboard as a file list so Ctrl+V pastes it anywhere. The cache keeps the last two batches and is wiped on startup.

Three ways to start a transfer: dropping files on the GUI window (IPC → `serve_ipc_file_sends`, no size cap), copy/paste, and mouse drag (both capped by `AUTO_TRANSFER_MAX_BYTES`, 4 GiB).

### Copy/paste of files (Ctrl+C → Ctrl+V)

Copying never moves bytes; **crossing the screen edge does**. `ClipboardWatcher::poll_all` reports a copied file list (`FilesChange::Copied`), and reads text only when the clipboard holds no files (file managers also offer the paths as text; syncing that would clobber the other machine's clipboard with paths that don't exist there). A list already on the clipboard at startup is not announced.

The server orchestrates both directions because it is the only side that sees edge crossings — `core/src/clipboard_files.rs`:

- `input_session` emits `ControlEvent::{Entered, Returned, Dragged}` over a channel (it runs on a blocking thread and must not do network/disk I/O); `run_control_events` consumes them.
- Server → client: on `Entered(device)`, send what was copied locally if that peer doesn't have that generation yet.
- Client → server: the client only announces (`ClipboardFilesOffer { set_id }`, `set_id = 0` withdraws); on `Returned(device)` the server asks with `ClipboardFilesRequest`. If the offer arrives while control is *not* on that client, it is requested right away (no crossing will trigger it).
- Files received from a client become available for the next peer the control enters (client → client relay), never sent back to their origin.

`ClipboardFiles` is pure state (no I/O) precisely so those rules are unit-tested in isolation.

Publishing a file list on Linux does **not** go through `arboard`: it only offers `text/uri-list`, and Nautilus only enables Paste when it sees `x-special/gnome-copied-files`. `clipboard/src/x11_files.rs` takes ownership of `CLIPBOARD` with `x11rb` and serves both formats plus plain text (used on X11 sessions and on GNOME/Wayland via XWayland; other Wayland compositors keep `arboard`'s native data-control path). Reading does use `arboard`, but its `text/uri-list` parser splits on `\n` only, so with standard CRLF lists every path came back with a trailing `\r` — `normalize_path` strips it. Both were found by the real-X tests in `clipboard/tests/x11_files.rs`.

### Dragging files across the edge

A drag that reaches the edge is turned into a transfer to the other machine's `ionconnect` Desktop folder; the local drag is cancelled so nothing is dropped locally.

- Server → client: in `apply_handoff_action`, if the left button is physically held, `x11::take_dragged_files()` runs **before** our pointer grab (while a drag is alive the source app holds the pointer grab and ours fails — which is why crossing mid-drag used to just bounce back).
- Client → server: the server knows the left button is held on the client (`ForwardedHeld`), so on `ReturnLocal` it sends `Message::DragQuery` **before** the forwarded button release. On the client that goes through the ordered `Discrete` queue of `injection_task` so it is resolved before the release is injected (the release would otherwise complete the drop).

`input/src/x11/drag.rs` does the full XDND handshake as a drop target (tiny `XdndAware` window under the pointer, 1px nudge, wait for `XdndEnter`/`XdndPosition`, convert `XdndSelection` to `text/uri-list`). Just reading `XdndSelection` is wrong: apps keep owning it after a drag ends, so a stale owner would hand back an hour-old file list as soon as the user crossed the edge while selecting text — only a source that talks to us proves a drag is live. Cancelling is toolkit-specific: Escape works for GTK 3/Qt/Chromium, but **GTK 4 ignores the keyboard during an X11 drag** (its `gdk_x11_drag_handle_event` only handles motion, button release and grab-broken), so the fallback breaks its pointer grab by unmapping the 1×1 helper window it uses as XDND source — only if the source window really is a 1×1 override-redirect helper, and it is remapped right after. Files are returned only if the drag actually ended. The injected Escape comes back through raw capture, so the server swallows it (`SessionState::swallow_key`).

`input/src/win32/drag.rs` is the same idea with OLE (a nearly transparent drop-target window + `IDropTarget::DragEnter` + `CF_HDROP` + Escape). It cross-compiles (`cargo check -p ionconnect-input --target x86_64-pc-windows-gnu`) but has never run on Windows. Drags between native Wayland apps never touch the X server and are invisible to this.

`input/tests/x11_drag.rs` runs against real GTK 3 and GTK 4 apps (python3-gi) in Xephyr — keep that when touching the handshake; a hand-written fake XDND source would only test our own reading of the spec.

### Installers and packaging

`.github/workflows/release.yml` builds native installers per platform via `tauri-action` (Windows `.exe`/`.msi`, Linux `.deb`/`.AppImage`) plus a standalone `ionconnect-core` binary, triggered by pushing a `vX.Y.Z` tag — end users need no Rust/build toolchain. `install.sh` (Ubuntu/Debian) and `install.ps1` (Windows) are the from-source path: they install Rust if missing, clone, `cargo build --release`, and put both binaries on `PATH`. `installer/linux/ionconnect-core.service` is a legacy systemd unit kept for reference/cleanup only — see the lifecycle note above for why it must stay disabled.
