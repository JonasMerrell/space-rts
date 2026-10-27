//! Browser build (wasm32): the JS side lives in web/hohmann.js (a miniquad plugin). WebSocket to
//! the server that served the page, localStorage, and notifications while the tab is hidden.
use crate::net::NetEvent;
use proto::{decode, encode, ClientMsg, ServerMsg};
use std::cell::{Cell, RefCell};

#[link(wasm_import_module = "env")]
extern "C" {
    fn hr_ws_open(ptr: *const u8, len: usize) -> i32;
    fn hr_ws_state(id: i32) -> i32;
    fn hr_ws_send(id: i32, ptr: *const u8, len: usize);
    fn hr_ws_recv_len(id: i32) -> i32;
    fn hr_ws_close(id: i32);
    fn hr_take(dst: *mut u8);
    fn hr_store_get_len(ptr: *const u8, len: usize) -> i32;
    fn hr_store_set(kp: *const u8, kl: usize, vp: *const u8, vl: usize);
    fn hr_store_remove(ptr: *const u8, len: usize);
    fn hr_page_host_len() -> i32;
    fn hr_notify(on: i32);
    fn hr_loaded();
    fn hr_text_fields(rects: *const f32, n: usize, focused: i32);
    fn hr_paste();
}

/// The JS plugin checks this against its own version.
#[no_mangle]
pub extern "C" fn hohmann_crate_version() -> u32 {
    1
}

/// Fetch a string the JS side just staged (`len` < 0 = none).
fn staged(len: i32) -> Option<String> {
    let len = usize::try_from(len).ok()?;
    let mut buf = vec![0u8; len];
    unsafe { hr_take(buf.as_mut_ptr()) };
    String::from_utf8(buf).ok()
}

pub fn store_get(key: &str) -> Option<String> {
    staged(unsafe { hr_store_get_len(key.as_ptr(), key.len()) })
}

pub fn store_set(key: &str, val: &str) {
    unsafe { hr_store_set(key.as_ptr(), key.len(), val.as_ptr(), val.len()) }
}

pub fn store_remove(key: &str) {
    unsafe { hr_store_remove(key.as_ptr(), key.len()) }
}

/// host[:port] of the page, i.e. the game server that served it.
pub fn page_host() -> String {
    staged(unsafe { hr_page_host_len() }).unwrap_or_default()
}

/// Show browser notifications for game events while the tab is hidden (asks permission on the next tap).
pub fn notifications(on: bool) {
    unsafe { hr_notify(on as i32) }
}

/// Assets are in: hide the page's loading message.
pub fn loaded() {
    unsafe { hr_loaded() }
}

/// Tell the page where the game's text fields are (CSS px) and whether one is focused: a tap on one
/// focuses a hidden HTML input so phones open their keyboard, whose typing comes back as key events.
pub fn text_fields(ui: &mut ui::Ui) {
    // PASTE from a field's menu: the page reads the system clipboard and calls `hohmann_paste`
    if std::mem::take(&mut ui.input.paste_request) {
        unsafe { hr_paste() }
    }
    if let Some(text) = PASTED.with(|p| p.borrow_mut().take()) {
        ui.input.inject_paste(text);
    }
    // macroquad's screen coordinates are already CSS px with high_dpi (the canvas is dpi× larger)
    let (fields, focused) = ui.input.text_fields();
    let flat: Vec<f32> = fields.iter().flat_map(|(_, r)| [r.x, r.y, r.w, r.h]).collect();
    let focused = focused.and_then(|f| fields.iter().position(|(id, _)| *id == f)).map_or(-1, |i| i as i32);
    unsafe { hr_text_fields(flat.as_ptr(), fields.len(), focused) }
}

thread_local! {
    static PASTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Called by the page with the system clipboard's text (a buffer from miniquad's `allocate_vec_u8`).
#[no_mangle]
pub extern "C" fn hohmann_paste(ptr: *mut u8, len: usize) {
    let bytes = unsafe { Vec::from_raw_parts(ptr, len, len) };
    if let Ok(text) = String::from_utf8(bytes) {
        PASTED.with(|p| *p.borrow_mut() = Some(text));
    }
}

/// WebSocket connection with the same interface as the native TLS one.
pub struct Net {
    id: i32,
    hello: RefCell<Option<ClientMsg>>,
    /// 0 connecting, 1 open, 2 closed (as last reported)
    state: Cell<i32>,
}

impl Net {
    /// Connect to the page's own server; `hello` is sent first once connected. The address is
    /// ignored: a page can only talk to the server it came from.
    pub fn connect(_addr: String, hello: ClientMsg) -> Net {
        let id = unsafe { hr_ws_open(std::ptr::null(), 0) };
        Net { id, hello: RefCell::new(Some(hello)), state: Cell::new(0) }
    }

    fn send_raw(&self, m: &ClientMsg) {
        let line = encode(m);
        unsafe { hr_ws_send(self.id, line.as_ptr(), line.len()) }
    }

    pub fn send(&self, m: ClientMsg) {
        self.send_raw(&m);
    }

    pub fn poll(&self) -> Vec<NetEvent> {
        let mut out = vec![];
        let st = unsafe { hr_ws_state(self.id) };
        if st == 1 && self.state.get() == 0 {
            out.push(NetEvent::Connected);
            if let Some(h) = self.hello.borrow_mut().take() {
                self.send_raw(&h);
            }
        }
        while let Some(line) = staged(unsafe { hr_ws_recv_len(self.id) }) {
            match decode::<ServerMsg>(&line) {
                Ok(m) => out.push(NetEvent::Msg(m)),
                Err(e) => {
                    macroquad::logging::warn!("bad server message: {e}");
                }
            }
        }
        if st == 2 && self.state.get() != 2 {
            out.push(if self.state.get() == 0 { NetEvent::Failed(format!("can't reach the game server at {}", page_host())) } else { NetEvent::Closed });
        }
        self.state.set(st);
        out
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        unsafe { hr_ws_close(self.id) }
    }
}
