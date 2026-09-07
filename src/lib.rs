mod bus_parser;
mod state_machine;
mod hotkey;
mod audio;
mod typer;
mod ui;

use std::ffi::{c_char, c_void};
use std::os::raw::c_int;
use std::sync::OnceLock;
use crossbeam_channel::{unbounded, Sender};
use log::info;

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

// 包装裸指针以满足 OnceLock 的 Send + Sync 约束
struct HostApiWrapper(*const mpl_host_api_t);
unsafe impl Send for HostApiWrapper {}
unsafe impl Sync for HostApiWrapper {}

static HOST_API: OnceLock<HostApiWrapper> = OnceLock::new();
static MSG_TX: OnceLock<Sender<bus_parser::WdisMessage>> = OnceLock::new();

#[no_mangle]
pub extern "C" fn micyou_plugin_info() -> mpl_plugin_info_t {
    mpl_plugin_info_t {
        abi_version: 1,
        api_version: 1,
        id: c"opss.Voice-Typing".as_ptr(),
    }
}

#[no_mangle]
pub extern "C" fn micyou_plugin_init(host: *const mpl_host_api_t) -> mpl_result_t {
    let _ = env_logger::try_init();
    let _ = HOST_API.set(HostApiWrapper(host));

    let (msg_tx, msg_rx) = unbounded();
    let _ = MSG_TX.set(msg_tx.clone());

    let host_ref = unsafe { &*host };
    // 读取音频文件到内存
    let start_wav = read_asset_file(host_ref, "assets/start.wav");
    let end_wav = read_asset_file(host_ref, "assets/end.wav");
    let audio_engine = audio::AudioEngine::new(start_wav, end_wav);

    let state_machine = state_machine::TimeWindowStateMachine::new(1500); // 1.5s grace period
    
    // 启动 Typer 线程
    let sm_clone = state_machine.clone();
    std::thread::spawn(move || {
        typer::run_typer_worker(msg_rx, sm_clone);
    });

    // 启动 Hotkey 线程
    let sm_clone2 = state_machine.clone();
    let audio_clone = audio_engine.clone();
    hotkey::start_hotkey_thread(sm_clone2, audio_clone);

    // 启动 UI 线程
    ui::start_ui_thread(state_machine);

    info!("Voice-Typing Plugin Initialized");
    MPL_OK
}

#[no_mangle]
pub extern "C" fn micyou_plugin_deinit() -> mpl_result_t {
    info!("Voice-Typing Plugin Deinitialized");
    MPL_OK
}

#[no_mangle]
pub extern "C" fn micyou_plugin_handle_message(
    _source: *const c_char,
    _topic: *const c_char,
    payload: *const u8,
    len: u32,
) -> mpl_result_t {
    if payload.is_null() || len < 20 {
        return MPL_OK;
    }

    let data = unsafe { std::slice::from_raw_parts(payload, len as usize) };
    
    if let Some(msg) = bus_parser::parse_wdis_payload(data) {
        if let Some(tx) = MSG_TX.get() {
            let _ = tx.send(msg);
        }
    }
    
    MPL_OK
}

fn read_asset_file(host: &mpl_host_api_t, path: &str) -> Vec<u8> {
    let c_path = std::ffi::CString::new(path).unwrap();
    let mut out_size: u32 = 0;
    
    unsafe {
        if let Some(fs_read) = host.fs_read {
            // 第一次调用获取大小
            let res = fs_read(host.ctx, c_path.as_ptr(), std::ptr::null_mut(), &mut out_size);
            if res == MPL_ERR_BUFFER_TOO_SMALL || out_size > 0 {
                let mut buffer = vec![0u8; out_size as usize];
                // 第二次调用读取数据
                fs_read(host.ctx, c_path.as_ptr(), buffer.as_mut_ptr() as *mut c_char, &mut out_size);
                buffer.truncate(out_size as usize);
                return buffer;
            }
        }
    }
    Vec::new()
}