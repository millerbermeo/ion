//! Transferencia de archivos entre equipos por el canal TCP+TLS confiable.
//!
//! Un archivo se manda como `FileOffer` → `FileChunk`* → `FileEnd` (o
//! `FileAbort` si la lectura falla a mitad). El receptor va anexando cada
//! trozo a `<nombre>.<id>.part` y, al recibir `FileEnd`, lo mueve a su nombre
//! definitivo (evitando pisar uno ya existente). No hay control de flujo
//! propio: el `mpsc` acotado por el que salen los mensajes ya frena al emisor
//! si la red no da abasto, y TCP garantiza orden y entrega.
//!
//! Varios archivos y/o carpetas movidos en una sola acción viajan como un
//! **lote** ([`FileBatch`]): cada `FileOffer` lleva el id del lote y su ruta
//! relativa, y un `FileBatchEnd` lo cierra. El tipo de lote decide el
//! destino:
//! - [`FileBatchKind::Downloads`] — arrastre con el mouse de un equipo al
//!   otro: queda en `<Escritorio>/ionconnect`.
//! - [`FileBatchKind::Clipboard`] — Ctrl+C de archivos: va a una carpeta de
//!   caché y, al cerrarse el lote, quien lo recibe lo publica en su
//!   portapapeles para que Ctrl+V lo pegue donde el usuario quiera.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ionconnect_ipc::IpcServer;
use ionconnect_protocol::{
    FileAbort, FileBatch, FileBatchEnd, FileBatchKind, FileChunk, FileEnd, FileOffer,
    MIME_DIRECTORY, Message,
};
use ionconnect_shared::DeviceId;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::routing::Routing;

/// `transfer_id` global de este proceso — solo tiene que ser único mientras
/// una transferencia está viva, así que un contador que nunca se reinicia
/// sobra.
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

/// Id de lote, mismo criterio que [`NEXT_TRANSFER_ID`].
static NEXT_BATCH_ID: AtomicU64 = AtomicU64::new(1);

/// Tamaño de cada `FileChunk`. Bien por debajo del límite de frame del códec
/// (1 `MiB`) para dejar lugar a la cabecera `postcard`, y chico como para no
/// monopolizar la conexión frente al mouse/teclado.
pub const CHUNK_SIZE: usize = 128 * 1024;

/// Tope de lo que se transfiere **solo** por haber copiado o arrastrado algo
/// (no aplica al envío explícito desde la GUI). Un Ctrl+C distraído sobre
/// una carpeta enorme no debería saturar la red ni llenar el disco del otro
/// equipo sin que nadie lo haya pedido.
pub const AUTO_TRANSFER_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Tope de entradas (archivos + carpetas) de un lote automático — mismo
/// motivo que [`AUTO_TRANSFER_MAX_BYTES`].
pub const AUTO_TRANSFER_MAX_ENTRIES: usize = 50_000;

/// Cuántos lotes de portapapeles recibidos se conservan en la caché. Dos y no
/// uno: un pegado del lote anterior puede seguir copiando cuando llega el
/// siguiente.
const CLIPBOARD_CACHE_KEEP: usize = 2;

/// Carpeta `ionconnect` en el **Escritorio** del usuario — el mismo lugar en
/// todos los sistemas operativos, independiente del nombre localizado de la
/// carpeta ("Escritorio", "Desktop", ...). `dirs::desktop_dir` resuelve el
/// nombre real (XDG `user-dirs.dirs` en Linux, carpeta conocida en Windows,
/// `~/Desktop` en macOS); si no se puede, se cae a `~/Desktop` y por último
/// al directorio actual. La carpeta se crea sola en la primera recepción
/// (ver [`IncomingFiles::on_offer`]) y también al arrancar `core`.
#[must_use]
pub fn default_transfer_dir() -> PathBuf {
    let desktop = dirs::desktop_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Desktop")))
        .unwrap_or_else(|| PathBuf::from("."));
    desktop.join("ionconnect")
}

/// Carpeta de caché donde aterrizan los archivos copiados en otro equipo
/// hasta que el usuario los pega. Cada lote va a su propia subcarpeta.
#[must_use]
pub fn clipboard_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ionconnect")
        .join("clipboard")
}

/// Crea la carpeta de transferencias si no existe — se llama al arrancar
/// `core` para que el usuario la vea antes de la primera transferencia — y
/// vacía la caché de portapapeles de la ejecución anterior (el portapapeles
/// ya no apunta ahí).
pub async fn ensure_transfer_dir() {
    let dir = default_transfer_dir();
    match fs::create_dir_all(&dir).await {
        Ok(()) => info!(dir = %dir.display(), "carpeta de transferencias lista"),
        Err(err) => {
            warn!(%err, dir = %dir.display(), "no se pudo crear la carpeta de transferencias");
        }
    }
    let _ = fs::remove_dir_all(clipboard_cache_dir()).await;
}

