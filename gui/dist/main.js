const invoke = () => window.__TAURI__.core.invoke;

const THEME_STORAGE_KEY = "ionconnect-theme";
const EDGE_OPTIONS = [
  ["left", "Izquierdo"],
  ["right", "Derecho"],
  ["top", "Arriba"],
  ["bottom", "Abajo"],
];

let peers = [];
let coreRunning = false;

const CORE_STATUS_LABELS = {
  starting: "Iniciando…",
  listening: "Escuchando conexiones…",
  connected: "Conectado",
  retrying: "Reintentando conexión…",
  error: "Error — mirá el log",
  stopped: "Detenido",
};

const CORE_STATUS_CLASSES = {
  starting: "status--connecting",
  listening: "status--online",
  connected: "status--online",
  retrying: "status--connecting",
  error: "status--error",
  stopped: "status--offline",
};

const CORE_RUNNING_STATUSES = new Set(["starting", "listening", "connected", "retrying", "error"]);

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

async function loadDeviceId() {
  const deviceId = await invoke()("get_device_id");
  document.getElementById("device-id").value = deviceId;
}

function copyDeviceId() {
  const field = document.getElementById("device-id");
  field.select();
  navigator.clipboard?.writeText(field.value);
}

function updateRoleVisibility() {
  const role = document.getElementById("role").value;
  document.getElementById("client-fields").hidden = role !== "client";
  document.getElementById("server-fields").hidden = role !== "server";
}

function updateCoreToggleLabel() {
  const role = document.getElementById("role").value;
  const btn = document.getElementById("core-toggle");
  btn.textContent = coreRunning ? "Detener" : role === "server" ? "Iniciar servidor" : "Conectar";
}

function setConnectionIndicator(status) {
  const el = document.getElementById("connection-indicator");
  const label = CORE_STATUS_LABELS[status] ?? "sin conexiones";
  const cls = CORE_STATUS_CLASSES[status] ?? "status--offline";
  el.className = `status ${cls}`;
  el.textContent = `● ${label}`;
}

function appendCoreLog(line) {
  const pre = document.getElementById("core-log-view");
  const lines = `${pre.textContent}${line}\n`.split("\n");
  pre.textContent = lines.slice(-200).join("\n");
  pre.scrollTop = pre.scrollHeight;
}

async function toggleCore() {
  const btn = document.getElementById("core-toggle");
  btn.disabled = true;
  try {
    if (coreRunning) {
      await invoke()("stop_core");
      coreRunning = false;
      setConnectionIndicator("stopped");
    } else {
      document.getElementById("core-log-view").textContent = "";
      await invoke()("start_core");
      coreRunning = true;
      setConnectionIndicator("starting");
    }
  } catch (error) {
    appendCoreLog(`[gui] ${error}`);
  } finally {
    btn.disabled = false;
    updateCoreToggleLabel();
  }
}

function renderPeers() {
  const body = document.getElementById("peers-body");
  body.innerHTML = "";
  peers.forEach((peer, index) => {
    const row = document.createElement("tr");

    const nameCell = document.createElement("td");
    const nameInput = document.createElement("input");
    nameInput.type = "text";
    nameInput.value = peer.name;
    nameInput.addEventListener("input", (e) => {
      peers[index].name = e.target.value;
    });
    nameCell.appendChild(nameInput);

    const idCell = document.createElement("td");
    const idInput = document.createElement("input");
    idInput.type = "text";
    idInput.value = peer.device_id;
    idInput.placeholder = "32 caracteres hexadecimales";
    idInput.addEventListener("input", (e) => {
      peers[index].device_id = e.target.value.trim();
    });
    idCell.appendChild(idInput);

    const edgeCell = document.createElement("td");
    const edgeSelect = document.createElement("select");
    for (const [value, label] of EDGE_OPTIONS) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      if (peer.edge === value) option.selected = true;
      edgeSelect.appendChild(option);
    }
    edgeSelect.addEventListener("change", (e) => {
      peers[index].edge = e.target.value;
    });
    edgeCell.appendChild(edgeSelect);

    const removeCell = document.createElement("td");
    const removeButton = document.createElement("button");
    removeButton.type = "button";
    removeButton.textContent = "✕";
    removeButton.addEventListener("click", () => {
      peers.splice(index, 1);
      renderPeers();
    });
    removeCell.appendChild(removeButton);

    row.append(nameCell, idCell, edgeCell, removeCell);
    body.appendChild(row);
  });
}

function addPeer() {
  peers.push({ device_id: "", name: "", edge: "right" });
  renderPeers();
}

async function loadSettings() {
  const settings = await invoke()("get_settings");
  document.getElementById("device_name").value = settings.device_name;
  document.getElementById("listen_port").value = settings.listen_port;
  document.getElementById("discovery_enabled").checked = settings.discovery_enabled;
  document.getElementById("pairing_mode").value = settings.pairing_mode;
  document.getElementById("log_level").value = settings.log_level;
  document.getElementById("role").value = settings.role;
  document.getElementById("server_address").value = settings.server_address ?? "";
  peers = (settings.peers ?? []).map((p) => ({ ...p }));
  renderPeers();
  updateRoleVisibility();
  updateCoreToggleLabel();
}

async function saveSettings(event) {
  event.preventDefault();
  const form = event.target;
  const serverAddress = form.server_address.value.trim();
  const settings = {
    device_name: form.device_name.value,
    listen_port: Number(form.listen_port.value),
    discovery_enabled: form.discovery_enabled.checked,
    pairing_mode: form.pairing_mode.value,
    log_level: form.log_level.value,
    role: form.role.value,
    server_address: serverAddress.length > 0 ? serverAddress : null,
    peers: peers.filter((p) => p.device_id.length > 0 && p.name.length > 0),
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
  document.getElementById("copy-device-id").addEventListener("click", copyDeviceId);
  document.getElementById("role").addEventListener("change", () => {
    updateRoleVisibility();
    updateCoreToggleLabel();
  });
  document.getElementById("add-peer").addEventListener("click", addPeer);
  document.getElementById("settings-form").addEventListener("submit", saveSettings);
  document.getElementById("core-toggle").addEventListener("click", toggleCore);

  window.__TAURI__.event.listen("core-log", (event) => appendCoreLog(event.payload));
  window.__TAURI__.event.listen("core-status", (event) => {
    coreRunning = CORE_RUNNING_STATUSES.has(event.payload);
    setConnectionIndicator(event.payload);
    updateCoreToggleLabel();
  });

  loadDeviceId();
  loadSettings();
  loadDevices();
});
