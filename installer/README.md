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

`ionconnect-core` (ver `core/`, orquesta captura→red→inyección) **no** se
corre como servicio: IonConnect solo funciona con la ventana de la GUI
abierta, que lo arranca y lo apaga. `installer/linux/ionconnect-core.service`
queda solo como referencia para quien quiera un modo headless propio; el
instalador ya no lo instala (y desactiva el de instalaciones viejas, porque
GUI + servicio pelean por el mismo puerto).

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
- (El `.desktop` file ya lo instala `install.sh`.)