/// Adivina un `MIME` razonable por la extensión — no vale la pena una
/// dependencia entera para esto; si no matchea, `application/octet-stream`.
fn mime_from_ext(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("mp4") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("txt" | "md") => "text/plain",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Sanea un único componente de ruta: sin separadores ni `NUL`, y nunca
/// vacío, `.` ni `..`.
fn sanitize_component(component: &str) -> Option<String> {
    let cleaned: String = component
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Deja solo el nombre de archivo (sin ruta, sin `..`, sin separadores) para
/// que un emisor no pueda hacer que el receptor escriba fuera de su carpeta
/// de descargas.
fn sanitize_name(name: &str) -> String {
    sanitize_relative(name)
        .pop()
        .unwrap_or_else(|| "archivo".to_string())
}

/// Parte `name` en componentes seguros. Acepta `/` y `\` como separador (el
/// emisor puede ser Windows), descarta `.`, `..`, vacíos, prefijos de unidad
/// y la raíz — así una ruta relativa de un lote nunca puede escapar de la
/// carpeta destino. Nunca devuelve una lista vacía.
fn sanitize_relative(name: &str) -> Vec<String> {
    let mut parts: Vec<String> = name
        .split(['/', '\\'])
        // `C:` de una ruta de Windows: no es un nombre válido de carpeta.
        .filter(|part| !(part.len() == 2 && part.ends_with(':')))
        .filter_map(sanitize_component)
        .collect();
    if parts.is_empty() {
        parts.push("archivo".to_string());
    }
    parts
}

/// Primera ruta libre en `dir` para `name`: `foto.png`, luego
/// `foto (1).png`, `foto (2).png`, ...
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let direct = dir.join(name);
    if !direct.exists() {
        return direct;
    }
    let as_path = Path::new(name);
    let stem = as_path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = as_path.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let candidate = match ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    direct
}

/// Una transferencia entrante a medio recibir.
struct Partial {
    file: fs::File,
    part_path: PathBuf,
    /// Carpeta donde queda el archivo terminado.
    dest_dir: PathBuf,
    final_name: String,
    /// Lote al que pertenece y si es una entrada de primer nivel de ese lote
    /// (las de primer nivel son las que se publican en el portapapeles).
    batch: Option<(u64, bool)>,
    received: u64,
    total: u64,
}

/// Un lote entrante a medio recibir.
struct IncomingBatch {
    kind: FileBatchKind,
    /// Carpeta base del lote: la de transferencias, o una subcarpeta propia
    /// de la caché de portapapeles.
    dir: PathBuf,
    /// Carpetas de primer nivel ya materializadas: nombre que mandó el emisor
    /// → ruta real (puede diferir si ya existía una con ese nombre).
    roots: HashMap<String, PathBuf>,
    /// Entradas de primer nivel ya completas, en orden de llegada.
    top_level: Vec<PathBuf>,
}

/// Un lote que terminó de llegar — ver [`IncomingFiles::on_batch_end`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedBatch {
    pub kind: FileBatchKind,
    /// Archivos y carpetas de primer nivel del lote, ya en su ruta final.
    pub paths: Vec<PathBuf>,
}

/// Estado del lado receptor: las transferencias entrantes en curso.
pub struct IncomingFiles {
    dir: PathBuf,
    clipboard_dir: PathBuf,
    active: HashMap<u64, Partial>,
    batches: HashMap<u64, IncomingBatch>,
}

