//! Transferencia de archivos entre equipos por el canal TCP+TLS confiable.
//!
//! Un archivo se manda como `FileOffer` → `FileChunk`* → `FileEnd` (o
//! `FileAbort` si la lectura falla a mitad). El receptor va anexando cada
//! trozo a `<nombre>.<id>.part` dentro de su carpeta de descargas y, al
//! recibir `FileEnd`, lo mueve a su nombre definitivo (evitando pisar uno ya
//! existente). No hay control de flujo propio: el `mpsc` acotado por el que
//! salen los mensajes ya frena al emisor si la red no da abasto, y TCP
//! garantiza orden y entrega.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ionconnect_ipc::IpcServer;
use ionconnect_protocol::{FileAbort, FileChunk, FileEnd, FileOffer, Message};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::routing::Routing;

/// `transfer_id` global de este proceso — solo tiene que ser único mientras
/// una transferencia está viva, así que un contador que nunca se reinicia
/// sobra.
static NEXT_TRANSFER_ID: AtomicU64 = AtomicU64::new(1);

/// Tamaño de cada `FileChunk`. Bien por debajo del límite de frame del códec
/// (1 `MiB`) para dejar lugar a la cabecera `postcard`, y chico como para no
/// monopolizar la conexión frente al mouse/teclado.
pub const CHUNK_SIZE: usize = 128 * 1024;

/// `~/Downloads/ionconnect` — donde caen los archivos recibidos.
#[must_use]
pub fn default_download_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join("Downloads").join("ionconnect")
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

/// Deja solo el nombre de archivo (sin ruta, sin `..`, sin separadores) para
/// que un emisor no pueda hacer que el receptor escriba fuera de su carpeta
/// de descargas.
fn sanitize_name(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("archivo");
    let cleaned: String = base
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "archivo".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Primera ruta libre en `dir` para `name`: `foto.png`, luego
/// `foto (1).png`, `foto (2).png`, ...
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let direct = dir.join(name);
    if !direct.exists() {
        return direct;
    }
    let as_path = Path::new(name);
    let stem = as_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name);
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
    final_name: String,
    received: u64,
    total: u64,
}

/// Estado del lado receptor: las transferencias entrantes en curso.
pub struct IncomingFiles {
    dir: PathBuf,
    active: HashMap<u64, Partial>,
}

impl IncomingFiles {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            active: HashMap::new(),
        }
    }

    pub async fn on_offer(&mut self, offer: FileOffer) {
        if let Err(err) = fs::create_dir_all(&self.dir).await {
            warn!(%err, dir = %self.dir.display(), "no se pudo crear la carpeta de descargas");
            return;
        }
        let name = sanitize_name(&offer.name);
        let part_path = self.dir.join(format!("{name}.{}.part", offer.transfer_id));
        match fs::File::create(&part_path).await {
            Ok(file) => {
                info!(%name, size = offer.total_size, mime = %offer.mime, "recibiendo archivo");
                self.active.insert(
                    offer.transfer_id,
                    Partial {
                        file,
                        part_path,
                        final_name: name,
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
    /// final si terminó bien — `core` la loguea como `archivo recibido` para
    /// que la GUI la muestre.
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
        let final_path = unique_path(&self.dir, &partial.final_name);
        match fs::rename(&partial.part_path, &final_path).await {
            Ok(()) => {
                info!(path = %final_path.display(), "archivo recibido");
                Some(final_path)
            }
            Err(err) => {
                warn!(%err, "no se pudo mover el archivo recibido a su nombre final");
                let _ = fs::remove_file(&partial.part_path).await;
                None
            }
        }
    }

    pub async fn on_abort(&mut self, abort: FileAbort) {
        if let Some(partial) = self.active.remove(&abort.transfer_id) {
            warn!(reason = %abort.reason, "el emisor abortó la transferencia");
            let _ = fs::remove_file(&partial.part_path).await;
        }
    }

    /// Descarta todos los `.part` a medias — al cerrar la sesión.
    pub async fn abort_all(&mut self) {
        for (_, partial) in self.active.drain() {
            let _ = fs::remove_file(&partial.part_path).await;
        }
    }
}

/// Lee `path` y emite `FileOffer` + `FileChunk`* + `FileEnd` por `sink`. Un
/// error al abrir o leer emite `FileAbort` (o nada, si ni siquiera se pudo
/// anunciar). `transfer_id` lo elige quien llama — un contador por conexión
/// alcanza. Si `sink` se cierra (la sesión terminó) se corta sin ruido.
pub async fn send_file(path: &Path, transfer_id: u64, sink: &mpsc::Sender<Message>) {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("archivo")
        .to_string();

    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(err) => {
            warn!(%err, path = %path.display(), "no se pudo abrir el archivo a enviar");
            return;
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
        }))
        .await
        .is_err()
    {
        return;
    }
    info!(%name, size = total, "enviando archivo");

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
                    return;
                }
                // Cederle el turno al resto de tareas: una transferencia
                // grande no debe congelar el reenvío de mouse/teclado.
                tokio::task::yield_now().await;
            }
            Err(err) => {
                warn!(%err, "error leyendo el archivo — se aborta la transferencia");
                let _ = sink
                    .send(Message::FileAbort(FileAbort {
                        transfer_id,
                        reason: err.to_string(),
                    }))
                    .await;
                return;
            }
        }
    }
    let _ = sink.send(Message::FileEnd(FileEnd { transfer_id })).await;
    info!(%name, "archivo enviado");
}

/// A dónde van los mensajes de una transferencia saliente, según el rol de
/// este equipo.
#[derive(Clone)]
pub enum FileSink {
    /// Servidor: se difunde a todos los peers conectados.
    Broadcast(Arc<Routing>),
    /// Cliente: se empuja al bucle de sesión, que lo manda por su única
    /// conexión con el servidor.
    Channel(mpsc::Sender<Message>),
}

/// Escucha el canal IPC local (la GUI de este equipo) y, por cada
/// `FileOffer` que llega —cuyo `name` es una **ruta local absoluta** elegida
/// por la GUI—, lee ese archivo y lo manda al otro extremo. Es la mitad
/// "enviar" de la transferencia; la de "recibir" vive en el bucle de sesión
/// de `server`/`client`, que alimenta un [`IncomingFiles`].
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
                let transfer_id = NEXT_TRANSFER_ID.fetch_add(1, Ordering::Relaxed);
                let sink = sink.clone();
                tokio::spawn(async move { deliver_file(&path, transfer_id, &sink).await });
            }
        });
    }
}

/// Manda un archivo local al otro extremo, adaptando `send_file` al `sink`
/// concreto. Un `mpsc` acotado da la contrapresión: si la red no da abasto,
/// `send_file` se frena en vez de acumular trozos en memoria.
async fn deliver_file(path: &Path, transfer_id: u64, sink: &FileSink) {
    match sink {
        FileSink::Broadcast(routing) => {
            let (tx, mut rx) = mpsc::channel::<Message>(8);
            let path = path.to_path_buf();
            let feeder = tokio::spawn(async move { send_file(&path, transfer_id, &tx).await });
            while let Some(message) = rx.recv().await {
                routing.broadcast(&message);
            }
            let _ = feeder.await;
        }
        FileSink::Channel(tx) => send_file(path, transfer_id, tx).await,
    }
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
}
