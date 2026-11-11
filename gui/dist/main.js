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
  server:
    '<svg width="18" height="18" viewBox="0 0 18 18" fill="none" aria-hidden="true"><rect x="1.5" y="2.5" width="15" height="10" rx="2" stroke="currentColor" stroke-width="1.5"/><path d="M6 15.5h6M9 12.5v3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><path d="M7.2 6.2 9 8l1.8-1.8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" transform="rotate(180 9 7.1)"/></svg>',
  client:
    '<svg width="18" height="18" viewBox="0 0 18 18" fill="none" aria-hidden="true"><rect x="3" y="3" width="12" height="8.5" rx="1.6" stroke="currentColor" stroke-width="1.5"/><path d="M1.5 14.5h15" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/><path d="M7.2 6.2 9 8l1.8-1.8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  upload:
    '<svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 10.5V2.5M4.8 5.5 8 2.3l3.2 3.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/><path d="M2.5 10v1.8a1.7 1.7 0 0 0 1.7 1.7h7.6a1.7 1.7 0 0 0 1.7-1.7V10" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>',
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
  port_busy: "Puerto ocupado",
  stopped: "Detenido",
};

const CORE_STATUS_CLASSES = {
  starting: "status--connecting",
  listening: "status--online",
  connected: "status--online",
  retrying: "status--connecting",
  error: "status--error",
  port_busy: "status--error",
  stopped: "status--offline",
};

/// Una línea que explica qué significa cada estado y qué hacer.
function statusDescription(status, role) {
  switch (status) {
    case "starting":
      return "Arrancando el servicio…";
    case "listening":
      return "Esperando que un cliente se conecte.";
    case "connected":
      return role === "server"
        ? "Mové el mouse hacia un borde para pasar al otro equipo."
        : "Este equipo recibe el mouse y el teclado del servidor.";
    case "retrying":
      return role === "client"
        ? "Buscando al servidor en la red local…"
        : "Reintentando…";
    case "error":
      return "Algo falló. Abrí la actividad del servicio para ver el detalle.";
    case "port_busy":
      return "Otro ionconnect-core usa el puerto. Deshabilitá el servicio systemd viejo.";
    default:
      return role === "server"
        ? "Iniciá el servidor para compartir este mouse y teclado."
        : "Conectate para recibir el control del servidor.";
  }
}

let corePollTimer = null;
let lastCoreStatus = "stopped";

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

function getRole() {
  return document.querySelector('input[name="role"]:checked')?.value ?? "server";
}

function setRole(role) {
  const radio = document.querySelector(`input[name="role"][value="${role}"]`);
  if (radio) radio.checked = true;
}

/// Nombre, rol e icono de la tarjeta principal.
function renderHero() {
  const role = getRole();
  document.getElementById("hero-name").textContent =
    document.getElementById("device_name").value.trim() || "Este equipo";
  document.getElementById("hero-role").textContent =
    role === "server" ? "Servidor · comparte mouse y teclado" : "Cliente · recibe el control";
  document.getElementById("hero-glyph").innerHTML = role === "server" ? ICONS.server : ICONS.client;
  setConnectionIndicator(lastCoreStatus);
}

function updateRoleVisibility() {
  const role = getRole();
  const auto = document.getElementById("discovery_enabled").checked;
  // La dirección manual solo tiene sentido en un cliente sin búsqueda automática.
  document.getElementById("client-fields").hidden = !(role === "client" && !auto);
  document.getElementById("server-fields").hidden = role !== "server";
  document.getElementById("discovery-text").textContent =
    role === "server"
      ? "Anunciar este equipo en la red local (mDNS)"
      : "Buscar el servidor automáticamente (mDNS)";
  document.getElementById("discovery-hint").textContent =
    role === "server"
      ? "Los clientes lo encuentran solos, sin escribir la IP."
      : auto
        ? "Si cambiás de servidor o de IP, se reconecta solo."
        : "";
  // El puerto de escucha solo lo usa el servidor.
  document.getElementById("port-row").hidden = role !== "server";
  updatePairingHint();
  renderHero();
}

function updatePairingHint() {
  const strict = document.getElementById("pairing_mode").value === "reject-unknown";
  document.getElementById("pairing-hint").textContent = strict
    ? "Solo se conectan equipos cuyo certificado ya conocés. Más seguro."
    : "Ideal para emparejar por primera vez en una red de confianza.";
}

