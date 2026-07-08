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
  libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev libdbus-1-dev \
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

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) log "Agregá $BIN_DIR a tu PATH (por ejemplo en ~/.bashrc): export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

log "Listo. Corré 'ionconnect-gui' para abrir la aplicación."
