use crate::resampler::CaptureResampler;
use ringbuf::{
    traits::{Producer, Split},
    HeapRb,
};
use std::ffi::{c_char, c_void};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc, Arc, Mutex, OnceLock,
};
use std::thread::{self, JoinHandle};
use tracing::{info, warn};

const COINIT_MULTITHREADED: u32 = 0x0;
const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_OBJECT_0: u32 = 0;

const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
const E_POINTER: i32 = 0x8000_4003_u32 as i32;

const VT_BLOB: u16 = 65;
const AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK: i32 = 1;
const PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE: i32 = 1;

const AUDCLNT_SHAREMODE_SHARED: i32 = 0;
const AUDCLNT_STREAMFLAGS_LOOPBACK: u32 = 0x0002_0000;
const AUDCLNT_STREAMFLAGS_EVENTCALLBACK: u32 = 0x0004_0000;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_IUNKNOWN: Guid = Guid {
    data1: 0x0000_0000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IAGILEOBJECT: Guid = Guid {
    data1: 0x94ea_2b94,
    data2: 0xe9cc,
    data3: 0x49e0,
    data4: [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90],
};

const IID_IAUDIOCLIENT: Guid = Guid {
    data1: 0x1cb9_ad4c,
    data2: 0xdbfa,
    data3: 0x4c32,
    data4: [0xb1, 0x78, 0xc2, 0xf5, 0x68, 0xa7, 0x03, 0xb2],
};

const IID_IAUDIOCAPTURECLIENT: Guid = Guid {
    data1: 0xc8ad_bd64,
    data2: 0xe71e,
    data3: 0x48a0,
    data4: [0xa4, 0xde, 0x18, 0x5c, 0x39, 0x5c, 0xd3, 0x17],
};

const IID_IACTIVATEAUDIOINTERFACECOMPLETIONHANDLER: Guid = Guid {
    data1: 0x41d9_49ab,
    data2: 0x9862,
    data3: 0x444a,
    data4: [0x80, 0xf6, 0xc2, 0x61, 0x33, 0x4d, 0xa5, 0xeb],
};

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioClientProcessLoopbackParams {
    target_process_id: u32,
    process_loopback_mode: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioClientActivationParams {
    activation_type: i32,
    process_loopback_params: AudioClientProcessLoopbackParams,
}

#[repr(C)]
struct PropVariantBlob {
    vt: u16,
    w_reserved1: u16,
    w_reserved2: u16,
    w_reserved3: u16,
    cb_size: u32,
    p_blob_data: *mut u8,
}

#[repr(C, packed(1))]
struct WaveFormatEx {
    w_format_tag: u16,
    n_channels: u16,
    n_samples_per_sec: u32,
    n_avg_bytes_per_sec: u32,
    n_block_align: u16,
    w_bits_per_sample: u16,
    cb_size: u16,
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
    fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const c_char) -> *mut c_void;
    fn GetCurrentProcessId() -> u32;
    fn CreateEventW(
        lp_event_attributes: *const c_void,
        b_manual_reset: i32,
        b_initial_state: i32,
        lp_name: *const u16,
    ) -> *mut c_void;
    fn SetEvent(h_event: *mut c_void) -> i32;
    fn WaitForSingleObject(h_handle: *mut c_void, dw_milliseconds: u32) -> u32;
    fn WaitForMultipleObjects(
        n_count: u32,
        lp_handles: *const *mut c_void,
        b_wait_all: i32,
        dw_milliseconds: u32,
    ) -> u32;
    fn CloseHandle(h_object: *mut c_void) -> i32;
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pv_reserved: *const c_void, dw_co_init: u32) -> i32;
    fn CoUninitialize();
}

