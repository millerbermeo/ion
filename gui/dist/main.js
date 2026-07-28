/* ==========================================================================
   IonConnect — GUI
   Este archivo se carga bloqueante desde el <head> (ver index.html): el
   bloque de tema de acá abajo corre antes del primer paint para que no haya
   destello. Todo lo que toca el DOM va dentro de DOMContentLoaded.
   ========================================================================== */

const THEME_STORAGE_KEY = "ionconnect-theme";

/// El tema por defecto del producto es oscuro: si no hay preferencia
/// guardada no se consulta `prefers-color-scheme`, se asume oscuro.
function applyStoredTheme() {
  const stored = localStorage.getItem(THEME_STORAGE_KEY);
  const theme = stored === "light" ? "light" : "dark";
  document.documentElement.setAttribute("data-theme", theme);
  return theme;
}

applyStoredTheme();

/* ==========================================================================
   Iconos (SVG inline — la CSP prohíbe `data:` URIs en CSS)
   ========================================================================== */

const ICONS = {
  sun: '<svg width="15" height="15" viewBox="0 0 15 15" fill="none" aria-hidden="true"><circle cx="7.5" cy="7.5" r="3.25" stroke="currentColor" stroke-width="1.4"/><path d="M7.5 1v1.5M7.5 12.5V14M14 7.5h-1.5M2.5 7.5H1M12.1 2.9l-1.06 1.06M3.96 11.04L2.9 12.1M12.1 12.1l-1.06-1.06M3.96 3.96L2.9 2.9" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
  moon: '<svg width="15" height="15" viewBox="0 0 15 15" fill="none" aria-hidden="true"><path d="M13 8.9A5.6 5.6 0 0 1 6.1 2a5.75 5.75 0 1 0 6.9 6.9Z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/></svg>',
  copy: '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><rect x="4.7" y="4.7" width="8" height="8" rx="1.6" stroke="currentColor" stroke-width="1.4"/><path d="M9.8 4.7V2.9a1.6 1.6 0 0 0-1.6-1.6H2.9a1.6 1.6 0 0 0-1.6 1.6v5.3a1.6 1.6 0 0 0 1.6 1.6h1.8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
  check: '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><path d="M2.5 7.4 5.6 10.5 11.5 4" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  cross: '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true"><path d="M2.8 2.8l6.4 6.4M9.2 2.8l-6.4 6.4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>',
  alert: '<svg width="14" height="14" viewBox="0 0 14 14" fill="none" aria-hidden="true"><circle cx="7" cy="7" r="5.8" stroke="currentColor" stroke-width="1.4"/><path d="M7 4.2v3.4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><circle cx="7" cy="9.7" r=".85" fill="currentColor"/></svg>',
  screens:
    '<svg width="26" height="26" viewBox="0 0 26 26" fill="none" aria-hidden="true"><rect x="1.5" y="5" width="11" height="8.5" rx="1.6" stroke="currentColor" stroke-width="1.4"/><rect x="14" y="12.5" width="10.5" height="8.5" rx="1.6" stroke="currentColor" stroke-width="1.4"/><path d="M7 13.5v2.5a2 2 0 0 0 2 2h5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
  plug: '<svg width="26" height="26" viewBox="0 0 26 26" fill="none" aria-hidden="true"><path d="M9.5 3.5v5M16.5 3.5v5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/><path d="M6.5 8.5h13v3a6.5 6.5 0 0 1-13 0v-3Z" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/><path d="M13 18v4.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"/></svg>',
};

/* ==========================================================================
   Estado
   ========================================================================== */

/// Etiquetas cortas para el control segmentado de cada equipo.
const EDGE_OPTIONS = [
  ["left", "Izq."],
  ["right", "Der."],
  ["top", "Arriba"],
  ["bottom", "Abajo"],
];

/// Etiquetas largas para el mapa de pantalla.
const EDGE_LABELS = {
  left: "Izquierda",
  right: "Derecha",
  top: "Arriba",
  bottom: "Abajo",
};

let peers = [];
let coreRunning = false;

