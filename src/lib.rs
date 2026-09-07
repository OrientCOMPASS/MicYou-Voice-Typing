mod bus_parser;
mod state_machine;
mod hotkey;
mod audio;
mod typer;
mod ui;

use std::ffi::{c_char, c_void};
use std::os::raw::c_int;
use std::sync::OnceLock;
use std::path::Path;
use crossbeam_channel::{unbounded, Sender};

#[allow(non_camel_case_types)]
pub type mpl_result_t = c_int;
pub const MPL_OK: mpl_result_t = 0;
pub const MPL_ERR_BUFFER_TOO_SMALL: mpl_result_t = 12;

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct mpl_host_api_t {
    pub log: Option<unsafe extern "C" fn(ctx: *mut c_void, level: c_int, msg: *const c_char)>,
    pub get_config: Option<unsafe extern "C" fn(ctx: *mut c_void, key: *const c_char, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub set_config: Option<unsafe extern "C" fn(ctx: *mut c_void, key: *const c_char, json_value: *const c_char) -> mpl_result_t>,
    pub emit_event: Option<unsafe extern "C" fn(ctx: *mut c_void, topic: *const c_char, json_payload: *const c_char) -> mpl_result_t>,
    pub send_message: Option<unsafe extern "C" fn(ctx: *mut c_void, target_json: *const c_char, payload: *const u8, payload_len: u32) -> mpl_result_t>,
    pub audio_state: Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub connected_devices: Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub ctx: *mut c_void,
    pub play_sound: Option<unsafe extern "C" fn(ctx: *mut c_void, path: *const c_char) -> mpl_result_t>,
    pub plugin_dir: Option<unsafe extern "C" fn(ctx: *mut c_void, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
    pub register_hotkey: Option<unsafe extern "C" fn(ctx: *mut c_void, shortcut: *const c_char, out_id: *mut u64) -> mpl_result_t>,
    pub open_window: Option<unsafe extern "C" fn(ctx: *mut c_void, panel_id: *const c_char) -> mpl_result_t>,
    pub fs_read: Option<unsafe extern "C" fn(ctx: *mut c_void, path: *const c_char, out: *mut c_char, out_size: *mut u32) -> mpl_result_t>,
}

#[allow(non_camel_case_types)]
#[repr(C)]
pub struct mpl_plugin_info_t {
    pub abi_version: u32,
    pub api_version: u32,
    pub id: *const c_char,
}

#[allow(dead_code)]
struct HostApiWrapper(*const mpl_host_api_t);
unsafe impl Send for HostApiWrapper {}
unsafe impl Sync for HostApiWrapper {}

struct PluginInfoWrapper(mpl_plugin_info_t);
unsafe impl Sync for PluginInfoWrapper {}

static HOST_API: OnceLock<HostApiWrapper> = OnceLock::new();
static MSG_TX: OnceLock<Sender<bus_parser::WdisMessage>> = OnceLock::new();

static PLUGIN_INFO: PluginInfoWrapper = PluginInfoWrapper(mpl_plugin_info_t {
    abi_version: 1,
    api_version: 1,
    id: c"opss.voice-typing".as_ptr(),
});

// 简化后的 panic 捕获宏，防止 Panic 跨越 FFI 边界
macro_rules! catch_panic {
    ($($body:tt)*) => {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            $($body)*
        })).unwrap_or_else(|_| -1)
    };
}

#[no_mangle]
pub extern "C" fn micyou_plugin_info() -> *const mpl_plugin_info_t {
    &PLUGIN_INFO.0
}

#[no_mangle]
pub extern "C" fn micyou_plugin_init(host: *const mpl_host_api_t) -> mpl_result_t {
    catch_panic! {
        if host.is_null() { return -1; }
        
        let _ = HOST_API.set(HostApiWrapper(host));
        let (msg_tx, msg_rx) = unbounded();
        let _ = MSG_TX.set(msg_tx.clone());

        let host_ref = unsafe { &*host };
        
        let plugin_dir = get_plugin_dir(host_ref);
        let start_path = Path::new(&plugin_dir).join("assets/start.wav");
        let end_path = Path::new(&plugin_dir).join("assets/end.wav");
        
        let start_wav = std::fs::read(&start_path).unwrap_or_default();
        let end_wav = std::fs::read(&end_path).unwrap_or_default();

        let audio_engine = audio::AudioEngine::new(start_wav, end_wav);
        let state_machine = state_machine::TimeWindowStateMachine::new(1500); 
        
        let mode_str = get_config_string(host_ref, "mode").unwrap_or_else(|| "toggle".to_string());

        let sm_clone = state_machine.clone();
        std::thread::spawn(move || { typer::run_typer_worker(msg_rx, sm_clone); });

        let sm_clone2 = state_machine.clone();
        let audio_clone = audio_engine.clone();
        hotkey::start_hotkey_thread(sm_clone2, audio_clone, mode_str);

        ui::start_ui_thread(state_machine);
        MPL_OK
    }
}

#[no_mangle]
pub extern "C" fn micyou_plugin_deinit() -> mpl_result_t { MPL_OK }

#[no_mangle]
pub extern "C" fn micyou_plugin_handle_message(
    _source: *const c_char, _topic: *const c_char, payload: *const u8, len: u32,
) -> mpl_result_t {
    if payload.is_null() || len < 20 { return MPL_OK; }
    let data = unsafe { std::slice::from_raw_parts(payload, len as usize) };
    if let Some(msg) = bus_parser::parse_wdis_payload(data) {
        if let Some(tx) = MSG_TX.get() { let _ = tx.send(msg); }
    }
    MPL_OK
}

fn get_plugin_dir(host: &mpl_host_api_t) -> String {
    let mut out_size: u32 = 0;
    let mut dummy = [0u8; 1];
    unsafe {
        if let Some(plugin_dir) = host.plugin_dir {
            plugin_dir(host.ctx, dummy.as_mut_ptr() as *mut c_char, &mut out_size);
            if out_size > 0 {
                let mut buffer = vec![0u8; out_size as usize + 1]; 
                let res2 = plugin_dir(host.ctx, buffer.as_mut_ptr() as *mut c_char, &mut out_size);
                if res2 == MPL_OK {
                    buffer.truncate(out_size as usize);
                    return String::from_utf8_lossy(&buffer).to_string();
                }
            }
        }
    }
    String::new()
}

fn get_config_string(host: &mpl_host_api_t, key: &str) -> Option<String> {
    let c_key = std::ffi::CString::new(key).unwrap();
    let mut out_size: u32 = 0;
    let mut dummy = [0u8; 1];
    unsafe {
        if let Some(get_config) = host.get_config {
            get_config(host.ctx, c_key.as_ptr(), dummy.as_mut_ptr() as *mut c_char, &mut out_size);
            if out_size > 0 {
                let mut buffer = vec![0u8; out_size as usize + 1];
                let res = get_config(host.ctx, c_key.as_ptr(), buffer.as_mut_ptr() as *mut c_char, &mut out_size);
                if res == MPL_OK {
                    buffer.truncate(out_size as usize);
                    return Some(String::from_utf8_lossy(&buffer).trim_matches('"').to_string());
                }
            }
        }
    }
    None
}