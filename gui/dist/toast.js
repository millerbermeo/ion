/* ==========================================================================
   IonConnect — aviso flotante de transferencias
   Recibe los eventos de `core` vía `window.__ionTransfer(evento, seq)` (lo
   llama `gui/src-tauri/src/toast.rs` con `eval`). Cada transferencia es una
   tarjeta: anillo de progreso mientras avanza, que se transforma en check
   (éxito) o alerta (error) y se va sola. Cuando no queda ninguna, se le pide
   a Rust que oculte la ventana (`toast_idle`).

   Evento: { id, direction: "send"|"receive", phase: "start"|"progress"|
   "done"|"error", via: "app"|"drag"|"clipboard"|"folder", name, count,
   total, done, reason }.
   ========================================================================== */

const THEME_STORAGE_KEY = "ionconnect-theme";

/// Cuánto queda a la vista una tarjeta terminada. Un éxito se lee de un
/// vistazo; un error merece más tiempo para leer el motivo.
const DONE_MS = 2600;
const ERROR_MS = 5200;
/// Una transferencia en curso sin novedades durante este tiempo se da por
/// perdida (no debería pasar: `core` siempre cierra con done/error).
const STALE_MS = 60000;
const MAX_CARDS = 4;

/// Curvas: entrada estilo iOS (arranque rápido, asentamiento largo, sin
/// rebote) y la inversa exacta para salir por el mismo camino.
const EASE_IN = "cubic-bezier(0.32, 0.72, 0, 1)";
const EASE_OUT = "cubic-bezier(0.68, 0, 1, 0.28)";
const OFFSCREEN = "translateX(calc(100% + 28px))";

const RING_R = 16;
const RING_C = 2 * Math.PI * RING_R;

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function applyTheme() {
  let stored = null;
  try {
    stored = localStorage.getItem(THEME_STORAGE_KEY);
  } catch {
    /* sin almacenamiento: tema por defecto */
  }
  document.documentElement.setAttribute("data-theme", stored === "light" ? "light" : "dark");
}
applyTheme();
window.addEventListener("storage", applyTheme);

/* --------------------------------------------------------------------------
   Formato
   -------------------------------------------------------------------------- */

const numberFormat = new Intl.NumberFormat("es", { maximumFractionDigits: 1 });

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = value >= 100 ? 0 : 1;
  return `${new Intl.NumberFormat("es", { maximumFractionDigits: digits }).format(value)} ${units[unit]}`;
}

function formatEta(seconds) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "";
  if (seconds < 60) return `quedan ${Math.max(1, Math.round(seconds))} s`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `quedan ${minutes} min`;
  return `quedan ${numberFormat.format(minutes / 60)} h`;
}

/** "foto.png" o "3 elementos", según cuántas entradas de primer nivel hay. */
function subject(ev) {
  if (ev.count > 1) return `${ev.count} elementos`;
  return ev.name || "archivo";
}

function titleFor(ev) {
  const send = ev.direction === "send";
  switch (ev.phase) {
    case "done":
      return ev.count > 1
        ? `${subject(ev)} ${send ? "enviados" : "recibidos"}`
        : `${send ? "Enviado" : "Recibido"}: ${subject(ev)}`;
    case "error":
      return `No se pudo ${send ? "enviar" : "recibir"} ${subject(ev)}`;
    default:
      return `${send ? "Enviando" : "Recibiendo"} ${subject(ev)}`;
  }
}

/** Texto de contexto: de dónde viene / a dónde va. */
function viaText(ev) {
  const send = ev.direction === "send";
  if (ev.phase === "done") {
    if (send) {
      if (ev.via === "clipboard") return "Listo para pegar en el otro equipo";
      if (ev.via === "drag") return "En el Escritorio del otro equipo › ionconnect";
      return "Entregado al otro equipo";
    }
    if (ev.via === "clipboard") return "Listo para pegar con Ctrl+V";
    return "Guardado en Escritorio › ionconnect";
  }
  if (send) {
    if (ev.via === "clipboard") return "Pasando lo copiado al otro equipo";
    if (ev.via === "drag") return "Arrastrado al otro equipo";
    return "Al otro equipo";
  }
  if (ev.via === "clipboard") return "Copiado en el otro equipo";
  return "Desde el otro equipo";
}

/* --------------------------------------------------------------------------
   Tarjetas
   -------------------------------------------------------------------------- */

const SVG_NS = "http://www.w3.org/2000/svg";

