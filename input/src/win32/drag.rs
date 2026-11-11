//! Detecta un arrastre de archivos en curso (OLE drag-and-drop) y se lo
//! "roba" a la aplicación de origen: averigua qué archivos lleva el usuario
//! y cancela el arrastre local, para que `core` los mande al otro equipo.
//! Es el equivalente Windows de `x11::drag`, con la misma idea.
//!
//! Windows no tiene forma de preguntarle a otro proceso qué está
//! arrastrando. Lo único que ve esos datos es un **destino de soltado** bajo
//! el cursor, así que se fabrica uno: una ventana diminuta y prácticamente
//! transparente, registrada con `RegisterDragDrop`, justo donde está el
//! puntero. Se mueve el cursor un píxel para que el bucle de arrastre del
//! origen (`DoDragDrop`) vuelva a mirar qué hay debajo, y OLE nos llama a
//! `IDropTarget::DragEnter` con el `IDataObject` del arrastre. De ahí se lee
//! `CF_HDROP` — la lista de archivos — y se rechaza el soltado.
//!
//! Con la lista en mano se inyecta Escape: `DoDragDrop` lo traduce en
//! `QueryContinueDrag(fEscapePressed = TRUE)` y toda fuente estándar
//! (incluido el Explorador) responde cancelando.
//!
//! **Sin verificar en ejecución**: igual que el resto de `win32`, este
//! módulo se compila de forma cruzada desde Linux pero nunca se ejercitó en
//! una máquina Windows. Ante cualquier falla devuelve "no hay arrastre", que
//! deja el comportamiento exactamente como estaba antes de existir.

use std::cell::RefCell;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize,
    OleUninitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VK_ESCAPE,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, LWA_ALPHA,
    MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes,
    ShowWindow, TranslateMessage, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Ref, implement, w};

/// Lado de la ventana sonda, en píxeles. Alcanza con que contenga al cursor
/// durante el vaivén de un píxel.
const PROBE_SIZE: i32 = 9;

/// Cuánto se espera a que OLE nos entregue el arrastre. Un origen con un
/// arrastre activo responde en su siguiente mensaje de movimiento; este
/// plazo es el costo de un cruce con el botón apretado que no era un
/// arrastre.
const ENTER_TIMEOUT: Duration = Duration::from_millis(200);

/// Cuánto se sigue atendiendo mensajes después del Escape, para que el
/// origen termine de cancelar (y nos mande `DragLeave`) antes de destruir la
/// ventana.
const CANCEL_GRACE: Duration = Duration::from_millis(120);

/// Si hay un arrastre de archivos en curso en este equipo, devuelve los
/// archivos y lo cancela. Vacío si no hay ninguno o si algo falla.
///
/// Bloquea a lo sumo unos cientos de milisegundos. Corre en un hilo propio
/// porque OLE exige un apartamento STA con bucle de mensajes, y no se puede
/// imponerle eso al hilo de quien llama.
#[must_use]
pub fn take_dragged_files() -> Vec<PathBuf> {
    std::thread::spawn(probe).join().unwrap_or_default()
}

/// Destino de soltado que solo mira qué se está arrastrando y lo rechaza.
#[implement(IDropTarget)]
struct DropProbe {
    /// `Some` en cuanto OLE nos presentó un arrastre (aunque no lleve
    /// archivos): es la señal para dejar de esperar.
    seen: Rc<RefCell<Option<Vec<PathBuf>>>>,
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for DropProbe_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let files = pdataobj.as_ref().map(dropped_files).unwrap_or_default();
        *self.seen.borrow_mut() = Some(files);
        reject(pdweffect);
        Ok(())
    }

    fn DragOver(
        &self,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        reject(pdweffect);
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        _pdataobj: Ref<'_, IDataObject>,
        _grfkeystate: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // Si el botón se soltara sobre la sonda antes de que llegue la
        // cancelación, tampoco se acepta nada: el archivo no se mueve.
        reject(pdweffect);
        Ok(())
    }
}

/// Le dice al origen que acá no se puede soltar.
fn reject(pdweffect: *mut DROPEFFECT) {
    if !pdweffect.is_null() {
        // SAFETY: OLE garantiza que `pdweffect` apunta a un `DROPEFFECT`
        // válido durante la llamada; se comprobó que no es nulo.
        unsafe { *pdweffect = DROPEFFECT_NONE };
    }
}