impl IncomingFiles {
    /// `dir` es la carpeta de transferencias; los lotes de portapapeles van
    /// a [`clipboard_cache_dir`].
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self::with_clipboard_dir(dir, clipboard_cache_dir())
    }

    #[must_use]
    pub fn with_clipboard_dir(dir: PathBuf, clipboard_dir: PathBuf) -> Self {
        Self {
            dir,
            clipboard_dir,
            active: HashMap::new(),
            batches: HashMap::new(),
        }
    }

    /// Estado del lote `batch`, creándolo (y su carpeta) la primera vez que
    /// aparece.
    async fn batch_entry(&mut self, batch: FileBatch) -> std::io::Result<&mut IncomingBatch> {
        if !self.batches.contains_key(&batch.id) {
            let dir = match batch.kind {
                FileBatchKind::Downloads => self.dir.clone(),
                FileBatchKind::Clipboard => {
                    let stamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    self.clipboard_dir.join(format!("{stamp}-{}", batch.id))
                }
            };
            fs::create_dir_all(&dir).await?;
            self.batches.insert(
                batch.id,
                IncomingBatch {
                    kind: batch.kind,
                    dir,
                    roots: HashMap::new(),
                    top_level: Vec::new(),
                },
            );
        }
        Ok(self
            .batches
            .get_mut(&batch.id)
            .expect("el lote se acaba de insertar"))
    }

    /// Resuelve dónde va una entrada: `(carpeta destino, nombre final,
    /// es de primer nivel)`. Para un lote, materializa la carpeta de primer
    /// nivel la primera vez que se la ve, con un nombre que no pise nada.
    async fn resolve_destination(
        &mut self,
        offer: &FileOffer,
    ) -> std::io::Result<(PathBuf, String, bool)> {
        let Some(batch) = offer.batch else {
            fs::create_dir_all(&self.dir).await?;
            return Ok((self.dir.clone(), sanitize_name(&offer.name), true));
        };
        let mut parts = sanitize_relative(&offer.name);
        let name = parts.pop().expect("sanitize_relative nunca devuelve vacío");
        let state = self.batch_entry(batch).await?;
        if parts.is_empty() {
            return Ok((state.dir.clone(), name, true));
        }
        let top = parts.remove(0);
        let root = if let Some(root) = state.roots.get(&top) {
            root.clone()
        } else {
            let root = unique_path(&state.dir, &top);
            fs::create_dir_all(&root).await?;
            state.roots.insert(top, root.clone());
            state.top_level.push(root.clone());
            root
        };
        let dest_dir = parts.iter().fold(root, |dir, part| dir.join(part));
        fs::create_dir_all(&dest_dir).await?;
        Ok((dest_dir, name, false))
    }

    pub async fn on_offer(&mut self, offer: FileOffer) {
        let (dest_dir, name, top_level) = match self.resolve_destination(&offer).await {
            Ok(resolved) => resolved,
            Err(err) => {
                warn!(%err, "no se pudo preparar la carpeta destino de la transferencia");
                return;
            }
        };

        if offer.mime == MIME_DIRECTORY {
            // Una carpeta (posiblemente vacía): solo se crea, no lleva trozos.
            // De primer nivel pasa por `roots` para no pisar una existente.
            let created = if let (true, Some(batch)) = (top_level, offer.batch) {
                let state = self
                    .batches
                    .get_mut(&batch.id)
                    .expect("resolve_destination ya creó el lote");
                if state.roots.contains_key(&name) {
                    Ok(())
                } else {
                    let root = unique_path(&state.dir, &name);
                    state.roots.insert(name.clone(), root.clone());
                    state.top_level.push(root.clone());
                    fs::create_dir_all(&root).await
                }
            } else {
                fs::create_dir_all(dest_dir.join(&name)).await
            };
            if let Err(err) = created {
                warn!(%err, %name, "no se pudo crear la carpeta recibida");
            }
            return;
        }

        let part_path = dest_dir.join(format!("{name}.{}.part", offer.transfer_id));
        match fs::File::create(&part_path).await {
            Ok(file) => {
                info!(%name, size = offer.total_size, mime = %offer.mime, "recibiendo archivo");
                self.active.insert(
                    offer.transfer_id,
                    Partial {
                        file,
                        part_path,
                        dest_dir,
                        final_name: name,
                        batch: offer.batch.map(|batch| (batch.id, top_level)),
                        received: 0,
                        total: offer.total_size,
                    },
                );
            }
            Err(err) => warn!(%err, "no se pudo crear el archivo temporal de descarga"),
        }
    }

    pub async fn on_chunk(&mut self, chunk: FileChunk) {
        let Some(partial) = self.active.get_mut(&chunk.transfer_id) else {
            return;
        };
        if let Err(err) = partial.file.write_all(&chunk.data).await {
            warn!(%err, "error escribiendo un trozo — se aborta la transferencia");
            if let Some(partial) = self.active.remove(&chunk.transfer_id) {
                let _ = fs::remove_file(&partial.part_path).await;
            }
            return;
        }
        partial.received += chunk.data.len() as u64;
    }

    /// Cierra y renombra la transferencia `transfer_id`. Devuelve la ruta
    /// final si terminó bien. Los archivos que quedan en la carpeta de
    /// transferencias se loguean como `archivo recibido` para que la GUI los
    /// muestre; los de un lote de portapapeles no (van a la caché y se
    /// anuncian todos juntos al cerrarse el lote).
    pub async fn on_end(&mut self, end: FileEnd) -> Option<PathBuf> {
        let mut partial = self.active.remove(&end.transfer_id)?;
        if let Err(err) = partial.file.flush().await {
            warn!(%err, "no se pudo hacer flush del archivo recibido");
        }
        drop(partial.file);
        if partial.total != 0 && partial.received != partial.total {
            warn!(
                received = partial.received,
                total = partial.total,
                "el archivo recibido no coincide en tamaño con lo anunciado — se conserva igual"
            );
        }
        let final_path = unique_path(&partial.dest_dir, &partial.final_name);
        if let Err(err) = fs::rename(&partial.part_path, &final_path).await {
            warn!(%err, "no se pudo mover el archivo recibido a su nombre final");
            let _ = fs::remove_file(&partial.part_path).await;
            return None;
        }
        let batch = partial
            .batch
            .and_then(|(id, top_level)| self.batches.get_mut(&id).map(|b| (b, top_level)));
        // Lo que queda en la carpeta de transferencias se anuncia de a uno;
        // dentro de un lote, solo las entradas de primer nivel.
        let announce = if let Some((state, top_level)) = batch {
            if top_level {
                state.top_level.push(final_path.clone());
            }
            top_level && state.kind == FileBatchKind::Downloads
        } else {
            true
        };
        if announce {
            info!(path = %final_path.display(), "archivo recibido");
        }
        Some(final_path)
    }

    pub async fn on_abort(&mut self, abort: FileAbort) {
        if let Some(partial) = self.active.remove(&abort.transfer_id) {
            warn!(reason = %abort.reason, "el emisor abortó la transferencia");
            let _ = fs::remove_file(&partial.part_path).await;
        }
    }

    /// Cierra el lote `end.id` y devuelve lo que llegó. `None` si el lote no
    /// existe o llegó vacío (nada que publicar).
    pub async fn on_batch_end(&mut self, end: FileBatchEnd) -> Option<CompletedBatch> {
        let state = self.batches.remove(&end.id)?;
        if state.top_level.is_empty() {
            if state.kind == FileBatchKind::Clipboard {
                let _ = fs::remove_dir_all(&state.dir).await;
            }
            return None;
        }
        match state.kind {
            FileBatchKind::Downloads => {
                // Las carpetas de primer nivel no pasaron por `on_end`: se
                // anuncian acá para que la GUI también las liste.
                for root in state.roots.values() {
                    info!(path = %root.display(), "archivo recibido");
                }
            }
            FileBatchKind::Clipboard => {
                prune_clipboard_cache(&self.clipboard_dir, &state.dir).await;
            }
        }
        Some(CompletedBatch {
            kind: state.kind,
            paths: state.top_level,
        })
    }

    /// Descarta todo lo que quedó a medias — al cerrar la sesión.
    pub async fn abort_all(&mut self) {
        for (_, partial) in self.active.drain() {
            let _ = fs::remove_file(&partial.part_path).await;
        }
        for (_, batch) in self.batches.drain() {
            // Un lote de portapapeles incompleto no sirve para pegar; uno de
            // arrastre conserva lo que sí llegó entero.
            if batch.kind == FileBatchKind::Clipboard {
                let _ = fs::remove_dir_all(&batch.dir).await;
            }
        }
    }
}