function svg(markup) {
  const wrap = document.createElementNS(SVG_NS, "svg");
  wrap.setAttribute("viewBox", "0 0 38 38");
  wrap.setAttribute("width", "38");
  wrap.setAttribute("height", "38");
  wrap.setAttribute("aria-hidden", "true");
  wrap.innerHTML = markup;
  return wrap;
}

function buildIcon(direction) {
  const icon = document.createElement("div");
  icon.className = "icon";

  // Flecha: arriba = sale de este equipo, abajo = llega.
  const arrow =
    direction === "send"
      ? "M19 24.5V13.5M14.5 18 19 13.5 23.5 18"
      : "M19 13.5V24.5M14.5 20 19 24.5 23.5 20";
  const ring = svg(
    `<circle class="ring__track" cx="19" cy="19" r="${RING_R}" fill="none" stroke-width="2.6"/>` +
      `<circle class="ring__arc" cx="19" cy="19" r="${RING_R}" fill="none" stroke-width="2.6" ` +
      `stroke-linecap="round" stroke-dasharray="${RING_C}" stroke-dashoffset="${RING_C}"/>` +
      `<path class="ring__glyph" d="${arrow}" fill="none" stroke="currentColor" ` +
      `stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/>`,
  );
  ring.classList.add("ring");

  const success = svg(
    `<circle cx="19" cy="19" r="17"/>` +
      `<path class="badge__mark" d="M12.6 19.6 17 24 25.6 14.6" fill="none" stroke="#fff" ` +
      `stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>`,
  );
  success.classList.add("badge", "badge--success");

  const error = svg(
    `<circle cx="19" cy="19" r="17"/>` +
      `<path class="badge__mark" d="M19 11.5V21" fill="none" stroke="#fff" ` +
      `stroke-width="2.8" stroke-linecap="round"/>` +
      `<circle cx="19" cy="26" r="1.7" fill="#fff"/>`,
  );
  error.classList.add("badge", "badge--error");

  icon.append(ring, success, error);
  return icon;
}

function buildCard(ev) {
  const card = document.createElement("article");
  card.className = "card";

  const body = document.createElement("div");
  body.className = "card__body";
  const title = document.createElement("p");
  title.className = "card__title";
  const subtitle = document.createElement("p");
  subtitle.className = "card__subtitle";
  body.append(title, subtitle);

  const meta = document.createElement("span");
  meta.className = "card__meta";

  const bar = document.createElement("div");
  bar.className = "card__bar";
  const fill = document.createElement("div");
  fill.className = "card__bar-fill";
  bar.append(fill);

  card.append(buildIcon(ev.direction), body, meta, bar);
  return {
    el: card,
    title,
    subtitle,
    meta,
    fill,
    arc: card.querySelector(".ring__arc"),
    timer: null,
    staleTimer: null,
    leaving: false,
    rate: 0,
    lastDone: 0,
    lastAt: performance.now(),
    startedAt: performance.now(),
  };
}

const stack = () => document.getElementById("stack");
const cards = new Map();
let lastSeq = 0;

/** Posición de cada tarjeta antes de un cambio de layout (FLIP). */
function snapshot() {
  const tops = new Map();
  for (const card of cards.values()) tops.set(card.el, card.el.getBoundingClientRect().top);
  return tops;
}

/** Desliza las tarjetas desde donde estaban a su nuevo lugar. */
function settle(before) {
  if (reducedMotion()) return;
  for (const [el, top] of before) {
    if (!el.isConnected) continue;
    const delta = top - el.getBoundingClientRect().top;
    if (Math.abs(delta) < 0.5) continue;
    el.animate([{ transform: `translateY(${delta}px)` }, { transform: "none" }], {
      duration: 420,
      easing: EASE_IN,
      composite: "add",
    });
  }
}

function enter(card) {
  const before = snapshot();
  stack().prepend(card.el);
  settle(before);
  const frames = reducedMotion()
    ? [{ opacity: 0 }, { opacity: 1 }]
    : [
        { transform: `${OFFSCREEN} scale(0.98)`, opacity: 0 },
        { transform: "none", opacity: 1 },
      ];
  card.el.animate(frames, { duration: reducedMotion() ? 200 : 560, easing: EASE_IN });
}

