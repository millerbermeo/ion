# Empaquetado de IonConnect

## Linux (`.deb` / AppImage)

El bundler de Tauri (`bundle.targets` en `gui/src-tauri/tauri.conf.json`, ya
configurado a `["deb", "appimage"]`) genera ambos paquetes directamente a
partir del binario `ionconnect-gui`:

```
cd gui/src-tauri
cargo tauri build
```

Requiere en el sistema de build: `libwebkit2gtk-4.1-dev`,
`libappindicator3-dev`, `librsvg2-dev`, `libdbus-1-dev`.

`ionconnect-core.service` es la unit de systemd (de **usuario**, no de
sistema — necesita la sesión gráfica) para el binario `ionconnect-core`
(ya implementado — ver `core/`, orquesta captura→red→inyección). Instalar
la unit con:

```
mkdir -p ~/.config/systemd/user
cp installer/linux/ionconnect-core.service ~/.config/systemd/user/
systemctl --user enable --now ionconnect-core.service
```

## Windows (`.msi` / `.exe`)

Tauri genera `.msi` (WiX) y/o `.exe` (NSIS) nativamente sin herramientas
adicionales — basta con agregar `"msi"` y/o `"nsis"` a `bundle.targets` en
`tauri.conf.json` y correr `cargo tauri build` **en Windows** (no se puede
cross-compilar el instalador desde Linux). Sin una máquina Windows
disponible en esta sesión de desarrollo, este paso no se pudo ejercitar —
mismo límite que los backends `win32` de `input`.

## Qué falta para un instalador completo

- Firma de código para el instalador de Windows (fase de release, no de
  desarrollo).
- Un `.desktop` file y entrada de autostart para Linux, análogo a la unit
  de systemd, si se prefiere autostart de sesión en vez de servicio.
