use crate::net::DecodedVideoFrame;
use jni::objects::{JClass, JObject};
use jni::JNIEnv;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Notify;

#[allow(dead_code)]
fn rgba_8888_format() -> i32 {
    i32::from(ndk::hardware_buffer_format::HardwareBufferFormat::R8G8B8A8_UNORM)
}

#[derive(Default)]
struct SurfaceState {
    window: Option<NonNull<ndk_sys::ANativeWindow>>,
    configured_width: u32,
    configured_height: u32,
}

unsafe impl Send for SurfaceState {}

static SURFACE_STATE: OnceLock<Mutex<SurfaceState>> = OnceLock::new();
static SURFACE_GEN: AtomicU64 = AtomicU64::new(0);
static VIDEO_NOTIFY_HOOK: OnceLock<Mutex<Option<Arc<Notify>>>> = OnceLock::new();

fn surface_state() -> &'static Mutex<SurfaceState> {
    SURFACE_STATE.get_or_init(|| Mutex::new(SurfaceState::default()))
}

fn video_notify_hook() -> &'static Mutex<Option<Arc<Notify>>> {
    VIDEO_NOTIFY_HOOK.get_or_init(|| Mutex::new(None))
}

#[no_mangle]
pub extern "system" fn Java_app_squadair_mobile_core_SquadNativeBridge_nativeAttachSurface(
    env: JNIEnv,
    _class: JClass,
    surface: JObject,
) {
    #[cfg(target_os = "android")]
    {
        let raw_win = if surface.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe {
                ndk_sys::ANativeWindow_fromSurface(
                    env.get_native_interface().cast(),
                    surface.as_raw().cast(),
                )
            }
        };
        let mut guard = surface_state().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(prev) = guard.window.take() {
            unsafe { ndk_sys::ANativeWindow_release(prev.as_ptr()) };
        }
        guard.window = NonNull::new(raw_win);
        guard.configured_width = 0;
        guard.configured_height = 0;
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (env, surface);
    }

    SURFACE_GEN.fetch_add(1, Ordering::AcqRel);
    if let Ok(guard) = video_notify_hook().lock() {
        if let Some(ref notify) = *guard {
            notify.notify_one();
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_app_squadair_mobile_core_SquadNativeBridge_nativeDetachSurface(
    _env: JNIEnv,
    _class: JClass,
) {
    let mut guard = surface_state().lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(target_os = "android")]
    if let Some(prev) = guard.window.take() {
        unsafe { ndk_sys::ANativeWindow_release(prev.as_ptr()) };
    }
    #[cfg(not(target_os = "android"))]
    {
        guard.window = None;
    }
    guard.configured_width = 0;
    guard.configured_height = 0;
    SURFACE_GEN.fetch_add(1, Ordering::AcqRel);
}

pub fn spawn_android_surface_render_loop(
    rt_handle: tokio::runtime::Handle,
    video_frame_slot: Arc<Mutex<Option<DecodedVideoFrame>>>,
    video_notify: Arc<Notify>,
    running: Arc<AtomicBool>,
) {
    if let Ok(mut guard) = video_notify_hook().lock() {
        *guard = Some(Arc::clone(&video_notify));
    }
    let _ = std::thread::Builder::new()
        .name("squad-android-video-render".to_string())
        .spawn(move || {
            let mut last_seq = 0u64;
            let mut last_gen = 0u64;
            while running.load(Ordering::Acquire) {
                rt_handle.block_on(video_notify.notified());
                if !running.load(Ordering::Acquire) {
                    break;
                }
                let frame_opt = video_frame_slot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let Some(frame) = frame_opt else {
                    continue;
                };
                let cur_gen = SURFACE_GEN.load(Ordering::Acquire);
                if frame.frame_seq == last_seq && cur_gen == last_gen {
                    continue;
                }
                render_frame_to_window(&frame);
                last_seq = frame.frame_seq;
                last_gen = cur_gen;
            }
        });
}

fn render_frame_to_window(frame: &DecodedVideoFrame) {
    if frame.width == 0 || frame.height == 0 {
        return;
    }
    let Some(expected_len) = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .and_then(|px| px.checked_mul(4))
    else {
        return;
    };
    if frame.rgba.len() < expected_len {
        return;
    }

    #[cfg(target_os = "android")]
    {
        let mut guard = surface_state().lock().unwrap_or_else(|e| e.into_inner());
        let Some(win_ptr) = guard.window else {
            return;
        };

        if guard.configured_width != frame.width || guard.configured_height != frame.height {
            let rc = unsafe {
                ndk_sys::ANativeWindow_setBuffersGeometry(
                    win_ptr.as_ptr(),
                    frame.width as i32,
                    frame.height as i32,
                    rgba_8888_format(),
                )
            };
            if rc == 0 {
                guard.configured_width = frame.width;
                guard.configured_height = frame.height;
            }
        }

        let mut out_buffer: ndk_sys::ANativeWindow_Buffer = unsafe { std::mem::zeroed() };
        let lock_rc = unsafe {
            ndk_sys::ANativeWindow_lock(win_ptr.as_ptr(), &mut out_buffer, std::ptr::null_mut())
        };
        if lock_rc != 0 {
            return;
        }

        if !out_buffer.bits.is_null()
            && out_buffer.width > 0
            && out_buffer.height > 0
            && out_buffer.stride >= out_buffer.width
        {
            let copy_w = (frame.width as usize).min(out_buffer.width as usize);
            let copy_h = (frame.height as usize).min(out_buffer.height as usize);
            let src_stride_bytes = frame.width as usize * 4;
            let copy_row_bytes = copy_w * 4;
            let dst_stride_bytes = out_buffer.stride as usize * 4;
            let dst_base = out_buffer.bits as *mut u8;

            unsafe {
                if out_buffer.stride as u32 == frame.width
                    && copy_w == frame.width as usize
                    && copy_h == frame.height as usize
                {
                    std::ptr::copy_nonoverlapping(
                        frame.rgba.as_ptr(),
                        dst_base,
                        src_stride_bytes * copy_h,
                    );
                } else {
                    for row in 0..copy_h {
                        std::ptr::copy_nonoverlapping(
                            frame.rgba.as_ptr().add(row * src_stride_bytes),
                            dst_base.add(row * dst_stride_bytes),
                            copy_row_bytes,
                        );
                    }
                }
            }
        }

        unsafe {
            ndk_sys::ANativeWindow_unlockAndPost(win_ptr.as_ptr());
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = frame;
    }
}
