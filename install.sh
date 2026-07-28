#!/usr/bin/env bash
# Instalador de IonConnect para Ubuntu/Debian.
#
# Uso:
#   curl -fsSL https://raw.githubusercontent.com/millerbermeo/ion/main/install.sh | bash
#
# Compila desde el código fuente (todavía no hay binarios pre-compilados
# publicados) e instala el binario de la GUI en ~/.local/bin.
set -euo pipefail

REPO_URL="https://github.com/millerbermeo/ion.git"
INSTALL_DIR="${IONCONNECT_SRC_DIR:-$HOME/.local/share/ionconnect/src}"
BIN_DIR="$HOME/.local/bin"

log() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

if [ "$(uname -s)" != "Linux" ]; then
  die "este script es para Linux (Ubuntu/Debian). Para Windows usá install.ps1."
fi

if ! command -v apt-get >/dev/null 2>&1; then
  die "no se encontró apt-get — este instalador asume Ubuntu/Debian."
fi

log "Instalando dependencias del sistema (pide sudo)..."
sudo apt-get update -y
sudo apt-get install -y \
  build-essential curl git pkg-config \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev \
  libssl-dev libx11-dev

if ! command -v cargo >/dev/null 2>&1; then
  log "Instalando Rust (rustup)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
else
  log "Rust ya está instalado ($(cargo --version))."
fi

if [ -d "$INSTALL_DIR/.git" ]; then
  log "Actualizando código fuente existente en $INSTALL_DIR..."
  git -C "$INSTALL_DIR" pull --ff-only
else
  log "Clonando $REPO_URL en $INSTALL_DIR..."
  mkdir -p "$(dirname "$INSTALL_DIR")"
  git clone --depth 1 "$REPO_URL" "$INSTALL_DIR"
fi

log "Compilando IonConnect (release, puede tardar varios minutos)..."
(cd "$INSTALL_DIR" && cargo build --release -p ionconnect-gui -p ionconnect-core)

mkdir -p "$BIN_DIR"
install -m 755 "$INSTALL_DIR/target/release/ionconnect-gui" "$BIN_DIR/ionconnect-gui"
install -m 755 "$INSTALL_DIR/target/release/ionconnect-core" "$BIN_DIR/ionconnect-core"

log "Creando acceso directo (menú de aplicaciones)..."
ICONS_SRC="$INSTALL_DIR/gui/src-tauri/icons"
ICON_ROOT="$HOME/.local/share/icons/hicolor"
DESKTOP_DIR="$HOME/.local/share/applications"
mkdir -p "$DESKTOP_DIR"
# Cada PNG va en el directorio de su tamaño real: si se instala uno de
# 512px dentro de `256x256/`, el tema de iconos lo escala y se ve borroso.
install_icon() {
  local src="$1" size="$2"
  [ -f "$src" ] || return 0
  mkdir -p "$ICON_ROOT/$size/apps"
  install -m 644 "$src" "$ICON_ROOT/$size/apps/ionconnect.png"
}
install_icon "$ICONS_SRC/32x32.png" 32x32
install_icon "$ICONS_SRC/128x128.png" 128x128
install_icon "$ICONS_SRC/128x128@2x.png" 256x256
install_icon "$ICONS_SRC/icon.png" 512x512

cat > "$DESKTOP_DIR/ionconnect.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=IonConnect
Comment=Compartí mouse y teclado entre equipos en la misma LAN
Exec=$BIN_DIR/ionconnect-gui
Icon=ionconnect
Terminal=false
Categories=Utility;Network;
# El escritorio asocia una ventana con su lanzador comparando el WM_CLASS
# de la ventana contra este campo (o, si falta, contra el nombre del
# archivo .desktop). La ventana de Tauri reporta "ionconnect-gui", que no
# coincide con "ionconnect", así que sin esta línea GNOME no encuentra el
# lanzador y muestra un icono genérico en el dock.
StartupWMClass=ionconnect-gui
EOF
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DESKTOP_DIR" || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -f -t "$ICON_ROOT" >/dev/null 2>&1 || true

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) log "Agregá $BIN_DIR a tu PATH (por ejemplo en ~/.bashrc): export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

if command -v systemctl >/dev/null 2>&1; then
  log "Instalando servicio systemd de usuario (corre ionconnect-core en segundo plano, sobrevive cerrar la GUI)..."
  SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
  mkdir -p "$SYSTEMD_USER_DIR"
  install -m 644 "$INSTALL_DIR/installer/linux/ionconnect-core.service" "$SYSTEMD_USER_DIR/ionconnect-core.service"
  systemctl --user daemon-reload
  systemctl --user enable ionconnect-core.service
  log "Servicio habilitado (arranca solo en el próximo login)."
  log "Para activarlo ya: systemctl --user start ionconnect-core.service"
  log "Ojo: si la GUI ya tiene 'ionconnect-core' corriendo (botón Conectar), cerrala antes de arrancar el servicio — los dos compitiendo por el mismo puerto fallan."
else
  log "systemctl no encontrado — omitiendo instalación del servicio de background. Instalación manual: ver installer/linux/ionconnect-core.service"
fi

log "Listo. Buscá 'IonConnect' en el menú de aplicaciones, o corré 'ionconnect-gui'."