type ActivateAudioInterfaceAsyncFn = unsafe extern "system" fn(
    device_interface_path: *const u16,
    riid: *const Guid,
    activation_params: *const PropVariantBlob,
    completion_handler: *mut c_void,
    activation_operation: *mut *mut c_void,
) -> i32;

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct AsyncOpVtbl {
    base: UnknownVtbl,
    get_activate_result: unsafe extern "system" fn(*mut c_void, *mut i32, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct CompletionHandlerVtbl {
    base: UnknownVtbl,
    activate_completed: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
}

#[repr(C)]
struct AudioClientVtbl {
    base: UnknownVtbl,
    initialize: unsafe extern "system" fn(
        *mut c_void,
        i32,
        u32,
        i64,
        i64,
        *const WaveFormatEx,
        *const Guid,
    ) -> i32,
    get_buffer_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    get_stream_latency: unsafe extern "system" fn(*mut c_void, *mut i64) -> i32,
    get_current_padding: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
    is_format_supported: unsafe extern "system" fn(
        *mut c_void,
        i32,
        *const WaveFormatEx,
        *mut *mut WaveFormatEx,
    ) -> i32,
    get_mix_format: unsafe extern "system" fn(*mut c_void, *mut *mut WaveFormatEx) -> i32,
    get_device_period: unsafe extern "system" fn(*mut c_void, *mut i64, *mut i64) -> i32,
    start: unsafe extern "system" fn(*mut c_void) -> i32,
    stop: unsafe extern "system" fn(*mut c_void) -> i32,
    reset: unsafe extern "system" fn(*mut c_void) -> i32,
    set_event_handle: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_service: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct AudioCaptureClientVtbl {
    base: UnknownVtbl,
    get_buffer: unsafe extern "system" fn(
        *mut c_void,
        *mut *mut u8,
        *mut u32,
        *mut u32,
        *mut u64,
        *mut u64,
    ) -> i32,
    release_buffer: unsafe extern "system" fn(*mut c_void, u32) -> i32,
    get_next_packet_size: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
}

#[inline]
unsafe fn com_vtbl<T>(obj: *mut c_void) -> &'static T {
    &*(*(obj as *const *const T))
}

#[inline]
unsafe fn com_release(obj: *mut c_void) {
    if !obj.is_null() {
        (com_vtbl::<UnknownVtbl>(obj).release)(obj);
    }
}

struct ActivationState {
    ready_event: usize,
    result: Mutex<Option<(i32, usize)>>,
}

impl Drop for ActivationState {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.result.lock() {
            if let Some((_, ptr_addr)) = guard.take() {
                if ptr_addr != 0 {
                    unsafe {
                        com_release(ptr_addr as *mut c_void);
                    }
                }
            }
        }
        if self.ready_event != 0 {
            unsafe {
                CloseHandle(self.ready_event as *mut c_void);
            }
            self.ready_event = 0;
        }
    }
}

#[repr(C)]
struct CompletionHandler {
    vtbl: *const CompletionHandlerVtbl,
    ref_count: AtomicU32,
    state: Arc<ActivationState>,
}

static COMPLETION_HANDLER_VTBL: CompletionHandlerVtbl = CompletionHandlerVtbl {
    base: UnknownVtbl {
        query_interface: handler_query_interface,
        add_ref: handler_add_ref,
        release: handler_release,
    },
    activate_completed: handler_activate_completed,
};

unsafe extern "system" fn handler_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    ppv: *mut *mut c_void,
) -> i32 {
    if ppv.is_null() || riid.is_null() {
        return E_POINTER;
    }
    let id = *riid;
    if id == IID_IUNKNOWN
        || id == IID_IAGILEOBJECT
        || id == IID_IACTIVATEAUDIOINTERFACECOMPLETIONHANDLER
    {
        *ppv = this;
        handler_add_ref(this);
        S_OK
    } else {
        *ppv = std::ptr::null_mut();
        E_NOINTERFACE
    }
}

