//! MicYou native plugin template (cdylib).
//! Copy include/micyou_plugin_abi.h into your crate or match these structs.

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum mpl_result_t {
    MPL_OK = 0,
    MPL_ERR_NOT_IMPLEMENTED = 1,
    MPL_ERR_INVALID_ARG = 2,
    MPL_ERR_RUNTIME = 3,
    MPL_ERR_BUFFER_TOO_SMALL = 4,
    MPL_ERR_PERMISSION = 5,
}

#[repr(C)]
pub struct mpl_plugin_info_t {
    pub abi_version: u32,
    pub api_version: u32,
    pub id: *const std::ffi::c_char,
    pub version: *const std::ffi::c_char,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct mpl_host_api_t {
    pub log: unsafe extern "C" fn(*mut std::ffi::c_void, i32, *const std::ffi::c_char) -> mpl_result_t,
    pub get_config: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub set_config: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char) -> mpl_result_t,
    pub emit_event: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char) -> mpl_result_t,
    pub send_message: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const u8, u32) -> mpl_result_t,
    pub audio_state: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub connected_devices: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    // 仅追加在 ctx 之后（append-only ABI，勿插入字段）
    pub ctx: *mut std::ffi::c_void,
    pub play_sound: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char) -> mpl_result_t,
    pub plugin_dir: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub register_hotkey: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *mut u64) -> mpl_result_t,
    pub open_window: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char) -> mpl_result_t,
    pub fs_read: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub fs_write: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char) -> mpl_result_t,
    pub set_timeout: unsafe extern "C" fn(*mut std::ffi::c_void, u64, *const std::ffi::c_char, *mut u64) -> mpl_result_t,
    pub clear_timeout: unsafe extern "C" fn(*mut std::ffi::c_void, u64) -> mpl_result_t,
    pub http_request: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char, *const std::ffi::c_char, *const std::ffi::c_char, *mut u64) -> mpl_result_t,
    pub set_interval: unsafe extern "C" fn(*mut std::ffi::c_void, u64, *const std::ffi::c_char, *mut u64) -> mpl_result_t,
    pub clear_interval: unsafe extern "C" fn(*mut std::ffi::c_void, u64) -> mpl_result_t,
    pub open_url: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char) -> mpl_result_t,
    pub notify: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char) -> mpl_result_t,
    pub locale: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub host_info: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub clipboard_read: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_char, *mut u32) -> mpl_result_t,
    pub clipboard_write: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char) -> mpl_result_t,
    pub set_panel_icon: unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_char, *const std::ffi::c_char) -> mpl_result_t,
}

const ID: &str = "opss.Voice-Typing";

/// raw 指针静态：需 Sync 才能放进 static
unsafe impl Sync for mpl_plugin_info_t {}

#[no_mangle]
pub extern "C" fn micyou_plugin_info() -> *const mpl_plugin_info_t {
    static INFO: mpl_plugin_info_t = mpl_plugin_info_t {
        abi_version: 1,
        api_version: 1,
        id: b"opss.Voice-Typing\0".as_ptr() as *const std::ffi::c_char,
        version: b"1.0.0\0".as_ptr() as *const std::ffi::c_char,
    };
    &INFO
}

#[no_mangle]
pub extern "C" fn micyou_plugin_init(host: *const mpl_host_api_t) -> mpl_result_t {
    unsafe {
        let host = &*host;
        let msg = std::ffi::CString::new(format!("{ID} initialized")).unwrap();
        ((*host).log)(host.ctx, 2, msg.as_ptr());
    }
    mpl_result_t::MPL_OK
}

#[no_mangle]
pub extern "C" fn micyou_plugin_deinit() {}

#[no_mangle]
pub extern "C" fn micyou_plugin_process(
    data: *mut f32,
    samples: u32,
    channels: u32,
    queued_ms: f64,
) -> mpl_result_t {
    unsafe {
        // TODO: real-time-safe DSP here. Never call host APIs from process().
        let _ = (data, samples, channels, queued_ms);
    }
    mpl_result_t::MPL_OK
}

#[no_mangle]
pub extern "C" fn micyou_plugin_handle_message(
    source: *const std::ffi::c_char,
    topic: *const std::ffi::c_char,
    payload: *const u8,
    payload_len: u32,
) -> mpl_result_t {
    let _ = (source, topic, payload, payload_len);
    mpl_result_t::MPL_OK
}
