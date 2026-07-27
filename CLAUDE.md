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
# nested display, never against your own session (they inject keys/clicks)
Xephyr :80 -screen 800x600 -ac &
DISPLAY=:80 cargo test -p ionconnect-input --test x11_smoke -- --ignored --test-threads=1
```

Linux build dependencies (for the GUI/Tauri crate and packaging): `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `libdbus-1-dev`, `build-essential`.

Some tests exercise real resources rather than mocks: real TLS handshakes over loopback (`crypto/tests/loopback.rs`), real X11 via Xephyr (`input/tests/x11_smoke.rs`), a real server+client pair running concurrently (`core/src/server.rs` tests). Keep that pattern — this codebase prefers integration-style tests over mocking its own dependencies where a real resource is available in CI.

## Architecture

Cargo workspace, one crate per responsibility (`Cargo.toml` lists members). Dependency direction flows roughly top-to-bottom:

| Crate | Responsibility |
|---|---|
| `shared` | Common types (`DeviceId`, `KeyModifiers`) used everywhere |
| `protocol` | Binary wire protocol — `Message` enum, encode/decode (`protocol/src/message.rs`, `codec.rs`) |
| `crypto` | Mutual TLS 1.3 + TOFU (trust-on-first-use) trust by certificate fingerprint |
| `network` | Tokio transport: TCP+TLS framing/reconnect/backoff/mDNS discovery, plus a **separate encrypted UDP channel** for high-frequency `MouseMove` deltas (`network/src/udp_codec.rs`) |
| `input` | Per-OS capture/injection backends: `input/src/x11`, `input/src/win32`, `input/src/wayland` |
| `screen` | Multi-monitor geometry (`MonitorGeometry`, `VirtualDesktop`) and screen-edge layout (`Layout`, `ScreenEdge`) — pure logic, no I/O |
| `clipboard` | Clipboard sync with loop-guard to avoid re-broadcasting a change that was just applied remotely |
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