const CORE_STATUS_LABELS = {
  starting: "Iniciando…",
  listening: "Escuchando conexiones",
  connected: "Conectado",
  retrying: "Reintentando conexión…",
  error: "Error — mirá la actividad",
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

let corePollTimer = null;

const invoke = () => window.__TAURI__.core.invoke;

/* ==========================================================================
   Tema
   ========================================================================== */

function renderThemeToggle() {
  const isDark = document.documentElement.getAttribute("data-theme") !== "light";
  const btn = document.getElementById("theme-toggle");
  // Se muestra el icono del tema al que se va a cambiar, no el actual.
  btn.innerHTML = isDark ? ICONS.sun : ICONS.moon;
  btn.title = isDark ? "Cambiar a tema claro" : "Cambiar a tema oscuro";
  btn.setAttribute("aria-label", btn.title);
}

function toggleTheme() {
  const isDark = document.documentElement.getAttribute("data-theme") !== "light";
  const next = isDark ? "light" : "dark";
  document.documentElement.setAttribute("data-theme", next);
  localStorage.setItem(THEME_STORAGE_KEY, next);
  renderThemeToggle();
}

/* ==========================================================================
   Identificador de este equipo
   ========================================================================== */

async function loadDeviceId() {
  const deviceId = await invoke()("get_device_id");
  document.getElementById("device-id").value = deviceId;
}

let copyResetTimer;
function copyDeviceId() {
  const field = document.getElementById("device-id");
  field.select();
  navigator.clipboard?.writeText(field.value);

  const btn = document.getElementById("copy-device-id");
  clearTimeout(copyResetTimer);
  btn.innerHTML = ICONS.check;
  btn.classList.add("btn--copied");
  btn.title = "Copiado";
  btn.setAttribute("aria-label", "Identificador copiado");
  copyResetTimer = setTimeout(() => {
    btn.innerHTML = ICONS.copy;
    btn.classList.remove("btn--copied");
    btn.title = "Copiar identificador";
    btn.setAttribute("aria-label", "Copiar identificador");
  }, 1500);
}

/* ==========================================================================
   Rol y control del servicio
   ========================================================================== */

function updateRoleVisibility() {
  const role = document.getElementById("role").value;
  document.getElementById("client-fields").hidden = role !== "client";
  document.getElementById("server-fields").hidden = role !== "server";
  // `listen_port` vive dentro de #server-fields: si quedara `required`
  // mientras está oculto, la validación del formulario bloquearía el envío
  // sin poder mostrar el mensaje (el campo no es enfocable).
  document.getElementById("listen_port").required = role === "server";
}

function updateCoreToggleLabel() {
  const role = document.getElementById("role").value;
  const btn = document.getElementById("core-toggle");
  btn.textContent = coreRunning
    ? "Detener servicio"
    : role === "server"
      ? "Iniciar servidor"
      : "Conectar";
}

function setConnectionIndicator(status) {
  const el = document.getElementById("connection-indicator");
  const label = CORE_STATUS_LABELS[status] ?? "Sin conexiones";
  const cls = CORE_STATUS_CLASSES[status] ?? "status--offline";
  el.className = `status ${cls}`;
  el.textContent = label;
}

function setCoreLog(lines) {
  const pre = document.getElementById("core-log-view");
  const text = lines.join("\n");
  if (pre.textContent === text) return;
  const wasScrolledToBottom = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 4;
  pre.textContent = text;
  if (wasScrolledToBottom) {
    pre.scrollTop = pre.scrollHeight;
  }
}

/// Única fuente de verdad para el estado de `core`: no confiamos en que
/// los eventos hayan llegado bien al webview, así que consultamos
/// `get_core_snapshot` cada segundo y pintamos lo que diga el backend.
async function pollCoreSnapshot() {
  try {
    const snapshot = await invoke()("get_core_snapshot");
    coreRunning = snapshot.running;
    setConnectionIndicator(snapshot.running ? snapshot.status : "stopped");
    setCoreLog(snapshot.log);
    updateCoreToggleLabel();
    await loadDevices();
  } catch {
    // get_core_snapshot no debería fallar nunca; si pasa, seguimos
    // sondeando en el próximo tick en vez de romper el polling.
  }
}

function startCorePolling() {
  if (corePollTimer) return;
  pollCoreSnapshot();
  corePollTimer = setInterval(pollCoreSnapshot, 1000);
}

async function toggleCore() {
  const btn = document.getElementById("core-toggle");
  btn.disabled = true;
  try {
    if (coreRunning) {
      await invoke()("stop_core");
    } else {
      await invoke()("start_core");
    }
  } catch (error) {
    setCoreLog([`[gui] ${error}`]);
  } finally {
    await pollCoreSnapshot();
    btn.disabled = false;
  }
}

/* ==========================================================================
   Utilidades de DOM
   ========================================================================== */

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/// Estado vacío con icono, título y una línea que dice qué hacer.
function emptyState(icon, title, hint) {
  const box = el("div", "empty");
  const iconBox = el("div", "empty__icon");
  iconBox.innerHTML = icon;
  box.append(iconBox, el("p", "empty__title", title), el("p", "empty__hint", hint));
  return box;
}

/* ==========================================================================
   Mapa de pantalla
   Se deriva enteramente de `peers` — no agrega estado propio. Un borde
   libre crea un equipo de ese lado; uno ocupado enfoca su tarjeta.
   ========================================================================== */

function renderScreenMap() {
  document.getElementById("screen-map-name").textContent =
    document.getElementById("device_name").value || "Este equipo";

  for (const zone of document.querySelectorAll(".screen-map__zone")) {
    const edge = zone.dataset.edge;
    const onEdge = peers.filter((p) => p.edge === edge);
    zone.innerHTML = "";
    zone.classList.toggle("screen-map__zone--filled", onEdge.length > 0);

    if (onEdge.length === 0) {
      zone.append(el("span", "screen-map__edge-label", EDGE_LABELS[edge]));
      zone.append(el("span", "screen-map__peer", "+ agregar"));
      zone.title = `Agregar un equipo a la ${EDGE_LABELS[edge].toLowerCase()}`;
    } else {
      for (const peer of onEdge) {
        zone.append(el("span", "screen-map__peer", peer.name || "Sin nombre"));
      }
      zone.title = `${EDGE_LABELS[edge]} — ${onEdge.length} equipo(s). Tocá para editarlo.`;
    }
    zone.setAttribute("aria-label", zone.title);
  }
}

function onScreenMapClick(event) {
  const zone = event.target.closest(".screen-map__zone");
  if (!zone) return;
  const edge = zone.dataset.edge;
  const index = peers.findIndex((p) => p.edge === edge);
  if (index === -1) {
    addPeer(edge);
  } else {
    // Ya hay un equipo de ese lado: llevar el foco a su tarjeta.
    const input = document.querySelector(`.peer[data-index="${index}"] .peer__name`);
    input?.focus();
    input?.scrollIntoView({ block: "nearest" });
  }
}

/// A qué zona salta el foco desde cada borde, según la flecha. Los botones
/// ya responden a Enter y Espacio por ser `<button>` nativos; esto agrega
/// el desplazamiento espacial que se espera de un mapa.
const SCREEN_MAP_NAV = {
  top: { ArrowLeft: "left", ArrowRight: "right", ArrowDown: "bottom" },
  left: { ArrowUp: "top", ArrowDown: "bottom", ArrowRight: "right" },
  right: { ArrowUp: "top", ArrowDown: "bottom", ArrowLeft: "left" },
  bottom: { ArrowUp: "top", ArrowLeft: "left", ArrowRight: "right" },
};

function onScreenMapKeydown(event) {
  const zone = event.target.closest(".screen-map__zone");
  if (!zone) return;
  const target = SCREEN_MAP_NAV[zone.dataset.edge]?.[event.key];
  if (!target) return;
  event.preventDefault();
  document.querySelector(`.screen-map__zone[data-edge="${target}"]`)?.focus();
}

/* ==========================================================================
   Tarjetas de equipos vecinos (configuración del servidor)
   ========================================================================== */

function renderPeers() {
  const body = document.getElementById("peers-body");
  body.innerHTML = "";

  if (peers.length === 0) {
    body.append(
      emptyState(
        ICONS.screens,
        "Todavía no hay equipos vecinos",
        "Tocá un borde del mapa de arriba para agregar el primero.",
      ),
    );
    renderScreenMap();
    return;
  }

  peers.forEach((peer, index) => {
    const card = el("div", "peer");
    card.dataset.index = String(index);

    // --- Fila 1: nombre + identificador + eliminar ---
    const top = el("div", "peer__row");

    const nameInput = el("input", "input peer__name");
    nameInput.type = "text";
    nameInput.value = peer.name;
    nameInput.placeholder = "Nombre del equipo";
    nameInput.setAttribute("aria-label", "Nombre del equipo vecino");
    nameInput.addEventListener("input", (e) => {
      peers[index].name = e.target.value;
      renderScreenMap();
    });

    const idInput = el("input", "input peer__id");
    idInput.type = "text";
    idInput.value = peer.device_id;
    idInput.placeholder = "Identificador del equipo";
    idInput.title = peer.device_id;
    idInput.setAttribute("aria-label", "Identificador del equipo vecino");
    idInput.addEventListener("input", (e) => {
      peers[index].device_id = e.target.value.trim();
      e.target.title = e.target.value;
    });

    const removeButton = el("button", "btn btn--danger btn--icon btn--sm");
    removeButton.type = "button";
    removeButton.innerHTML = ICONS.cross;
    removeButton.title = "Quitar este equipo";
    removeButton.setAttribute("aria-label", "Quitar este equipo");
    removeButton.addEventListener("click", () => {
      peers.splice(index, 1);
      renderPeers();
    });

    top.append(nameInput, idInput, removeButton);

    // --- Fila 2: lado de la pantalla ---
    // Se usan radios nativos (no `role="radio"`) para heredar del navegador
    // la navegación con flechas y la semántica de grupo sin JS extra.
    const foot = el("div", "peer__foot");
    foot.append(el("span", "peer__edge-label", "Lado"));

    const picker = el("div", "edge-picker");
    for (const [value, label] of EDGE_OPTIONS) {
      const opt = el("label", "edge-picker__opt");
      const radio = el("input", "edge-picker__input");
      radio.type = "radio";
      radio.name = `edge-${index}`;
      radio.value = value;
      radio.checked = peer.edge === value;
      radio.addEventListener("change", () => {
        peers[index].edge = value;
        // Solo se repinta el mapa: repintar las tarjetas perdería el foco.
        renderScreenMap();
      });
      opt.append(radio, el("span", "edge-picker__face", label));
      picker.append(opt);
    }
    foot.append(picker);

    card.append(top, foot);
    body.append(card);
  });

  renderScreenMap();
}

/// `edge` puede venir del mapa de pantalla. Se valida el tipo porque este
/// mismo nombre se usa como manejador de click, donde el primer argumento
/// sería el `Event`.
function addPeer(edge) {
  const side = typeof edge === "string" ? edge : "right";
  peers.push({ device_id: "", name: "", edge: side });
  renderPeers();
  // Foco en el nombre del equipo recién creado.
  document.querySelector(`.peer[data-index="${peers.length - 1}"] .peer__name`)?.focus();
}

/* ==========================================================================
   Configuración
   ========================================================================== */

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
  try {
    await invoke()("save_settings", { settings });
    showToast("Configuración guardada", "success");
  } catch (error) {
    showToast(`No se pudo guardar: ${error}`, "error");
  }
}