/// Borra los lotes viejos de la caché de portapapeles, conservando `current`
/// y los [`CLIPBOARD_CACHE_KEEP`]` - 1` anteriores más recientes. Los nombres
/// de subcarpeta empiezan con la marca de tiempo, así que ordenar por nombre
/// es ordenar por antigüedad.
async fn prune_clipboard_cache(cache: &Path, current: &Path) {
    let Ok(mut entries) = fs::read_dir(cache).await else {
        return;
    };
    let mut older = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path != current {
            older.push(path);
        }
    }
    older.sort();
    let keep = CLIPBOARD_CACHE_KEEP.saturating_sub(1);
    let stale = older.len().saturating_sub(keep);
    for path in older.into_iter().take(stale) {
        let _ = fs::remove_dir_all(&path).await;
    }
}

/// Lee `path` y emite `FileOffer` + `FileChunk`* + `FileEnd` por `sink`,
/// anunciándolo con su propio nombre y sin lote. Ver [`send_entry`].
#[cfg(test)]
async fn send_file(path: &Path, transfer_id: u64, sink: &mpsc::Sender<Message>) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("archivo")
        .to_string();
    send_entry(path, name, transfer_id, None, sink).await;
}

/// Lee `path` y lo emite por `sink` anunciado como `name` (nombre o ruta
/// relativa dentro de un lote). Un error al abrir o leer emite `FileAbort`
/// (o nada, si ni siquiera se pudo anunciar). Si `sink` se cierra (la sesión
/// terminó) se corta sin ruido. Devuelve `false` solo en ese caso, para que
/// quien manda un lote deje de insistir.
async fn send_entry(
    path: &Path,
    name: String,
    transfer_id: u64,
    batch: Option<FileBatch>,
    sink: &mpsc::Sender<Message>,
) -> bool {
    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(err) => {
            // `name` va último y sin comillas en el formato de `tracing`
            // cuando no tiene espacios; la GUI parsea de `name=` al fin de
            // línea, así que soporta nombres con espacios.
            warn!(%err, %name, "no se pudo abrir el archivo a enviar");
            return true;
        }
    };
    let total = file.metadata().await.map_or(0, |m| m.len());
    let mime = mime_from_ext(path);

    if sink
        .send(Message::FileOffer(FileOffer {
            transfer_id,
            name: name.clone(),
            total_size: total,
            mime,
            batch,
        }))
        .await
        .is_err()
    {
        return false;
    }
    info!(size = total, %name, "enviando archivo");

    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        match file.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if sink
                    .send(Message::FileChunk(FileChunk {
                        transfer_id,
                        data: buf[..n].to_vec(),
                    }))
                    .await
                    .is_err()
                {
                    return false;
                }
                // Cederle el turno al resto de tareas: una transferencia
                // grande no debe congelar el reenvío de mouse/teclado.
                tokio::task::yield_now().await;
            }
            Err(err) => {
                warn!(%err, %name, "error leyendo el archivo — se aborta la transferencia");
                return sink
                    .send(Message::FileAbort(FileAbort {
                        transfer_id,
                        reason: err.to_string(),
                    }))
                    .await
                    .is_ok();
            }
        }
    }
    let delivered = sink
        .send(Message::FileEnd(FileEnd { transfer_id }))
        .await
        .is_ok();
    info!(%name, "archivo enviado");
    delivered
}

/// Una entrada (archivo o carpeta) de un lote saliente.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OutgoingEntry {
    path: PathBuf,
    /// Ruta relativa con `/`, tal como viaja en `FileOffer::name`.
    name: String,
    is_dir: bool,
}

