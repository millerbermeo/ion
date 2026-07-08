const invoke = () => window.__TAURI__.core.invoke;

const THEME_STORAGE_KEY = "ionconnect-theme";

function applyStoredTheme() {
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  if (stored === "light" || stored === "dark") {
    document.documentElement.setAttribute("data-theme", stored);
  }
}

function toggleTheme() {
  const current = document.documentElement.getAttribute("data-theme");
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  const currentlyDark = current === "dark" || (!current && prefersDark);
  const next = currentlyDark ? "light" : "dark";
  document.documentElement.setAttribute("data-theme", next);
  localStorage.setItem(THEME_STORAGE_KEY, next);
}

async function loadSettings() {
  const settings = await invoke()("get_settings");
  document.getElementById("device_name").value = settings.device_name;
  document.getElementById("listen_port").value = settings.listen_port;
  document.getElementById("discovery_enabled").checked = settings.discovery_enabled;
  document.getElementById("pairing_mode").value = settings.pairing_mode;
  document.getElementById("log_level").value = settings.log_level;
}

async function saveSettings(event) {
  event.preventDefault();
  const form = event.target;
  const settings = {
    device_name: form.device_name.value,
    listen_port: Number(form.listen_port.value),
    discovery_enabled: form.discovery_enabled.checked,
    pairing_mode: form.pairing_mode.value,
    log_level: form.log_level.value,
  };
  const status = document.getElementById("save-status");
  try {
    await invoke()("save_settings", { settings });
    status.textContent = "Guardado.";
  } catch (error) {
    status.textContent = `Error al guardar: ${error}`;
  }
  setTimeout(() => {
    status.textContent = "";
  }, 3000);
}

async function loadDevices() {
  const devices = await invoke()("list_devices");
  const list = document.getElementById("device-list");
  list.innerHTML = "";
  if (devices.length === 0) {
    const empty = document.createElement("li");
    empty.className = "device-list__empty";
    empty.textContent = "Sin equipos conectados todavía.";
    list.appendChild(empty);
    return;
  }
  for (const device of devices) {
    const item = document.createElement("li");
    item.textContent = `${device.name} — ${device.connected ? "conectado" : "desconectado"}`;
    list.appendChild(item);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  applyStoredTheme();
  document.getElementById("theme-toggle").addEventListener("click", toggleTheme);
  document.getElementById("settings-form").addEventListener("submit", saveSettings);
  loadSettings();
  loadDevices();
});