unsafe extern "system" fn handler_add_ref(this: *mut c_void) -> u32 {
    let me = &*(this as *const CompletionHandler);
    me.ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn handler_release(this: *mut c_void) -> u32 {
    let me = &*(this as *const CompletionHandler);
    let prev = me.ref_count.fetch_sub(1, Ordering::Release);
    if prev == 1 {
        std::sync::atomic::fence(Ordering::Acquire);
        drop(Box::from_raw(this as *mut CompletionHandler));
        0
    } else {
        prev - 1
    }
}

unsafe extern "system" fn handler_activate_completed(
    this: *mut c_void,
    operation: *mut c_void,
) -> i32 {
    let me = &*(this as *const CompletionHandler);
    let mut final_hr: i32 = -1;
    let mut audio_client: *mut c_void = std::ptr::null_mut();
    if !operation.is_null() {
        let mut hr: i32 = -1;
        let mut raw_iface: *mut c_void = std::ptr::null_mut();
        let call_hr = (com_vtbl::<AsyncOpVtbl>(operation).get_activate_result)(
            operation,
            &mut hr,
            &mut raw_iface,
        );
        if call_hr >= 0 && hr >= 0 && !raw_iface.is_null() {
            let mut client: *mut c_void = std::ptr::null_mut();
            let qi_hr = (com_vtbl::<UnknownVtbl>(raw_iface).query_interface)(
                raw_iface,
                &IID_IAUDIOCLIENT,
                &mut client,
            );
            com_release(raw_iface);
            if qi_hr >= 0 && !client.is_null() {
                final_hr = hr;
                audio_client = client;
            } else {
                final_hr = if qi_hr < 0 { qi_hr } else { -1 };
            }
        } else {
            if !raw_iface.is_null() {
                com_release(raw_iface);
            }
            final_hr = if call_hr < 0 { call_hr } else { hr };
        }
    }
    if let Ok(mut guard) = me.state.result.lock() {
        if let Some((_, old_addr)) = guard.replace((final_hr, audio_client as usize)) {
            if old_addr != 0 {
                com_release(old_addr as *mut c_void);
            }
        }
    } else if !audio_client.is_null() {
        com_release(audio_client);
    }
    if me.state.ready_event != 0 {
        SetEvent(me.state.ready_event as *mut c_void);
    }
    S_OK
}

fn mmdevapi_activate_fn() -> Result<ActivateAudioInterfaceAsyncFn, String> {
    static PROC: OnceLock<Result<usize, String>> = OnceLock::new();
    let res = PROC.get_or_init(|| unsafe {
        let mmdevapi_name: Vec<u16> = "mmdevapi.dll\0".encode_utf16().collect();
        let mmdevapi = LoadLibraryW(mmdevapi_name.as_ptr());
        if mmdevapi.is_null() {
            return Err("LoadLibraryW(mmdevapi.dll) failed".into());
        }
        let proc_ptr = GetProcAddress(mmdevapi, c"ActivateAudioInterfaceAsync".as_ptr());
        if proc_ptr.is_null() {
            return Err("Missing ActivateAudioInterfaceAsync".into());
        }
        Ok(proc_ptr as usize)
    });
    match res {
        Ok(addr) => Ok(unsafe {
            std::mem::transmute::<*mut c_void, ActivateAudioInterfaceAsyncFn>(*addr as *mut c_void)
        }),
        Err(msg) => Err(msg.clone()),
    }
}

pub(crate) struct WinProcessLoopback {
    stop_event: *mut c_void,
    thread: Option<JoinHandle<()>>,
}

unsafe impl Send for WinProcessLoopback {}

impl Drop for WinProcessLoopback {
    fn drop(&mut self) {
        unsafe {
            if !self.stop_event.is_null() {
                SetEvent(self.stop_event);
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        unsafe {
            if !self.stop_event.is_null() {
                CloseHandle(self.stop_event);
                self.stop_event = std::ptr::null_mut();
            }
        }
    }
}

pub(crate) struct BuiltWinProcessLoopback {
    pub handle: WinProcessLoopback,
    pub cons: <HeapRb<f32> as Split>::Cons,
    pub resampler: CaptureResampler,
    pub failed: Arc<AtomicBool>,
}

pub(crate) fn start_windows_process_loopback() -> Option<BuiltWinProcessLoopback> {
    let stop_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if stop_event.is_null() {
        return None;
    }

    let rb = HeapRb::<f32>::new(48_000 / 2);
    let (prod, cons) = rb.split();
    let failed = Arc::new(AtomicBool::new(false));
    let failed_worker = Arc::clone(&failed);
    let stop_addr = stop_event as usize;

    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);

    let thread = match thread::Builder::new()
        .name("squad-win-proc-loopback".into())
        .spawn(move || {
            run_process_loopback_thread(stop_addr as *mut c_void, prod, failed_worker, ready_tx);
        }) {
        Ok(h) => h,
        Err(err) => {
            warn!("Failed to spawn Windows process loopback thread: {err}");
            unsafe {
                CloseHandle(stop_event);
            }
            return None;
        }
    };

    match ready_rx.recv_timeout(std::time::Duration::from_secs(4)) {
        Ok(Ok(())) => Some(BuiltWinProcessLoopback {
            handle: WinProcessLoopback {
                stop_event,
                thread: Some(thread),
            },
            cons,
            resampler: CaptureResampler::new(48_000),
            failed,
        }),
        Ok(Err(reason)) => {
            warn!("Windows Process Loopback unavailable ({reason}); falling back to WASAPI");
            unsafe {
                SetEvent(stop_event);
            }
            let _ = thread.join();
            unsafe {
                CloseHandle(stop_event);
            }
            None
        }
        Err(_) => {
            warn!("Windows Process Loopback initialization timed out; falling back to WASAPI");
            unsafe {
                SetEvent(stop_event);
            }
            let _ = thread.join();
            unsafe {
                CloseHandle(stop_event);
            }
            None
        }
    }
}

fn run_process_loopback_thread(
    stop_event: *mut c_void,
    mut prod: <HeapRb<f32> as Split>::Prod,
    failed: Arc<AtomicBool>,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
) {
    unsafe {
        let co_hr = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED);
        let need_co_uninit = co_hr >= 0;

        let activate_async = match mmdevapi_activate_fn() {
            Ok(f) => f,
            Err(err) => {
                let _ = ready_tx.send(Err(err));
                if need_co_uninit {
                    CoUninitialize();
                }
                return;
            }
        };

        let ready_event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
        if ready_event.is_null() {
            let _ = ready_tx.send(Err("CreateEventW(ready) failed".into()));
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let state = Arc::new(ActivationState {
            ready_event: ready_event as usize,
            result: Mutex::new(None),
        });
        let handler_ptr = Box::into_raw(Box::new(CompletionHandler {
            vtbl: &COMPLETION_HANDLER_VTBL,
            ref_count: AtomicU32::new(1),
            state: Arc::clone(&state),
        }));

        let mut act_params = AudioClientActivationParams {
            activation_type: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            process_loopback_params: AudioClientProcessLoopbackParams {
                target_process_id: GetCurrentProcessId(),
                process_loopback_mode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            },
        };
        let prop_var = PropVariantBlob {
            vt: VT_BLOB,
            w_reserved1: 0,
            w_reserved2: 0,
            w_reserved3: 0,
            cb_size: std::mem::size_of::<AudioClientActivationParams>() as u32,
            p_blob_data: &mut act_params as *mut _ as *mut u8,
        };
        let dev_path: Vec<u16> = "VAD\\Process_Loopback\0".encode_utf16().collect();
        let mut async_op: *mut c_void = std::ptr::null_mut();

        let hr = activate_async(
            dev_path.as_ptr(),
            &IID_IAUDIOCLIENT,
            &prop_var,
            handler_ptr as *mut c_void,
            &mut async_op,
        );
        if hr < 0 {
            let _ = ready_tx.send(Err(format!(
                "ActivateAudioInterfaceAsync failed: 0x{:08X}",
                hr as u32
            )));
            com_release(async_op);
            handler_release(handler_ptr as *mut c_void);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let wait_res = WaitForSingleObject(ready_event, 3000);
        com_release(async_op);
        handler_release(handler_ptr as *mut c_void);
        if wait_res != WAIT_OBJECT_0 {
            let _ = ready_tx.send(Err("ActivateAudioInterfaceAsync timed out".into()));
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let (activate_hr, audio_client_addr) = match state.result.lock() {
            Ok(mut guard) => guard.take().unwrap_or((-1, 0)),
            Err(_) => (-1, 0),
        };
        let audio_client = audio_client_addr as *mut c_void;

        if activate_hr < 0 || audio_client.is_null() {
            let _ = ready_tx.send(Err(format!(
                "Process loopback activation result 0x{:08X}",
                activate_hr as u32
            )));
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let wfex = WaveFormatEx {
            w_format_tag: WAVE_FORMAT_IEEE_FLOAT,
            n_channels: 2,
            n_samples_per_sec: 48_000,
            n_avg_bytes_per_sec: 384_000,
            n_block_align: 8,
            w_bits_per_sample: 32,
            cb_size: 0,
        };
        let init_hr = (com_vtbl::<AudioClientVtbl>(audio_client).initialize)(
            audio_client,
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            200_000,
            0,
            &wfex,
            std::ptr::null(),
        );
        if init_hr < 0 {
            let _ = ready_tx.send(Err(format!(
                "IAudioClient::Initialize failed: 0x{:08X}",
                init_hr as u32
            )));
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let audio_event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
        if audio_event.is_null() {
            let _ = ready_tx.send(Err("CreateEventW(audio) failed".into()));
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let ev_hr =
            (com_vtbl::<AudioClientVtbl>(audio_client).set_event_handle)(audio_client, audio_event);
        if ev_hr < 0 {
            let _ = ready_tx.send(Err(format!(
                "IAudioClient::SetEventHandle failed: 0x{:08X}",
                ev_hr as u32
            )));
            CloseHandle(audio_event);
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let mut capture_client: *mut c_void = std::ptr::null_mut();
        let svc_hr = (com_vtbl::<AudioClientVtbl>(audio_client).get_service)(
            audio_client,
            &IID_IAUDIOCAPTURECLIENT,
            &mut capture_client,
        );
        if svc_hr < 0 || capture_client.is_null() {
            let _ = ready_tx.send(Err(format!(
                "IAudioClient::GetService(IAudioCaptureClient) failed: 0x{:08X}",
                svc_hr as u32
            )));
            CloseHandle(audio_event);
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        let start_hr = (com_vtbl::<AudioClientVtbl>(audio_client).start)(audio_client);
        if start_hr < 0 {
            let _ = ready_tx.send(Err(format!(
                "IAudioClient::Start failed: 0x{:08X}",
                start_hr as u32
            )));
            com_release(capture_client);
            CloseHandle(audio_event);
            com_release(audio_client);
            if need_co_uninit {
                CoUninitialize();
            }
            return;
        }

        info!("Windows Process Loopback active (48000 Hz/2ch, excludes own PID)");
        let _ = ready_tx.send(Ok(()));

        let cap_vtbl = com_vtbl::<AudioCaptureClientVtbl>(capture_client);
        let handles = [stop_event, audio_event];
        'capture: loop {
            let w = WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE);
            if w == WAIT_OBJECT_0 {
                break;
            }
            if w != WAIT_OBJECT_0 + 1 {
                failed.store(true, Ordering::Relaxed);
                break;
            }

            loop {
                let mut packet_frames: u32 = 0;
                let hr = (cap_vtbl.get_next_packet_size)(capture_client, &mut packet_frames);
                if hr < 0 {
                    failed.store(true, Ordering::Relaxed);
                    break 'capture;
                }
                if packet_frames == 0 {
                    break;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut frames_read: u32 = 0;
                let mut flags: u32 = 0;
                let hr = (cap_vtbl.get_buffer)(
                    capture_client,
                    &mut data_ptr,
                    &mut frames_read,
                    &mut flags,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if hr < 0 {
                    failed.store(true, Ordering::Relaxed);
                    break 'capture;
                }

                if frames_read > 0
                    && (flags & AUDCLNT_BUFFERFLAGS_SILENT) == 0
                    && !data_ptr.is_null()
                {
                    let samples = std::slice::from_raw_parts(
                        data_ptr as *const f32,
                        (frames_read as usize) * 2,
                    );
                    for pair in samples.as_chunks::<2>().0 {
                        let _ = prod.try_push((pair[0] + pair[1]) * 0.5);
                    }
                }

                let hr = (cap_vtbl.release_buffer)(capture_client, frames_read);
                if hr < 0 {
                    failed.store(true, Ordering::Relaxed);
                    break 'capture;
                }
            }
        }

        let _ = (com_vtbl::<AudioClientVtbl>(audio_client).stop)(audio_client);
        com_release(capture_client);
        CloseHandle(audio_event);
        com_release(audio_client);
        if need_co_uninit {
            CoUninitialize();
        }
    }
}