/// Aplana `paths` (archivos y/o carpetas) en la lista de entradas a mandar,
/// recorriendo las carpetas en profundidad. Devuelve también el total de
/// bytes. Los enlaces simbólicos se saltean: seguirlos puede sacar la
/// transferencia de lo que el usuario seleccionó, o no terminar nunca.
/// Corta en cuanto se supera `max_entries` — quien llama lo trata como
/// "demasiado grande".
fn collect_entries(paths: &[PathBuf], max_entries: usize) -> (Vec<OutgoingEntry>, u64) {
    let mut entries = Vec::new();
    let mut total = 0u64;
    let mut pending: Vec<(PathBuf, String)> = paths
        .iter()
        .rev()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            Some((path.clone(), name))
        })
        .collect();
    while let Some((path, name)) = pending.pop() {
        if entries.len() > max_entries {
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            entries.push(OutgoingEntry {
                path: path.clone(),
                name: name.clone(),
                is_dir: true,
            });
            let Ok(children) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut children: Vec<_> = children.filter_map(Result::ok).collect();
            children.sort_by_key(std::fs::DirEntry::file_name);
            for child in children.into_iter().rev() {
                if let Some(child_name) = child.file_name().to_str() {
                    pending.push((child.path(), format!("{name}/{child_name}")));
                }
            }
        } else if meta.is_file() {
            total += meta.len();
            entries.push(OutgoingEntry {
                path,
                name,
                is_dir: false,
            });
        }
    }
    (entries, total)
}

/// Manda `paths` (archivos y/o carpetas) por `sink`.
///
/// `kind` es `None` para el envío explícito desde la GUI, sin tope de
/// tamaño. Con `Some(_)` es una transferencia automática (portapapeles o
/// arrastre): se respeta [`AUTO_TRANSFER_MAX_BYTES`] y siempre viaja como
/// lote. Un único archivo suelto sin `kind` viaja sin lote, igual que
/// siempre.
pub async fn send_paths(
    paths: Vec<PathBuf>,
    kind: Option<FileBatchKind>,
    sink: &mpsc::Sender<Message>,
) {
    let max_entries = if kind.is_some() {
        AUTO_TRANSFER_MAX_ENTRIES
    } else {
        usize::MAX
    };
    let collected =
        tokio::task::spawn_blocking(move || collect_entries(&paths, max_entries)).await;
    let Ok((entries, total)) = collected else {
        return;
    };
    if entries.is_empty() {
        return;
    }
    if kind.is_some() && (total > AUTO_TRANSFER_MAX_BYTES || entries.len() > max_entries) {
        warn!(
            bytes = total,
            entries = entries.len(),
            "lo copiado/arrastrado es demasiado grande para pasarlo solo — usá el envío de archivos de la app"
        );
        return;
    }

    let single_file = entries.len() == 1 && !entries[0].is_dir;
    let batch = match kind {
        None if single_file => None,
        None => Some(FileBatchKind::Downloads),
        Some(kind) => Some(kind),
    }
    .map(|kind| FileBatch {
        id: NEXT_BATCH_ID.fetch_add(1, Ordering::Relaxed),
        kind,
    });

    for entry in entries {
        let transfer_id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);
        let delivered = if entry.is_dir {
            sink.send(Message::FileOffer(FileOffer {
                transfer_id,
                name: entry.name,
                total_size: 0,
                mime: MIME_DIRECTORY.to_string(),
                batch,
            }))
            .await
            .is_ok()
        } else {
            send_entry(&entry.path, entry.name, transfer_id, batch, sink).await
        };
        if !delivered {
            return;
        }
    }
    if let Some(batch) = batch {
        let _ = sink
            .send(Message::FileBatchEnd(FileBatchEnd { id: batch.id }))
            .await;
    }
}

/// A dónde van los mensajes de una transferencia saliente, según el rol de
/// este equipo.
#[derive(Clone)]
pub enum FileSink {
    /// Servidor: se difunde a todos los peers conectados.
    Broadcast(Arc<Routing>),
    /// Servidor: a un único peer (el que tiene o acaba de recibir el control).
    Peer(Arc<Routing>, DeviceId),
    /// Cliente: se empuja al bucle de sesión, que lo manda por su única
    /// conexión con el servidor.
    Channel(mpsc::Sender<Message>),
}

/// Escucha el canal IPC local (la GUI de este equipo) y, por cada
/// `FileOffer` que llega —cuyo `name` es una **ruta local absoluta** elegida
/// por la GUI—, lee ese archivo o carpeta y lo manda al otro extremo. Es la
/// mitad "enviar" de la transferencia; la de "recibir" vive en el bucle de
/// sesión de `server`/`client`, que alimenta un [`IncomingFiles`].
///
/// Corre hasta que el proceso termina; un fallo al abrir el socket IPC solo
/// deshabilita el envío desde la GUI (recibir sigue funcionando).
pub async fn serve_ipc_file_sends(token_file: PathBuf, sink: FileSink) {
    let server = match IpcServer::bind(&token_file).await {
        Ok(server) => server,
        Err(err) => {
            warn!(%err, "no se pudo abrir el canal IPC local — la GUI no podrá enviar archivos");
            return;
        }
    };
    info!("canal IPC local listo para envío de archivos");

    loop {
        let mut conn = match server.accept().await {
            Ok(conn) => conn,
            Err(err) => {
                warn!(%err, "fallo aceptando una conexión IPC local");
                continue;
            }
        };
        let sink = sink.clone();
        tokio::spawn(async move {
            while let Ok(Some(message)) = conn.recv().await {
                let Message::FileOffer(offer) = message else {
                    continue;
                };
                let path = PathBuf::from(offer.name);
                let sink = sink.clone();
                tokio::spawn(async move { deliver_paths(vec![path], None, &sink).await });
            }
        });
    }
}

