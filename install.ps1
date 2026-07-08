#Requires -Version 5.1
<#
Instalador de IonConnect para Windows 11.

Uso (PowerShell):
  irm https://raw.githubusercontent.com/millerbermeo/ion/main/install.ps1 | iex

Compila desde el código fuente (todavía no hay binarios pre-compilados
publicados) e instala el ejecutable de la GUI en %LOCALAPPDATA%\IonConnect\bin.
#>

$ErrorActionPreference = "Stop"

$RepoUrl = "https://github.com/millerbermeo/ion.git"
$InstallDir = "$env:LOCALAPPDATA\IonConnect\src"
$BinDir = "$env:LOCALAPPDATA\IonConnect\bin"

function Write-Step($msg) {
    Write-Host "==> $msg" -ForegroundColor Cyan
}

function Test-Command($name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

if (-not (Test-Command "git")) {
    throw "Git no está instalado. Instalalo desde https://git-scm.com/download/win y volvé a correr este script."
}

if (-not (Test-Command "cargo")) {
    Write-Step "Instalando Rust (rustup-init)..."
    $rustupExe = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupExe
    & $rustupExe -y
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
} else {
    Write-Step "Rust ya está instalado ($(cargo --version))."
}

if (Test-Path "$InstallDir\.git") {
    Write-Step "Actualizando código fuente existente en $InstallDir..."
    git -C $InstallDir pull --ff-only
} else {
    Write-Step "Clonando $RepoUrl en $InstallDir..."
    New-Item -ItemType Directory -Force -Path (Split-Path $InstallDir) | Out-Null
    git clone --depth 1 $RepoUrl $InstallDir
}

Write-Step "Compilando IonConnect (release, puede tardar varios minutos)..."
Push-Location $InstallDir
try {
    cargo build --release -p ionconnect-gui
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Copy-Item "$InstallDir\target\release\ionconnect-gui.exe" "$BinDir\ionconnect-gui.exe" -Force

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    Write-Step "Se agregó $BinDir al PATH del usuario. Abrí una terminal nueva para que tome efecto."
}

Write-Step "Listo. Corré 'ionconnect-gui' para abrir la aplicación."
