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
4. Tocar **Conectar** en la GUI de cada equipo (servidor primero). Mover el mouse hacia el borde configurado pasa el control al equipo vecino.

IonConnect corre **solo mientras la ventana de la GUI está abierta**: no hay ícono de bandeja ni servicio en segundo plano. Al cerrar la ventana (con confirmación) se apaga `ionconnect-core` y se avisa al otro equipo. Si tenés una instalación vieja con el servicio systemd `ionconnect-core.service` habilitado, desactivalo — la GUI y el servicio pelean por el mismo puerto (`Address already in use`):

```bash
systemctl --user disable --now ionconnect-core.service
rm -f ~/.config/systemd/user/ionconnect-core.service && systemctl --user daemon-reload
```

(`install.sh` ya hace esto solo desde esta versión.)

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
| `clipboard` | Sincronización de portapapeles (texto y archivos copiados) con prevención de bucles |
| `config` | Configuración TOML con recarga en caliente (incluye rol y peers) |
| `ipc` | Canal local GUI↔core autenticado por token |
| `core` | Orquestador: servidor (captura+enruta) y cliente (recibe+inyecta) |
| `gui` | Aplicación Tauri (panel de control) |

Cada crate tiene su propia suite de tests (unitarios + integración contra recursos reales cuando es posible: TLS real sobre loopback, X11 real vía Xephyr, inotify real, servidor+cliente reales corriendo en paralelo, etc.).

## Roadmap

- [x] Protocolo binario + criptografía TLS/TOFU
- [x] Transporte de red (tokio) + discovery mDNS
- [x] Cliente encuentra al servidor por mDNS (sin `server_address`; se re-busca en cada reconexión). mDNS no cruza subredes/VLAN: ahí usar `server_address` manual (acepta `host:puerto`)
- [x] Captura/inyección de entrada (X11 completo; Windows y portal Wayland como cliente)
- [x] Geometría multi-monitor y hand-off de cursor
- [x] Sincronización de portapapeles
- [x] Configuración persistente + hot-reload, con rol y peers
- [x] IPC local GUI↔core
- [x] GUI (Tauri): rol, peers con lado de pantalla, ID propio copiable
- [x] Transferencia de archivos (arrastrar a la ventana de la GUI → se guardan en la carpeta `ionconnect` del Escritorio del otro equipo)
- [x] Copiar y pegar archivos entre equipos: Ctrl+C en el explorador de uno, cruzar el cursor, Ctrl+V en el explorador del otro (archivos y carpetas, en ambos sentidos)
- [x] Arrastrar archivos de un equipo al otro con el mouse: se arrastra hasta el borde de la pantalla y aparecen en la carpeta `ionconnect` del Escritorio del otro equipo
- [x] Ciclo de vida atado a la ventana: sin bandeja, cerrar la ventana apaga `core` y avisa al otro extremo con `Disconnect`
- [x] Binario `core`: orquesta captura→red→inyección extremo a extremo (servidor X11)
- [x] CI que publica instaladores nativos por plataforma (GitHub Actions + `tauri-action`), sin pedirle a nadie que compile
- [ ] Servidor (captura) en Windows/Wayland — hoy solo cliente en esas plataformas
- [ ] Intercambio real de geometría de pantalla entre equipos (hoy se asume la misma resolución)
- [ ] Backend de captura Wayland nativo (wlroots / `ext-input-capture-v1`)
- [ ] Portapapeles de imágenes
- [ ] Soltar el archivo arrastrado en una carpeta concreta del otro equipo (hoy siempre llega a `ionconnect` del Escritorio)
- [ ] Soporte macOS

## Pasar archivos entre equipos

Hay tres formas, y ninguna de las dos primeras necesita abrir la ventana de IonConnect:

| Cómo | Qué hacés | Dónde quedan |
|---|---|---|
| **Copiar y pegar** | Ctrl+C sobre archivos o carpetas en el explorador, llevás el cursor al otro equipo y Ctrl+V donde quieras | Donde pegues |
| **Arrastrar** | Arrastrás los archivos hasta el borde de la pantalla que da al otro equipo y seguís de largo | Carpeta `ionconnect` del Escritorio del otro equipo |
| **Desde la app** | Los soltás sobre la ventana de IonConnect (o "Elegir archivos…") | Carpeta `ionconnect` del Escritorio del otro equipo |

Detalles que conviene saber:

- Al copiar no se transfiere nada: los archivos viajan cuando el cursor cruza al otro equipo. Con archivos grandes, el Ctrl+V puede tardar un momento en estar disponible.
- Copiar y arrastrar tienen un tope de 4 GB por vez (para que un Ctrl+C distraído sobre una carpeta enorme no llene el otro disco). El envío desde la app no tiene tope.
- Los enlaces simbólicos dentro de una carpeta no se siguen.
- Cortar (Ctrl+X) se trata como copiar: el original no se borra.

## Limitaciones conocidas

- **Arrastrar archivos entre equipos necesita que la aplicación de origen sea X11** (incluye las que corren sobre XWayland) en Linux, u OLE estándar en Windows. Un arrastre desde una aplicación Wayland nativa —por ejemplo Nautilus en una sesión GNOME sobre Wayland— no es visible para IonConnect: en ese caso usá copiar y pegar. Verificado contra GTK 3 y GTK 4 reales; la parte de Windows compila pero no se ejercitó en una máquina Windows.
- **Copiar y pegar archivos de un cliente a otro cliente** pasa por el servidor: funciona si el cursor pasa por la pantalla del servidor en el camino.

- **El servidor (el que capta el mouse/teclado) debe ser Ubuntu X11.** Windows y Wayland ya funcionan como cliente (reciben e inyectan), pero la captura con detección de borde solo está implementada para X11.
- **La geometría de cada peer se asume igual a la del servidor** (no hay todavía un mensaje de protocolo que intercambie resoluciones reales). El hand-off funciona correctamente como máquina de estados; el punto exacto de reingreso puede no ser perfecto si las resoluciones difieren mucho.
- El backend de inyección de Windows y el de Wayland (portal `RemoteDesktop`) están implementados pero no se pudieron ejercitar en esta sesión de desarrollo (hecha en Ubuntu X11) — el código sigue la documentación de sus respectivas APIs, pero no tienen la misma cobertura de pruebas reales que el camino X11.

## Licencia

MIT
