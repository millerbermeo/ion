# IonConnect

Alternativa moderna a [Barrier](https://github.com/debauchee/barrier)/[Input Leap](https://github.com/input-leap/input-leap) escrita en Rust: compartir mouse, teclado y portapapeles entre Windows 11 y Ubuntu (X11 y Wayland) en la misma LAN.

> **Estado: funcional de punta a punta en Ubuntu X11 (servidor) ↔ cualquier plataforma soportada (cliente).** `core` ya orquesta captura→red→inyección real: probado con dos instancias reales (TLS mutuo, TOFU, autenticación, hand-off de mouse) sobre loopback. El servidor (el equipo con el mouse/teclado físico) requiere X11 por ahora; cualquier plataforma soportada por `input` (Windows, X11, o Wayland vía portal) puede ser cliente. Ver [Roadmap](#roadmap) y [Limitaciones conocidas](#limitaciones-conocidas).

## Instalación

### Opción 1: descargar el instalador (recomendado, sin compilar nada)

Ir a **[Releases](https://github.com/millerbermeo/ion/releases)** y descargar:

- **Windows**: `IonConnect_x.x.x_x64-setup.exe` o `.msi` — instalar y listo.
- **Ubuntu/Debian**: `.deb` (`sudo apt install ./ionconnect_x.x.x_amd64.deb`) o `.AppImage` (marcarlo ejecutable y correrlo).
- En ambos casos, `ionconnect-core` se descarga de la misma release y va en el `PATH` (en Linux, `~/.local/bin` alcanza).

Estos instaladores los compila automáticamente GitHub Actions (`.github/workflows/release.yml`) en máquinas que ya tienen todo lo necesario — el usuario final no necesita Rust, Visual Studio ni ninguna dependencia de compilación.

> Si el repositorio todavía no tiene ninguna release publicada, alguien con permisos de push tiene que crear un tag (`git tag v0.1.0 && git push origin v0.1.0`) para disparar el primer build.

### Opción 2: compilar desde el código fuente

Para quien prefiera compilar localmente o esté en una plataforma sin instalador pre-compilado todavía:

**Ubuntu / Debian:**

```bash
curl -fsSL https://raw.githubusercontent.com/millerbermeo/ion/main/install.sh | bash
```

**Windows 11 (PowerShell):**

```powershell
irm https://raw.githubusercontent.com/millerbermeo/ion/main/install.ps1 | iex
```

Ambos scripts instalan Rust (si falta) — en Windows también hace falta tener
instalado Visual C++ Build Tools, el script lo detecta y avisa si falta —,
clonan el repositorio, compilan en modo release y dejan `ionconnect-gui` e
`ionconnect-core` en el `PATH` del usuario.

## Uso rápido

1. En **cada** equipo: correr `ionconnect-gui`, copiar el "ID de este equipo" que muestra.
2. En el equipo con el mouse/teclado físico (el **servidor**): dejar el rol en "Servidor", agregar cada otro equipo como peer pegando su ID y eligiendo de qué lado de la pantalla está (izquierda/derecha/arriba/abajo).
3. En los demás equipos (**clientes**): cambiar el rol a "Cliente" y poner la dirección `ip:puerto` del servidor.
4. Correr `ionconnect-core` en los equipos (servidor primero). Mover el mouse hacia el borde configurado pasa el control al equipo vecino.

### Correr en segundo plano (Linux)

`install.sh` deja instalado (pero sin arrancar) un servicio systemd de usuario, así `ionconnect-core` sigue corriendo aunque cierres la GUI:

```bash
systemctl --user start ionconnect-core.service    # arrancarlo ahora
systemctl --user status ionconnect-core.service   # ver que esté corriendo
journalctl --user -u ionconnect-core.service -f   # logs en vivo
```

Ya quedó habilitado para el próximo login (`enable`). Si usás el servicio, no uses el botón "Conectar" de la GUI en esa máquina — ambos compitiendo por el mismo puerto fallan.

## Compilar manualmente

Requisitos: [Rust estable](https://rustup.rs/), y en Linux: `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `libdbus-1-dev`, `build-essential`.

```bash
git clone https://github.com/millerbermeo/ion.git
cd ion
cargo build --release -p ionconnect-gui -p ionconnect-core
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
| `config` | Configuración TOML con recarga en caliente (incluye rol y peers) |
| `ipc` | Canal local GUI↔core autenticado por token |
| `core` | Orquestador: servidor (captura+enruta) y cliente (recibe+inyecta) |
| `gui` | Aplicación Tauri (panel de control) |

Cada crate tiene su propia suite de tests (unitarios + integración contra recursos reales cuando es posible: TLS real sobre loopback, X11 real vía Xephyr, inotify real, servidor+cliente reales corriendo en paralelo, etc.).

## Roadmap

- [x] Protocolo binario + criptografía TLS/TOFU
- [x] Transporte de red (tokio) + discovery mDNS
- [x] Captura/inyección de entrada (X11 completo; Windows y portal Wayland como cliente)
- [x] Geometría multi-monitor y hand-off de cursor
- [x] Sincronización de portapapeles
- [x] Configuración persistente + hot-reload, con rol y peers
- [x] IPC local GUI↔core
- [x] GUI (Tauri): rol, peers con lado de pantalla, ID propio copiable
- [x] Binario `core`: orquesta captura→red→inyección extremo a extremo (servidor X11)
- [x] CI que publica instaladores nativos por plataforma (GitHub Actions + `tauri-action`), sin pedirle a nadie que compile
- [ ] Servidor (captura) en Windows/Wayland — hoy solo cliente en esas plataformas
- [ ] Intercambio real de geometría de pantalla entre equipos (hoy se asume la misma resolución)
- [ ] Backend de captura Wayland nativo (wlroots / `ext-input-capture-v1`)
- [ ] Transferencia de archivos, portapapeles de imágenes
- [ ] Soporte macOS

## Limitaciones conocidas

- **El servidor (el que capta el mouse/teclado) debe ser Ubuntu X11.** Windows y Wayland ya funcionan como cliente (reciben e inyectan), pero la captura con detección de borde solo está implementada para X11.
- **La geometría de cada peer se asume igual a la del servidor** (no hay todavía un mensaje de protocolo que intercambie resoluciones reales). El hand-off funciona correctamente como máquina de estados; el punto exacto de reingreso puede no ser perfecto si las resoluciones difieren mucho.
- El backend de inyección de Windows y el de Wayland (portal `RemoteDesktop`) están implementados pero no se pudieron ejercitar en esta sesión de desarrollo (hecha en Ubuntu X11) — el código sigue la documentación de sus respectivas APIs, pero no tienen la misma cobertura de pruebas reales que el camino X11.

## Licencia

MIT
