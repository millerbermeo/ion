//! Publica una lista de archivos en el portapapeles X11 (`CLIPBOARD`) en
//! todos los formatos que esperan los exploradores de archivos de Linux.
//!
//! `arboard` solo ofrece `text/uri-list`. Con eso alcanza para Dolphin y la
//! mayoría de las apps, pero Nautilus (GNOME) reconoce un "copiar archivos"
//! por `x-special/gnome-copied-files` — sin ese formato su Pegar queda
//! deshabilitado. Como `arboard` no expone formatos arbitrarios, acá se toma
//! la propiedad de la selección directamente con `x11rb` y se sirven los
//! formatos a mano.
//!
//! La selección en X11 vive mientras viva su dueño, así que cada publicación
//! arranca un hilo que atiende los pedidos hasta que otro cliente (o una
//! publicación nuestra posterior, o `arboard` al escribir texto) le quita la
//! propiedad.

use std::path::PathBuf;

use x11rb::connection::Connection as _;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SelectionNotifyEvent, SelectionRequestEvent, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::error::ClipboardError;
use ionconnect_shared::uri::path_to_file_uri;

fn backend(err: impl std::fmt::Display) -> ClipboardError {
    ClipboardError::Backend(err.to_string())
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        UTF8_STRING,
        URI_LIST: b"text/uri-list",
        GNOME_COPIED_FILES: b"x-special/gnome-copied-files",
        TEXT_PLAIN_UTF8: b"text/plain;charset=utf-8",
        TEXT_PLAIN: b"text/plain",
    }
}

/// Contenido ya serializado para cada familia de formatos.
struct Payload {
    uri_list: Vec<u8>,
    gnome: Vec<u8>,
    text: Vec<u8>,
}

impl Payload {
    fn new(files: &[PathBuf]) -> Self {
        let uris: Vec<String> = files.iter().map(|p| path_to_file_uri(p)).collect();
        // RFC 2483: una URI por línea terminada en CRLF.
        let uri_list = uris.iter().fold(String::new(), |mut acc, uri| {
            acc.push_str(uri);
            acc.push_str("\r\n");
            acc
        });
        // Formato de Nautilus/Nemo/Caja: la acción y después las URIs,
        // separadas por `\n`, sin salto final.
        let gnome = format!("copy\n{}", uris.join("\n"));
        // Quien pegue en un editor de texto recibe las rutas, igual que al
        // copiar archivos en el explorador local.
        let text = files
            .iter()
            .map(|p| p.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            uri_list: uri_list.into_bytes(),
            gnome: gnome.into_bytes(),
            text: text.into_bytes(),
        }
    }
}

/// Toma la propiedad de `CLIPBOARD` y deja un hilo sirviendo `files`.
/// Devuelve en cuanto la selección ya es nuestra — o un error si no hay
/// servidor X o no se la pudo tomar, para que quien llama caiga a `arboard`.
pub(crate) fn publish(files: &[PathBuf]) -> Result<(), ClipboardError> {
    let (conn, screen_num) = x11rb::connect(None).map_err(backend)?;
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().map_err(backend)?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(backend)?;
    let atoms = Atoms::new(&conn)
        .map_err(backend)?
        .reply()
        .map_err(backend)?;

    conn.set_selection_owner(window, atoms.CLIPBOARD, x11rb::CURRENT_TIME)
        .map_err(backend)?;
    let owner = conn
        .get_selection_owner(atoms.CLIPBOARD)
        .map_err(backend)?
        .reply()
        .map_err(backend)?
        .owner;
    if owner != window {
        return Err(ClipboardError::Backend(
            "no se pudo tomar la propiedad del portapapeles X11".to_string(),
        ));
    }

    let payload = Payload::new(files);
    std::thread::Builder::new()
        .name("ionconnect-clipboard-files".to_string())
        .spawn(move || serve(&conn, window, atoms, &payload))
        .map_err(backend)?;
    Ok(())
}

/// Atiende `SelectionRequest` hasta perder la propiedad de la selección.
fn serve(conn: &RustConnection, window: u32, atoms: Atoms, payload: &Payload) {
    loop {
        let Ok(event) = conn.wait_for_event() else {
            return;
        };
        match event {
            Event::SelectionRequest(request) => {
                if answer(conn, atoms, payload, &request).is_err() {
                    return;
                }
            }
            // Otro cliente copió algo: ya no somos el portapapeles.
            Event::SelectionClear(clear) if clear.owner == window => {
                let _ = conn.destroy_window(window);
                let _ = conn.flush();
                return;
            }
            _ => {}
        }
    }
}

fn answer(
    conn: &RustConnection,
    atoms: Atoms,
    payload: &Payload,
    request: &SelectionRequestEvent,
) -> Result<(), ClipboardError> {
    // ICCCM: un cliente viejo puede mandar `property = None`; en ese caso
    // se usa el propio `target` como propiedad.
    let property = if request.property == x11rb::NONE {
        request.target
    } else {
        request.property
    };
    let target: Atom = request.target;
    let text_targets = [
        atoms.UTF8_STRING,
        atoms.TEXT_PLAIN_UTF8,
        atoms.TEXT_PLAIN,
        Atom::from(AtomEnum::STRING),
    ];

    let written = if target == atoms.TARGETS {
        let mut targets = vec![atoms.TARGETS, atoms.URI_LIST, atoms.GNOME_COPIED_FILES];
        targets.extend_from_slice(&text_targets);
        conn.change_property32(
            PropMode::REPLACE,
            request.requestor,
            property,
            AtomEnum::ATOM,
            &targets,
        )
        .map_err(backend)?;
        true
    } else {
        let data = if target == atoms.URI_LIST {
            Some(&payload.uri_list)
        } else if target == atoms.GNOME_COPIED_FILES {
            Some(&payload.gnome)
        } else if text_targets.contains(&target) {
            Some(&payload.text)
        } else {
            None
        };
        match data {
            Some(data) => {
                conn.change_property8(PropMode::REPLACE, request.requestor, property, target, data)
                    .map_err(backend)?;
                true
            }
            None => false,
        }
    };

    // `property = None` en la respuesta le dice al solicitante que ese
    // formato no está disponible.
    let notify = SelectionNotifyEvent {
        response_type: x11rb::protocol::xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: request.time,
        requestor: request.requestor,
        selection: request.selection,
        target,
        property: if written { property } else { x11rb::NONE },
    };
    conn.send_event(false, request.requestor, EventMask::NO_EVENT, notify)
        .map_err(backend)?;
    conn.flush().map_err(backend)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_serializes_every_format() {
        let payload = Payload::new(&[
            PathBuf::from("/home/ana/informe final.pdf"),
            PathBuf::from("/tmp/ñandú.txt"),
        ]);
        assert_eq!(
            String::from_utf8(payload.uri_list).unwrap(),
            "file:///home/ana/informe%20final.pdf\r\nfile:///tmp/%C3%B1and%C3%BA.txt\r\n"
        );
        assert_eq!(
            String::from_utf8(payload.gnome).unwrap(),
            "copy\nfile:///home/ana/informe%20final.pdf\nfile:///tmp/%C3%B1and%C3%BA.txt"
        );
        assert_eq!(
            String::from_utf8(payload.text).unwrap(),
            "/home/ana/informe final.pdf\n/tmp/ñandú.txt"
        );
    }
}