function dismiss(id) {
  const card = cards.get(id);
  if (!card || card.leaving) return;
  card.leaving = true;
  clearTimeout(card.timer);
  clearTimeout(card.staleTimer);

  // Sale por donde entró, partiendo del valor en pantalla (si todavía estaba
  // entrando, no salta).
  const current = getComputedStyle(card.el);
  const frames = reducedMotion()
    ? [{ opacity: current.opacity }, { opacity: 0 }]
    : [
        { transform: current.transform === "none" ? "none" : current.transform, opacity: current.opacity },
        { transform: `${OFFSCREEN} scale(0.98)`, opacity: 0 },
      ];
  for (const anim of card.el.getAnimations()) anim.cancel();
  const exit = card.el.animate(frames, {
    duration: reducedMotion() ? 180 : 380,
    easing: EASE_OUT,
    fill: "forwards",
  });
  exit.finished
    .catch(() => {})
    .then(() => {
      // Puede haberla reemplazado otra tarjeta con el mismo id.
      if (cards.get(id) === card) cards.delete(id);
      const before = snapshot();
      card.el.remove();
      settle(before);
      maybeIdle();
    });
}

function maybeIdle() {
  if (cards.size > 0) return;
  // Dar tiempo a que termine el reacomodo antes de ocultar la ventana.
  setTimeout(() => {
    if (cards.size === 0) {
      window.__TAURI__?.core?.invoke("toast_idle", { seq: lastSeq }).catch(() => {});
    }
  }, 60);
}

/** Si hay demasiadas, se va la más vieja — terminada antes que en curso. */
function enforceLimit() {
  const live = [...cards.entries()].filter(([, c]) => !c.leaving);
  if (live.length <= MAX_CARDS) return;
  const finished = live.filter(([, c]) => c.el.matches(".card--done, .card--error"));
  const victim = (finished.length ? finished : live)[0];
  dismiss(victim[0]);
}

function render(card, ev) {
  const final = ev.phase === "done" || ev.phase === "error";
  const known = ev.total > 0;
  const fraction = known ? Math.min(1, ev.done / ev.total) : 0;

  card.el.classList.toggle("card--progress", !final);
  card.el.classList.toggle("card--indeterminate", !final && !known);
  card.el.classList.toggle("card--done", ev.phase === "done");
  card.el.classList.toggle("card--error", ev.phase === "error");
  card.el.setAttribute("aria-label", titleFor(ev));

  card.title.textContent = titleFor(ev);

  if (ev.phase === "error") {
    card.subtitle.textContent = ev.reason || "Algo salió mal";
  } else if (final || ev.done === 0) {
    card.subtitle.textContent = viaText(ev);
  } else {
    // Velocidad suavizada para el tiempo restante.
    const now = performance.now();
    const dt = (now - card.lastAt) / 1000;
    if (dt > 0 && ev.done > card.lastDone) {
      const instant = (ev.done - card.lastDone) / dt;
      card.rate = card.rate ? card.rate * 0.75 + instant * 0.25 : instant;
    }
    card.lastAt = now;
    card.lastDone = ev.done;

    const parts = [known ? `${formatBytes(ev.done)} de ${formatBytes(ev.total)}` : `${formatBytes(ev.done)} ${ev.direction === "send" ? "enviados" : "recibidos"}`];
    if (known && card.rate > 0 && now - card.startedAt > 1200) {
      const eta = formatEta((ev.total - ev.done) / card.rate);
      if (eta) parts.push(eta);
    }
    card.subtitle.textContent = parts.join(" · ");
  }

  card.meta.textContent = !final && known ? `${Math.floor(fraction * 100)}%` : "";
  card.fill.style.transform = known ? `scaleX(${fraction})` : "";
  card.arc.setAttribute(
    "stroke-dashoffset",
    String(known ? RING_C * (1 - Math.max(0.02, fraction)) : RING_C * 0.72),
  );

  clearTimeout(card.timer);
  clearTimeout(card.staleTimer);
  if (final) {
    card.timer = setTimeout(() => dismiss(ev.id), ev.phase === "error" ? ERROR_MS : DONE_MS);
  } else {
    card.staleTimer = setTimeout(() => dismiss(ev.id), STALE_MS);
  }
}

window.__ionTransfer = (ev, seq) => {
  lastSeq = Math.max(lastSeq, seq || 0);
  applyTheme();
  if (!ev || typeof ev.id !== "number") return;

  let card = cards.get(ev.id);
  if (card && card.leaving) {
    // Llegó un evento para una tarjeta que se estaba yendo: se reemplaza.
    card = undefined;
  }
  if (!card) {
    if (ev.phase === "progress" && cards.size >= MAX_CARDS * 2) return;
    card = buildCard(ev);
    cards.set(ev.id, card);
    render(card, ev);
    enter(card);
    enforceLimit();
    return;
  }
  render(card, ev);
};