/// Manda archivos/carpetas locales al otro extremo, adaptando
/// [`send_paths`] al `sink` concreto. Un `mpsc` acotado da la contrapresión:
/// si la red no da abasto, la lectura se frena en vez de acumular trozos en
/// memoria.
pub async fn deliver_paths(paths: Vec<PathBuf>, kind: Option<FileBatchKind>, sink: &FileSink) {
    let routed: Box<dyn Fn(Message) + Send> = match sink {
        FileSink::Channel(tx) => return send_paths(paths, kind, tx).await,
        FileSink::Broadcast(routing) => {
            let routing = routing.clone();
            Box::new(move |message| routing.broadcast(&message))
        }
        FileSink::Peer(routing, device) => {
            let (routing, device) = (routing.clone(), *device);
            Box::new(move |message| {
                routing.send_to(device, message);
            })
        }
    };
    let (tx, mut rx) = mpsc::channel::<Message>(8);
    let feeder = tokio::spawn(async move { send_paths(paths, kind, &tx).await });
    while let Some(message) = rx.recv().await {
        routed(message);
    }
    let _ = feeder.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ionconnect-file-test-{tag}-{}-{}",
            std::process::id(),
            fastrand_u32()
        ));
        std::fs::create_dir_all(&dir).expect("crear tmp dir");
        dir
    }

    // Pequeño PRNG para no depender de nada: xorshift sobre el tiempo.
    fn fastrand_u32() -> u32 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let mut x = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            .max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    }

    #[test]
    fn transfer_dir_is_ionconnect_under_the_desktop() {
        let dir = default_transfer_dir();
        assert_eq!(
            dir.file_name().and_then(|s| s.to_str()),
            Some("ionconnect"),
            "el último componente siempre es `ionconnect`"
        );
        // El padre es el escritorio: su nombre varía por SO/idioma, pero
        // nunca debería ser la raíz ni quedar vacío.
        assert!(
            dir.parent().is_some_and(|p| !p.as_os_str().is_empty()),
            "debería colgar de una carpeta de escritorio real"
        );
    }

    /// Camino completo del envío por drag&drop: la "GUI" se conecta al canal
    /// IPC local y manda un `FileOffer` con una ruta local; `serve_ipc_file_sends`
    /// lee ese archivo y lo difunde por `Routing`; se reensambla del otro
    /// lado con `IncomingFiles`. Prueba justo lo que el usuario reportó roto.
    #[tokio::test]
    async fn drag_drop_over_ipc_reaches_the_peer_and_writes_the_file() {
        let workdir = tmp_dir("ipc");
        let token_file = workdir.join("ipc.token");

        // Archivo local que la "GUI" va a soltar.
        let source = workdir.join("documento.txt");
        let payload = b"contenido de prueba para la transferencia".to_vec();
        std::fs::write(&source, &payload).unwrap();

        // Peer falso registrado en Routing.
        let routing = Arc::new(Routing::new());
        let peer = ionconnect_shared::DeviceId::new();
        let (peer_tx, mut peer_rx) = mpsc::unbounded_channel();
        routing.register(peer, peer_tx);

        // Mitad "enviar": el canal IPC local.
        tokio::spawn(serve_ipc_file_sends(
            token_file.clone(),
            FileSink::Broadcast(routing.clone()),
        ));

        // La "GUI" se conecta y suelta el archivo (esperando a que el
        // servidor IPC publique el token).
        let mut gui = None;
        for _ in 0..50 {
            match ionconnect_ipc::IpcClient::connect(&token_file).await {
                Ok(conn) => {
                    gui = Some(conn);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        let mut gui = gui.expect("el canal IPC local debería aceptar la conexión");
        gui.send(Message::FileOffer(FileOffer {
            transfer_id: 0,
            name: source.to_string_lossy().into_owned(),
            total_size: 0,
            mime: String::new(),
            batch: None,
        }))
        .await
        .expect("enviar el FileOffer por IPC");

        // Mitad "recibir": lo que le llega al peer se reensambla.
        let dest = workdir.join("Escritorio").join("ionconnect");
        let mut incoming = IncomingFiles::new(dest.clone());
        let mut got = None;
        loop {
            let msg = tokio::time::timeout(std::time::Duration::from_secs(5), peer_rx.recv())
                .await
                .expect("el peer debería recibir mensajes de transferencia")
                .expect("el canal del peer no debería cerrarse");
            match msg {
                Message::FileOffer(o) => incoming.on_offer(o).await,
                Message::FileChunk(c) => incoming.on_chunk(c).await,
                Message::FileEnd(e) => {
                    got = incoming.on_end(e).await;
                    break;
                }
                Message::FileAbort(a) => panic!("transferencia abortada: {}", a.reason),
                other => panic!("mensaje inesperado: {other:?}"),
            }
        }

        let final_path = got.expect("la transferencia debería completarse");
        assert_eq!(final_path, dest.join("documento.txt"));
        assert_eq!(std::fs::read(&final_path).unwrap(), payload);
        std::fs::remove_dir_all(&workdir).ok();
    }

    #[test]
    fn sanitize_name_keeps_only_the_final_component() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("/tmp/foo/bar.txt"), "bar.txt");
        assert_eq!(sanitize_name(".."), "archivo");
        assert_eq!(sanitize_name(""), "archivo");
        assert_eq!(sanitize_name("normal.png"), "normal.png");
    }

    #[test]
    fn unique_path_does_not_clobber_existing_files() {
        let dir = tmp_dir("unique");
        std::fs::write(dir.join("a.txt"), b"1").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (1).txt"));
        std::fs::write(dir.join("a (1).txt"), b"2").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (2).txt"));
        assert_eq!(unique_path(&dir, "libre.txt"), dir.join("libre.txt"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn receiver_reassembles_a_file_from_chunks() {
        let dir = tmp_dir("recv");
        let mut incoming = IncomingFiles::new(dir.clone());
        incoming
            .on_offer(FileOffer {
                transfer_id: 1,
                name: "saludo.txt".to_string(),
                total_size: 11,
                mime: "text/plain".to_string(),
                batch: None,
            })
            .await;
        incoming
            .on_chunk(FileChunk {
                transfer_id: 1,
                data: b"hola ".to_vec(),
            })
            .await;
        incoming
            .on_chunk(FileChunk {
                transfer_id: 1,
                data: b"mundo".to_vec(),
            })
            .await;
        let path = incoming
            .on_end(FileEnd { transfer_id: 1 })
            .await
            .expect("la transferencia debería completarse");

        assert_eq!(path, dir.join("saludo.txt"));
        assert_eq!(std::fs::read(&path).unwrap(), b"hola mundo");
        // No queda ningún .part.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "part"))
            .collect();
        assert!(leftovers.is_empty(), "no debería quedar ningún .part");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn abort_discards_the_partial_file() {
        let dir = tmp_dir("abort");
        let mut incoming = IncomingFiles::new(dir.clone());
        incoming
            .on_offer(FileOffer {
                transfer_id: 7,
                name: "incompleto.bin".to_string(),
                total_size: 100,
                mime: "application/octet-stream".to_string(),
                batch: None,
            })
            .await;
        incoming
            .on_chunk(FileChunk {
                transfer_id: 7,
                data: vec![0u8; 10],
            })
            .await;
        incoming
            .on_abort(FileAbort {
                transfer_id: 7,
                reason: "cancelado".to_string(),
            })
            .await;
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_none(),
            "el .part debería haberse borrado"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn send_file_emits_offer_then_chunks_then_end() {
        let dir = tmp_dir("send");
        let src = dir.join("origen.dat");
        let payload = vec![0xABu8; CHUNK_SIZE + 1234]; // dos trozos
        std::fs::write(&src, &payload).unwrap();

        let (tx, mut rx) = mpsc::channel(64);
        let sender = tokio::spawn(async move { send_file(&src, 42, &tx).await });

        let mut collected = Vec::new();
        while let Some(msg) = rx.recv().await {
            collected.push(msg);
        }
        sender.await.unwrap();

        match &collected[0] {
            Message::FileOffer(o) => {
                assert_eq!(o.transfer_id, 42);
                assert_eq!(o.name, "origen.dat");
                assert_eq!(o.total_size, payload.len() as u64);
            }
            other => panic!("el primer mensaje debería ser FileOffer, fue {other:?}"),
        }
        assert!(matches!(collected.last(), Some(Message::FileEnd(e)) if e.transfer_id == 42));

        let reassembled: Vec<u8> = collected
            .iter()
            .filter_map(|m| match m {
                Message::FileChunk(c) => Some(c.data.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(reassembled, payload);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitize_relative_never_escapes_the_destination() {
        assert_eq!(sanitize_relative("fotos/2024/a.png"), ["fotos", "2024", "a.png"]);
        assert_eq!(sanitize_relative("../../etc/passwd"), ["etc", "passwd"]);
        assert_eq!(sanitize_relative("C:\\Users\\ana\\doc.txt"), ["Users", "ana", "doc.txt"]);
        assert_eq!(sanitize_relative("/abs/./x"), ["abs", "x"]);
        assert_eq!(sanitize_relative("../.."), ["archivo"]);
    }

    /// Arma `raiz/{a.txt, sub/{b.txt, vacia/}}` y devuelve `raiz`.
    fn sample_tree(base: &Path) -> PathBuf {
        let root = base.join("raiz");
        std::fs::create_dir_all(root.join("sub/vacia")).unwrap();
        std::fs::write(root.join("a.txt"), b"contenido a").unwrap();
        std::fs::write(root.join("sub/b.txt"), b"contenido b").unwrap();
        root
    }

    /// Manda `paths` con `send_paths` y lo reensambla con `incoming`,
    /// devolviendo el lote completado (si lo hubo).
    async fn round_trip(
        paths: Vec<PathBuf>,
        kind: Option<FileBatchKind>,
        incoming: &mut IncomingFiles,
    ) -> Option<CompletedBatch> {
        let (tx, mut rx) = mpsc::channel(64);
        let sender = tokio::spawn(async move { send_paths(paths, kind, &tx).await });
        let mut completed = None;
        while let Some(message) = rx.recv().await {
            match message {
                Message::FileOffer(o) => incoming.on_offer(o).await,
                Message::FileChunk(c) => incoming.on_chunk(c).await,
                Message::FileEnd(e) => {
                    incoming.on_end(e).await;
                }
                Message::FileBatchEnd(e) => completed = incoming.on_batch_end(e).await,
                other => panic!("mensaje inesperado: {other:?}"),
            }
        }
        sender.await.unwrap();
        completed
    }

    #[test]
    fn collect_entries_walks_directories_in_order_and_skips_symlinks() {
        let dir = tmp_dir("collect");
        let root = sample_tree(&dir);
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", root.join("enlace")).unwrap();

        let (entries, total) = collect_entries(std::slice::from_ref(&root), usize::MAX);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["raiz", "raiz/a.txt", "raiz/sub", "raiz/sub/b.txt", "raiz/sub/vacia"]);
        assert_eq!(total, 22);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn dragged_folder_lands_in_downloads_without_clobbering() {
        let dir = tmp_dir("drag");
        let root = sample_tree(&dir.join("origen"));
        let dest = dir.join("destino");
        // Ya hay una `raiz` en destino: la nueva no debe mezclarse con ella.
        std::fs::create_dir_all(dest.join("raiz")).unwrap();
        std::fs::write(dest.join("raiz/previo.txt"), b"x").unwrap();

        let mut incoming = IncomingFiles::with_clipboard_dir(dest.clone(), dir.join("cache"));
        let completed = round_trip(vec![root], Some(FileBatchKind::Downloads), &mut incoming)
            .await
            .expect("el lote debería completarse");

        let landed = dest.join("raiz (1)");
        assert_eq!(completed.kind, FileBatchKind::Downloads);
        assert_eq!(completed.paths, vec![landed.clone()]);
        assert_eq!(std::fs::read(landed.join("a.txt")).unwrap(), b"contenido a");
        assert_eq!(std::fs::read(landed.join("sub/b.txt")).unwrap(), b"contenido b");
        assert!(landed.join("sub/vacia").is_dir(), "las carpetas vacías también viajan");
        assert_eq!(
            std::fs::read_dir(dest.join("raiz")).unwrap().count(),
            1,
            "la carpeta que ya existía queda intacta"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn copied_files_land_in_the_clipboard_cache_and_are_reported_top_level_only() {
        let dir = tmp_dir("clip");
        let root = sample_tree(&dir.join("origen"));
        let loose = dir.join("origen/suelto.txt");
        std::fs::write(&loose, b"suelto").unwrap();
        let downloads = dir.join("destino");
        let cache = dir.join("cache");

        let mut incoming = IncomingFiles::with_clipboard_dir(downloads.clone(), cache.clone());
        let completed = round_trip(
            vec![root, loose],
            Some(FileBatchKind::Clipboard),
            &mut incoming,
        )
        .await
        .expect("el lote debería completarse");

        assert_eq!(completed.kind, FileBatchKind::Clipboard);
        assert_eq!(completed.paths.len(), 2, "solo lo de primer nivel va al portapapeles");
        assert!(completed.paths.iter().all(|p| p.starts_with(&cache)));
        assert_eq!(completed.paths[0].file_name().unwrap(), "raiz");
        assert_eq!(std::fs::read(&completed.paths[1]).unwrap(), b"suelto");
        assert!(completed.paths[0].join("sub/b.txt").is_file());
        assert!(!downloads.exists(), "nada de esto va a la carpeta de transferencias");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn clipboard_cache_keeps_only_the_most_recent_batches() {
        let dir = tmp_dir("prune");
        let file = dir.join("origen.txt");
        std::fs::write(&file, b"x").unwrap();
        let cache = dir.join("cache");
        let mut incoming = IncomingFiles::with_clipboard_dir(dir.join("destino"), cache.clone());

        for _ in 0..4 {
            round_trip(vec![file.clone()], Some(FileBatchKind::Clipboard), &mut incoming).await;
            // El nombre de la subcarpeta lleva la marca de tiempo en ms.
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), CLIPBOARD_CACHE_KEEP);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_single_file_from_the_gui_still_travels_without_a_batch() {
        let dir = tmp_dir("single");
        let file = dir.join("uno.txt");
        std::fs::write(&file, b"1").unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        send_paths(vec![file], None, &tx).await;
        drop(tx);
        let mut saw_batch_end = false;
        while let Some(message) = rx.recv().await {
            match message {
                Message::FileOffer(o) => assert_eq!(o.batch, None),
                Message::FileBatchEnd(_) => saw_batch_end = true,
                _ => {}
            }
        }
        assert!(!saw_batch_end);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn abort_all_drops_an_unfinished_clipboard_batch() {
        let dir = tmp_dir("abortbatch");
        let cache = dir.join("cache");
        let mut incoming = IncomingFiles::with_clipboard_dir(dir.join("destino"), cache.clone());
        incoming
            .on_offer(FileOffer {
                transfer_id: 1,
                name: "a.txt".to_string(),
                total_size: 5,
                mime: String::new(),
                batch: Some(FileBatch { id: 9, kind: FileBatchKind::Clipboard }),
            })
            .await;
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        incoming.abort_all().await;
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