/// Rutas de `CF_HDROP` dentro de `data`; vacío si el arrastre no lleva
/// archivos.
fn dropped_files(data: &IDataObject) -> Vec<PathBuf> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        #[allow(clippy::cast_sign_loss)]
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: `format` vive durante la llamada; el `STGMEDIUM` devuelto es
    // nuestro y se libera con `ReleaseStgMedium` más abajo.
    let Ok(mut medium) = (unsafe { data.GetData(&raw const format) }) else {
        return Vec::new();
    };
    // SAFETY: se pidió `TYMED_HGLOBAL`, así que el miembro activo de la
    // unión es `hGlobal`, y para `CF_HDROP` ese handle es un `HDROP`.
    let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);

    let mut files = Vec::new();
    // SAFETY: `hdrop` es válido hasta `ReleaseStgMedium`. `u32::MAX` como
    // índice devuelve la cantidad de archivos.
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    for index in 0..count {
        // SAFETY: ídem; sin buffer devuelve el largo sin el nulo final.
        let len = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
        let mut buffer = vec![0u16; len + 1];
        // SAFETY: `buffer` tiene lugar para `len` caracteres más el nulo.
        let written = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) } as usize;
        if written > 0 {
            files.push(PathBuf::from(std::ffi::OsString::from_wide(
                &buffer[..written.min(len)],
            )));
        }
    }
    // SAFETY: `medium` vino de `GetData` y no se usa después de esto.
    unsafe { ReleaseStgMedium(&raw mut medium) };
    files
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: reenvío directo de los argumentos que entregó el sistema.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn probe() -> Vec<PathBuf> {
    // SAFETY: inicializa OLE (STA) en este hilo recién creado; se equilibra
    // con `OleUninitialize` al final.
    if unsafe { OleInitialize(None) }.is_err() {
        return Vec::new();
    }
    let files = probe_in_apartment().unwrap_or_default();
    // SAFETY: pareja del `OleInitialize` de arriba, mismo hilo.
    unsafe { OleUninitialize() };
    files
}

/// El cuerpo de la sonda, con OLE ya inicializado en este hilo.
fn probe_in_apartment() -> Option<Vec<PathBuf>> {
    let class_name = w!("IonConnectDragProbe");
    // SAFETY: llamadas Win32 de ventana estándar; todos los punteros que se
    // pasan viven durante la llamada y la ventana se destruye antes de
    // salir.
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        // Devuelve 0 si la clase ya estaba registrada por una sonda
        // anterior: no es un error.
        RegisterClassW(&raw const class);

        let mut cursor = POINT::default();
        GetCursorPos(&raw mut cursor).ok()?;
        let hwnd = CreateWindowExW(
            // Encima de todo, fuera de la barra de tareas y sin robar el
            // foco. `LAYERED` permite hacerla casi invisible sin que deje
            // de recibir el arrastre (alfa 0 la volvería "transparente a
            // clics" y OLE la saltearía).
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class_name,
            w!(""),
            WS_POPUP,
            cursor.x - PROBE_SIZE / 2,
            cursor.y - PROBE_SIZE / 2,
            PROBE_SIZE,
            PROBE_SIZE,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 1, LWA_ALPHA);

        let seen = Rc::new(RefCell::new(None));
        let target: IDropTarget = DropProbe { seen: seen.clone() }.into();
        if RegisterDragDrop(hwnd, &target).is_err() {
            let _ = DestroyWindow(hwnd);
            return None;
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // Vaivén de un píxel: `DoDragDrop` solo vuelve a buscar el destino
        // bajo el cursor cuando el mouse se mueve.
        nudge_cursor();
        pump_messages(ENTER_TIMEOUT, || seen.borrow().is_some());

        let files = seen.borrow_mut().take().unwrap_or_default();
        if !files.is_empty() {
            press_escape();
            pump_messages(CANCEL_GRACE, || false);
        }

        let _ = RevokeDragDrop(hwnd);
        let _ = DestroyWindow(hwnd);
        Some(files)
    }
}

/// Atiende la cola de mensajes de este hilo —por donde OLE entrega las
/// llamadas a nuestro `IDropTarget`— hasta que `done` sea cierto o venza
/// `timeout`.
fn pump_messages(timeout: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    let mut message = MSG::default();
    loop {
        // SAFETY: `message` es un `MSG` válido y propio de este hilo.
        unsafe {
            while PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
        if done() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn send(inputs: &[INPUT]) {
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let size = size_of::<INPUT>() as i32;
    // SAFETY: `inputs` es un slice válido de `INPUT` bien formados.
    unsafe { SendInput(inputs, size) };
}

fn nudge_cursor() {
    let step = |dx: i32, dy: i32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&[step(-1, -1), step(1, 1), step(1, 1), step(-1, -1)]);
}

fn press_escape() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_ESCAPE,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    send(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)]);
}
