# IonConnect

Alternativa moderna a [Barrier](https://github.com/debauchee/barrier)/[Input Leap](https://github.com/input-leap/input-leap) escrita en Rust: compartir mouse, teclado y portapapeles entre Windows 11 y Ubuntu (X11 y Wayland) en la misma LAN.

> **Estado: en desarrollo activo.** Los módulos de bajo nivel (protocolo, criptografía, red, entrada, pantalla, portapapeles, configuración, IPC) están implementados y probados. Todavía falta el binario `core` que los orquesta en un servicio funcional — hoy la GUI compila y administra configuración local, pero el compartir mouse/teclado extremo a extremo aún no está conectado. Ver [Roadmap](#roadmap).

## Instalación rápida

**Ubuntu / Debian:**

```bash
curl -fsSL https://raw.githubusercontent.com/millerbermeo/ion/main/install.sh | bash
```

**Windows 11 (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/millerbermeo/ion/main/install.ps1 | iex
```

Ambos scripts instalan Rust (si falta), clonan el repositorio, compilan en modo release y dejan el ejecutable `ionconnect-gui` en el `PATH` del usuario. Compilan desde el código fuente porque todavía no se publican binarios pre-compilados.

## Compilar manualmente

Requisitos: [Rust estable](https://rustup.rs/), y en Linux: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `build-essential`.

```bash
git clone https://github.com/millerbermeo/ion.git
cd ion
cargo build --release -p ionconnect-gui
cargo test --workspace --exclude ionconnect-gui   # tests de los crates sin GUI
```

## Arquitectura

Workspace de Cargo, un crate por responsabilidad (arquitectura limpia/hexagonal):

| Crate | Responsabilidad |
|---|---|
| `shared` | Tipos comunes (`DeviceId`, `KeyModifiers`) |
| `protocol` | Protocolo binario de wire (mensajes, encode/decode) |
| `crypto` | TLS 1.3 mutuo + confianza TOFU por fingerprint |
| `network` | Transporte tokio: framing, heartbeat, reconexión, discovery mDNS |
| `input` | Captura/inyección de mouse+teclado (X11, Windows, portal Wayland) |
| `screen` | Geometría multi-monitor y hand-off de cursor entre equipos |
| `clipboard` | Sincronización de portapapeles con prevención de bucles |
| `config` | Configuración TOML con recarga en caliente |
| `ipc` | Canal local GUI↔core autenticado por token |
| `gui` | Aplicación Tauri (panel de control) |

Cada crate tiene su propia suite de tests (unitarios + integración contra recursos reales cuando es posible: TLS real sobre loopback, X11 real vía Xephyr, inotify real, etc.).

## Roadmap

- [x] Protocolo binario + criptografía TLS/TOFU
- [x] Transporte de red (tokio) + discovery mDNS
- [x] Captura/inyección de entrada (X11 completo; Windows y portal Wayland sin poder probarse en esta máquina de desarrollo)
- [x] Geometría multi-monitor y hand-off de cursor
- [x] Sincronización de portapapeles
- [x] Configuración persistente + hot-reload
- [x] IPC local GUI↔core
- [x] Scaffold de GUI (Tauri)
- [ ] Binario `core`: orquesta captura→red→inyección extremo a extremo
- [ ] Backend de captura Wayland nativo (wlroots / `ext-input-capture-v1`)
- [ ] Transferencia de archivos, portapapeles de imágenes
- [ ] Soporte macOS

## Licencia

MIT