function updateCoreToggleLabel() {
  const role = getRole();
  const btn = document.getElementById("core-toggle");
  btn.textContent = coreRunning
    ? role === "server"
      ? "Detener servidor"
      : "Desconectar"
    : role === "server"
      ? "Iniciar servidor"
      : "Conectar";
  // Detener es destructivo pero reversible: tono rojo suave, no relleno.
  btn.classList.toggle("btn--primary", !coreRunning);
  btn.classList.toggle("btn--danger", coreRunning);
}

function setConnectionIndicator(status) {
  const el = document.getElementById("connection-indicator");
  const label = CORE_STATUS_LABELS[status] ?? "Sin conexiones";
  const cls = CORE_STATUS_CLASSES[status] ?? "status--offline";
  el.className = `status ${cls}`;
  el.textContent = label;

  lastCoreStatus = status;
  const hero = document.getElementById("hero");
  if (!hero) return;
  hero.dataset.state = status;
  document.getElementById("hero-status-label").textContent =
    status === "stopped" ? "Detenido" : label;
  document.getElementById("hero-status-desc").textContent = statusDescription(status, getRole());
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
const SPINNER =
  '<svg class="spin" width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="3" stroke-opacity="0.25"/><path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" stroke-width="3" stroke-linecap="round"/></svg>';

function renderReceivedFiles(files) {
  const box = document.getElementById("received-files-box");
  const list = document.getElementById("received-files");
  if (!list || !box) return;
  const items = Array.isArray(files) ? files : [];
  const signature = items.join("\n");
  if (list.dataset.signature === signature) return;
  list.dataset.signature = signature;

  box.hidden = items.length === 0;
  list.innerHTML = "";
  for (const path of items.slice().reverse()) {
    const name = path.split("/").pop() || path;
    const item = el("li", "transfer-list__item");
    item.append(el("span", "transfer-list__icon transfer-list__icon--ok", "✓"));
    const body = el("div", "transfer-list__body");
    body.append(el("span", "transfer-list__name", name));
    body.append(el("span", "transfer-list__path", path));
    item.append(body);
    list.append(item);
  }
}

/// Progreso de los archivos que este equipo está mandando: spinner mientras
/// `core` no logueó `archivo enviado`, ✓ al terminar, ✕ si falló.
function renderSentFiles(files) {
  const box = document.getElementById("sent-files-box");
  const list = document.getElementById("sent-files");
  if (!list || !box) return;
  const items = Array.isArray(files) ? files : [];
  const signature = items.map((f) => `${f.name}:${f.done}:${f.failed}`).join("\n");
  if (list.dataset.signature === signature) return;
  list.dataset.signature = signature;

  box.hidden = items.length === 0;
  document.querySelector("#sent-files-box .file-group__label").textContent =
    items.some((f) => !f.done && !f.failed) ? "Enviando…" : "Enviados";

  list.innerHTML = "";
  for (const f of items.slice().reverse()) {
    const item = el("li", "transfer-list__item");
    let icon;
    if (f.failed) {
      icon = el("span", "transfer-list__icon transfer-list__icon--err", "✕");
    } else if (f.done) {
      icon = el("span", "transfer-list__icon transfer-list__icon--ok", "✓");
    } else {
      icon = el("span", "transfer-list__icon transfer-list__icon--pending");
      icon.innerHTML = SPINNER;
    }
    item.append(icon);
    const body = el("div", "transfer-list__body");
    body.append(el("span", "transfer-list__name", f.name));
    body.append(
      el(
        "span",
        "transfer-list__path",
        f.failed ? "no se pudo leer el archivo" : f.done ? "enviado" : "enviando…",
      ),
    );
    item.append(body);
    list.append(item);
  }
}

/// Rutas soltadas en la ventana (las pasa el backend vía `eval`) o elegidas
/// en el diálogo — se mandan por `send_files` y el resultado va a un toast.
window.__ionFilesDropped = async (paths) => {
  if (!Array.isArray(paths) || paths.length === 0) return;
  try {
    const message = await invoke()("send_files", { paths });
    showToast(message, "success");
  } catch (error) {
    showToast(String(error), "error");
  }
};

async function pickAndSendFiles() {
  const btn = document.getElementById("pick-files");
  btn.disabled = true;
  try {
    const message = await invoke()("send_files_dialog");
    if (message && message !== "Selección cancelada.") showToast(message, "success");
  } catch (error) {
    showToast(String(error), "error");
  } finally {
    btn.disabled = false;
  }
}

async function pollCoreSnapshot() {
  try {
    const snapshot = await invoke()("get_core_snapshot");
    coreRunning = snapshot.running;
    setConnectionIndicator(snapshot.running ? snapshot.status : "stopped");
    setCoreLog(snapshot.log);
    renderReceivedFiles(snapshot.received_files);
    renderSentFiles(snapshot.sent_files);
    updateCoreToggleLabel();
    await loadDevices(snapshot.running ? snapshot.status : "stopped");
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
      markDirty();
    });

    const idInput = el("input", "input peer__id");
    idInput.type = "text";
    idInput.value = peer.device_id;
    idInput.placeholder = "Identificador";
    idInput.spellcheck = false;
    idInput.title = peer.device_id;
    idInput.setAttribute("aria-label", "Identificador del equipo vecino");
    idInput.addEventListener("input", (e) => {
      peers[index].device_id = e.target.value.trim();
      e.target.title = e.target.value;
      markDirty();
    });

    const removeButton = el("button", "btn btn--ghost btn--icon btn--sm peer__remove");
    removeButton.type = "button";
    removeButton.innerHTML = ICONS.cross;
    removeButton.title = "Quitar este equipo";
    removeButton.setAttribute("aria-label", "Quitar este equipo");
    removeButton.addEventListener("click", () => {
      peers.splice(index, 1);
      renderPeers();
      markDirty();
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
        markDirty();
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
  markDirty();
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
  // Un cliente con dirección manual guardada arranca con la búsqueda apagada.
  document.getElementById("discovery_enabled").checked =
    settings.discovery_enabled && !(settings.role === "client" && settings.server_address);
  document.getElementById("pairing_mode").value = settings.pairing_mode;
  // Si el valor guardado no existe como opción, cae en "Todos".
  const logLevel = document.getElementById("log_level");
  logLevel.value = settings.log_level;
  if (!logLevel.value) logLevel.value = "all";
  setRole(settings.role);
  document.getElementById("server_address").value = settings.server_address ?? "";
  peers = (settings.peers ?? []).map((p) => ({ ...p }));
  renderPeers();
  updateRoleVisibility();
  updateCoreToggleLabel();
  setDirty(false);
}

/* --------------------------------------------------------------------------
   Cambios sin guardar
   -------------------------------------------------------------------------- */

let dirty = false;

function setDirty(value) {
  dirty = value;
  const bar = document.getElementById("savebar");
  bar.dataset.dirty = String(value);
  document.getElementById("savebar-state").textContent = value
    ? "Cambios sin guardar"
    : "Todo guardado";
  document.getElementById("save-settings").disabled = !value;
}

function markDirty() {
  if (!dirty) setDirty(true);
}

/// Validación en línea (no al enviar): marca el campo y devuelve el mensaje.
function validateSettings(form) {
  const name = form.device_name;
  const nameOk = name.value.trim().length > 0;
  name.setAttribute("aria-invalid", String(!nameOk));
  if (!nameOk) return { field: name, message: "Poné un nombre para este equipo." };

  if (getRole() === "server") {
    const port = form.listen_port;
    const n = Number(port.value);
    const portOk = Number.isInteger(n) && n >= 1 && n <= 65535;
    port.setAttribute("aria-invalid", String(!portOk));
    if (!portOk) return { field: port, message: "El puerto tiene que estar entre 1 y 65535." };
  }
  return null;
}

async function saveSettings(event) {
  event.preventDefault();
  const form = event.target;
  const invalid = validateSettings(form);
  if (invalid) {
    invalid.field.focus();
    showToast(invalid.message, "error");
    return;
  }
  // Con búsqueda automática la dirección manual no se usa (y se limpia para
  // que no tape al descubrimiento en el core).
  const serverAddress =
    getRole() === "client" && !form.discovery_enabled.checked
      ? form.server_address.value.trim()
      : "";
  const settings = {
    device_name: form.device_name.value,
    listen_port: Number(form.listen_port.value),
    discovery_enabled: form.discovery_enabled.checked,
    pairing_mode: form.pairing_mode.value,
    log_level: form.log_level.value,
    role: getRole(),
    server_address: serverAddress.length > 0 ? serverAddress : null,
    peers: peers.filter((p) => p.device_id.length > 0 && p.name.length > 0),
  };
  try {
    await invoke()("save_settings", { settings });
    setDirty(false);
    showToast(
      coreRunning ? "Guardado. Reiniciá el servicio para aplicarlo." : "Configuración guardada",
      "success",
    );
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

async function loadDevices(status) {
  let devices = await invoke()("list_devices");
  // Red de seguridad: si `core` dice "connected" pero el parseo del log no
  // dejó ninguna fila (nombre raro, línea que scrolleó), mostrar igual que
  // hay un equipo conectado.
  if (devices.length === 0 && status === "connected") {
    devices = [{ name: "Equipo remoto", connected: true, latency_ms: null }];
  }
  const list = document.getElementById("device-list");
  const count = document.getElementById("peer-count-indicator");

  count.textContent = String(devices.length);
  count.dataset.active = String(devices.some((d) => d.connected));
  count.title = `${devices.length} equipo${devices.length === 1 ? "" : "s"}`;

  // El sondeo corre cada segundo: solo se repinta si algo cambió, para no
  // reiniciar la animación de entrada ni perder el hover.
  const signature = `${getRole()}|` + devices
    .map((d) => `${d.name}:${d.connected}:${d.latency_ms}`)
    .join("\n");
  if (list.dataset.signature === signature) return;
  list.dataset.signature = signature;

  list.innerHTML = "";
  if (devices.length === 0) {
    const item = el("li");
    item.append(
      emptyState(
        ICONS.plug,
        "Sin equipos conectados",
        getRole() === "server"
          ? "Iniciá el servidor y abrí IonConnect en el otro equipo."
          : "Conectate: el servidor se busca solo en la red local.",
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
   Confirmación al cerrar la ventana
   El backend (Rust) intercepta el cierre real (`CloseRequested` →
   `prevent_close`) y llama a `window.__ionCloseRequested()` vía `eval`. Acá
   solo se muestra el modal; "Cerrar" invoca `confirm_close`, que hace salir
   la app de forma ordenada (SIGTERM a core → avisa a los peers).
   ========================================================================== */

function showCloseModal() {
  const overlay = document.getElementById("close-modal");
  overlay.hidden = false;
  document.getElementById("close-cancel").focus();
}

function hideCloseModal() {
  document.getElementById("close-modal").hidden = true;
}

async function confirmClose() {
  const btn = document.getElementById("close-confirm");
  btn.disabled = true;
  try {
    await invoke()("confirm_close");
  } catch {
    // Si falla, al menos que no quede el botón trabado.
    btn.disabled = false;
  }
}

/* ==========================================================================
   Arranque
   ========================================================================== */

window.addEventListener("DOMContentLoaded", () => {
  window.__ionCloseRequested = showCloseModal;
  document.getElementById("close-cancel").addEventListener("click", hideCloseModal);
  document.getElementById("close-confirm").addEventListener("click", confirmClose);
  document.getElementById("close-modal").addEventListener("click", (event) => {
    // Click en el fondo oscuro = cancelar.
    if (event.target.id === "close-modal") hideCloseModal();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !document.getElementById("close-modal").hidden) {
      hideCloseModal();
    }
  });

  renderThemeToggle();
  document.getElementById("copy-device-id").innerHTML = ICONS.copy;

  document.getElementById("theme-toggle").addEventListener("click", toggleTheme);
  document.getElementById("copy-device-id").addEventListener("click", copyDeviceId);
  document.getElementById("role-group").addEventListener("change", () => {
    updateRoleVisibility();
    updateCoreToggleLabel();
  });
  document.getElementById("discovery_enabled").addEventListener("change", updateRoleVisibility);
  document.getElementById("pairing_mode").addEventListener("change", updatePairingHint);
  document.getElementById("device_name").addEventListener("input", () => {
    renderScreenMap();
    renderHero();
  });
  const form = document.getElementById("settings-form");
  // Cualquier cambio en el formulario marca "sin guardar"; un campo marcado
  // inválido se limpia en cuanto el usuario lo corrige.
  form.addEventListener("input", (event) => {
    markDirty();
    if (event.target.getAttribute?.("aria-invalid") === "true") validateSettings(form);
  });
  form.addEventListener("change", markDirty);
  document.getElementById("discard-settings").addEventListener("click", () => {
    loadSettings();
    showToast("Cambios descartados", "success");
  });
  for (const icon of document.querySelectorAll(".role-picker__icon")) {
    icon.innerHTML = ICONS[icon.dataset.icon] ?? "";
  }
  document.getElementById("file-drop-icon").innerHTML = ICONS.upload;
  document.getElementById("add-peer").addEventListener("click", () => addPeer("right"));
  document.getElementById("screen-map").addEventListener("click", onScreenMapClick);
  document.getElementById("screen-map").addEventListener("keydown", onScreenMapKeydown);
  document.getElementById("settings-form").addEventListener("submit", saveSettings);
  document.getElementById("core-toggle").addEventListener("click", toggleCore);
  document.getElementById("pick-files").addEventListener("click", pickAndSendFiles);

  startCorePolling();

  loadDeviceId();
  loadSettings();
  loadDevices();
});