let toastTimeout;
function showToast(message, type) {
  const toast = document.getElementById("save-status");
  clearTimeout(toastTimeout);
  toast.innerHTML = "";
  const icon = el("span", "toast__icon");
  icon.innerHTML = type === "success" ? ICONS.check : ICONS.alert;
  toast.append(icon, el("span", null, message));
  toast.className = `toast toast--${type} toast--visible`;
  toastTimeout = setTimeout(() => {
    toast.classList.remove("toast--visible");
  }, 3000);
}

/* ==========================================================================
   Equipos conectados
   ========================================================================== */

async function loadDevices() {
  const devices = await invoke()("list_devices");
  const list = document.getElementById("device-list");
  const count = document.getElementById("peer-count-indicator");

  count.textContent =
    devices.length === 0
      ? "Sin equipos"
      : `${devices.length} equipo${devices.length === 1 ? "" : "s"}`;

  list.innerHTML = "";
  if (devices.length === 0) {
    const item = el("li");
    item.append(
      emptyState(
        ICONS.plug,
        "Sin equipos conectados",
        "Iniciá el servicio y esperá a que el otro equipo se conecte.",
      ),
    );
    list.append(item);
    return;
  }

  for (const device of devices) {
    const item = el("li", "device");
    item.append(el("div", "device__avatar", (device.name || "?").charAt(0)));

    const bodyBox = el("div", "device__body");
    bodyBox.append(el("span", "device__name", device.name || "Sin nombre"));
    if (device.latency_ms !== null && device.latency_ms !== undefined) {
      bodyBox.append(el("span", "device__meta", `${device.latency_ms} ms`));
    }
    item.append(bodyBox);

    const pill = el(
      "span",
      `status ${device.connected ? "status--online" : "status--offline"}`,
      device.connected ? "Conectado" : "Desconectado",
    );
    item.append(pill);
    list.append(item);
  }
}

/* ==========================================================================
   Arranque
   ========================================================================== */

window.addEventListener("DOMContentLoaded", () => {
  renderThemeToggle();
  document.getElementById("copy-device-id").innerHTML = ICONS.copy;

  document.getElementById("theme-toggle").addEventListener("click", toggleTheme);
  document.getElementById("copy-device-id").addEventListener("click", copyDeviceId);
  document.getElementById("role").addEventListener("change", () => {
    updateRoleVisibility();
    updateCoreToggleLabel();
  });
  document.getElementById("device_name").addEventListener("input", renderScreenMap);
  document.getElementById("add-peer").addEventListener("click", () => addPeer("right"));
  document.getElementById("screen-map").addEventListener("click", onScreenMapClick);
  document.getElementById("screen-map").addEventListener("keydown", onScreenMapKeydown);
  document.getElementById("settings-form").addEventListener("submit", saveSettings);
  document.getElementById("core-toggle").addEventListener("click", toggleCore);

  startCorePolling();

  loadDeviceId();
  loadSettings();
  loadDevices();
});
