use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapRb,
};
use std::ffi::{c_char, c_void};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tracing::{info, warn};

const RTLD_LAZY: i32 = 0x00001;
const RTLD_LOCAL: i32 = 0;

const AAUDIO_OK: i32 = 0;
const AAUDIO_UNSPECIFIED: i32 = 0;
const AAUDIO_DIRECTION_OUTPUT: i32 = 0;
const AAUDIO_DIRECTION_INPUT: i32 = 1;
const AAUDIO_FORMAT_PCM_I16: i32 = 1;
const AAUDIO_FORMAT_PCM_FLOAT: i32 = 2;
const AAUDIO_SHARING_MODE_SHARED: i32 = 1;
const AAUDIO_PERFORMANCE_MODE_NONE: i32 = 10;
const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
const AAUDIO_USAGE_VOICE_COMMUNICATION: i32 = 2;
const AAUDIO_CONTENT_TYPE_SPEECH: i32 = 1;
const AAUDIO_INPUT_PRESET_VOICE_COMMUNICATION: i32 = 7;
const AAUDIO_CALLBACK_RESULT_CONTINUE: i32 = 0;

#[repr(C)]
struct AAudioStream {
    _private: [u8; 0],
}

#[repr(C)]
struct AAudioStreamBuilder {
    _private: [u8; 0],
}

type DataCallbackFn = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallbackFn = unsafe extern "C" fn(*mut AAudioStream, *mut c_void, i32);

extern "C" {
    fn dlopen(filename: *const c_char, flag: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

struct AAudioApi {
    create_stream_builder: unsafe extern "C" fn(*mut *mut AAudioStreamBuilder) -> i32,
    set_direction: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_device_id: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_channel_count: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_format: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_sharing_mode: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_performance_mode: unsafe extern "C" fn(*mut AAudioStreamBuilder, i32),
    set_data_callback: unsafe extern "C" fn(*mut AAudioStreamBuilder, DataCallbackFn, *mut c_void),
    set_error_callback:
        unsafe extern "C" fn(*mut AAudioStreamBuilder, ErrorCallbackFn, *mut c_void),
    open_stream: unsafe extern "C" fn(*mut AAudioStreamBuilder, *mut *mut AAudioStream) -> i32,
    delete_builder: unsafe extern "C" fn(*mut AAudioStreamBuilder) -> i32,
    request_start: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    request_stop: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    close_stream: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    get_sample_rate: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    get_channel_count: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    get_format: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    get_device_id: unsafe extern "C" fn(*mut AAudioStream) -> i32,
    set_usage: Option<unsafe extern "C" fn(*mut AAudioStreamBuilder, i32)>,
    set_content_type: Option<unsafe extern "C" fn(*mut AAudioStreamBuilder, i32)>,
    set_input_preset: Option<unsafe extern "C" fn(*mut AAudioStreamBuilder, i32)>,
}

unsafe impl Send for AAudioApi {}
unsafe impl Sync for AAudioApi {}

static AAUDIO_API: OnceLock<Option<AAudioApi>> = OnceLock::new();

unsafe fn load_sym<T>(handle: *mut c_void, name: &std::ffi::CStr) -> Option<T> {
    let ptr = dlsym(handle, name.as_ptr());
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute_copy(&ptr))
    }
}

fn get_aaudio_api() -> Option<&'static AAudioApi> {
    AAUDIO_API
        .get_or_init(|| unsafe {
            let handle = dlopen(c"libaaudio.so".as_ptr(), RTLD_LAZY | RTLD_LOCAL);
            if handle.is_null() {
                warn!("dlopen(libaaudio.so) failed; falling back to cpal");
                return None;
            }
            let Some(create_stream_builder) = load_sym(handle, c"AAudio_createStreamBuilder")
            else {
                warn!("Missing AAudio_createStreamBuilder in libaaudio.so");
                return None;
            };
            let set_direction = load_sym(handle, c"AAudioStreamBuilder_setDirection")?;
            let set_device_id = load_sym(handle, c"AAudioStreamBuilder_setDeviceId")?;
            let set_channel_count = load_sym(handle, c"AAudioStreamBuilder_setChannelCount")?;
            let set_format = load_sym(handle, c"AAudioStreamBuilder_setFormat")?;
            let set_sharing_mode = load_sym(handle, c"AAudioStreamBuilder_setSharingMode")?;
            let set_performance_mode = load_sym(handle, c"AAudioStreamBuilder_setPerformanceMode")?;
            let set_data_callback = load_sym(handle, c"AAudioStreamBuilder_setDataCallback")?;
            let set_error_callback = load_sym(handle, c"AAudioStreamBuilder_setErrorCallback")?;
            let open_stream = load_sym(handle, c"AAudioStreamBuilder_openStream")?;
            let delete_builder = load_sym(handle, c"AAudioStreamBuilder_delete")?;
            let request_start = load_sym(handle, c"AAudioStream_requestStart")?;
            let request_stop = load_sym(handle, c"AAudioStream_requestStop")?;
            let close_stream = load_sym(handle, c"AAudioStream_close")?;
            let get_sample_rate = load_sym(handle, c"AAudioStream_getSampleRate")?;
            let get_channel_count = load_sym(handle, c"AAudioStream_getChannelCount")?;
            let get_format = load_sym(handle, c"AAudioStream_getFormat")?;
            let get_device_id = load_sym(handle, c"AAudioStream_getDeviceId")?;

            Some(AAudioApi {
                create_stream_builder,
                set_direction,
                set_device_id,
                set_channel_count,
                set_format,
                set_sharing_mode,
                set_performance_mode,
                set_data_callback,
                set_error_callback,
                open_stream,
                delete_builder,
                request_start,
                request_stop,
                close_stream,
                get_sample_rate,
                get_channel_count,
                get_format,
                get_device_id,
                set_usage: load_sym(handle, c"AAudioStreamBuilder_setUsage"),
                set_content_type: load_sym(handle, c"AAudioStreamBuilder_setContentType"),
                set_input_preset: load_sym(handle, c"AAudioStreamBuilder_setInputPreset"),
            })
        })
        .as_ref()
}

pub(crate) enum AndroidDeviceSpec {
    Default,
    Id(i32),
    LegacyIdx,
}

pub(crate) fn parse_android_device_spec(spec: Option<&str>) -> AndroidDeviceSpec {
    let Some(raw) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return AndroidDeviceSpec::Default;
    };
    if let Some(id_str) = raw.strip_prefix("@id:") {
        if let Ok(id) = id_str.parse::<i32>() {
            if id > 0 {
                return AndroidDeviceSpec::Id(id);
            }
        }
        return AndroidDeviceSpec::Default;
    }
    if let Some(idx_str) = raw.strip_prefix("@idx:") {
        if idx_str.parse::<usize>().is_ok() {
            return AndroidDeviceSpec::LegacyIdx;
        }
    }
    AndroidDeviceSpec::Default
}

struct InputCallbackCtx {
    prod: <HeapRb<f32> as Split>::Prod,
    channels: usize,
    is_i16: bool,
    failed: Arc<AtomicBool>,
}

struct OutputCallbackCtx {
    cons: <HeapRb<f32> as Split>::Cons,
    channels: usize,
    is_i16: bool,
    failed: Arc<AtomicBool>,
}

unsafe extern "C" fn input_data_cb(
    _stream: *mut AAudioStream,
    user_data: *mut c_void,
    audio_data: *mut c_void,
    num_frames: i32,
) -> i32 {
    if user_data.is_null() || audio_data.is_null() || num_frames <= 0 {
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    }
    let ctx = &mut *(user_data as *mut InputCallbackCtx);
    let frames = num_frames as usize;
    let ch = ctx.channels.max(1);
    let Some(total) = frames.checked_mul(ch) else {
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    };
    if ctx.is_i16 {
        let samples = std::slice::from_raw_parts(audio_data as *const i16, total);
        if ch == 1 {
            for &s in samples {
                let _ = ctx.prod.try_push((s as f32) / 32768.0);
            }
        } else {
            for frame in samples.chunks_exact(ch) {
                let sum: f32 = frame.iter().map(|&s| (s as f32) / 32768.0).sum();
                let _ = ctx.prod.try_push(sum / (ch as f32));
            }
        }
    } else {
        let samples = std::slice::from_raw_parts(audio_data as *const f32, total);
        if ch == 1 {
            let _ = ctx.prod.push_slice(samples);
        } else {
            for frame in samples.chunks_exact(ch) {
                let sum: f32 = frame.iter().copied().sum();
                let _ = ctx.prod.try_push(sum / (ch as f32));
            }
        }
    }
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

unsafe extern "C" fn input_error_cb(
    _stream: *mut AAudioStream,
    user_data: *mut c_void,
    _error: i32,
) {
    if !user_data.is_null() {
        let ctx = &*(user_data as *const InputCallbackCtx);
        ctx.failed.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn output_data_cb(
    _stream: *mut AAudioStream,
    user_data: *mut c_void,
    audio_data: *mut c_void,
    num_frames: i32,
) -> i32 {
    if user_data.is_null() || audio_data.is_null() || num_frames <= 0 {
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    }
    let ctx = &mut *(user_data as *mut OutputCallbackCtx);
    let frames = num_frames as usize;
    let ch = ctx.channels.max(1);
    let Some(total) = frames.checked_mul(ch) else {
        return AAUDIO_CALLBACK_RESULT_CONTINUE;
    };
    if ctx.is_i16 {
        let out = std::slice::from_raw_parts_mut(audio_data as *mut i16, total);
        if ch == 1 {
            for dst in out.iter_mut() {
                let s = ctx.cons.try_pop().unwrap_or(0.0);
                *dst = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            }
        } else if ch == 2 {
            for frame in out.as_chunks_mut::<2>().0 {
                let s = ctx.cons.try_pop().unwrap_or(0.0);
                let q = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                frame[0] = q;
                frame[1] = q;
            }
        } else {
            for frame in out.chunks_exact_mut(ch) {
                let s = ctx.cons.try_pop().unwrap_or(0.0);
                let q = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                frame[0] = q;
                frame[1] = q;
                frame[2..].fill(0);
            }
        }
    } else {
        let out = std::slice::from_raw_parts_mut(audio_data as *mut f32, total);
        if ch == 1 {
            let popped = ctx.cons.pop_slice(out);
            out[popped..].fill(0.0);
        } else if ch == 2 {
            for frame in out.as_chunks_mut::<2>().0 {
                let s = ctx.cons.try_pop().unwrap_or(0.0);
                frame[0] = s;
                frame[1] = s;
            }
        } else {
            for frame in out.chunks_exact_mut(ch) {
                let s = ctx.cons.try_pop().unwrap_or(0.0);
                frame[0] = s;
                frame[1] = s;
                frame[2..].fill(0.0);
            }
        }
    }
    AAUDIO_CALLBACK_RESULT_CONTINUE
}

unsafe extern "C" fn output_error_cb(
    _stream: *mut AAudioStream,
    user_data: *mut c_void,
    _error: i32,
) {
    if !user_data.is_null() {
        let ctx = &*(user_data as *const OutputCallbackCtx);
        ctx.failed.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct AAudioStreamHandle {
    api: &'static AAudioApi,
    stream: *mut AAudioStream,
    ctx_ptr: *mut c_void,
    drop_ctx: unsafe fn(*mut c_void),
}

unsafe impl Send for AAudioStreamHandle {}

impl Drop for AAudioStreamHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.api.request_stop)(self.stream);
            let _ = (self.api.close_stream)(self.stream);
            (self.drop_ctx)(self.ctx_ptr);
        }
    }
}

unsafe fn drop_input_ctx(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(Box::from_raw(ptr as *mut InputCallbackCtx));
    }
}

unsafe fn drop_output_ctx(ptr: *mut c_void) {
    if !ptr.is_null() {
        drop(Box::from_raw(ptr as *mut OutputCallbackCtx));
    }
}

pub(crate) struct BuiltAAudioInput {
    pub stream: AAudioStreamHandle,
    pub cap_cons: <HeapRb<f32> as Split>::Cons,
    pub sample_rate: u32,
    pub channels: usize,
    pub using_fallback: bool,
}

pub(crate) struct BuiltAAudioOutput {
    pub stream: AAudioStreamHandle,
    pub play_prod: <HeapRb<f32> as Split>::Prod,
    pub sample_rate: u32,
    pub channels: usize,
    pub using_fallback: bool,
}

pub(crate) fn open_aaudio_input(
    requested_device_id: i32,
    failed: Arc<AtomicBool>,
) -> Option<BuiltAAudioInput> {
    let api = get_aaudio_api()?;
    let attempts = [
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_FLOAT,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_I16,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_FLOAT,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_I16,
            false,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_FLOAT,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_I16,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_FLOAT,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_I16,
            true,
        ),
    ];
    let count = if requested_device_id != AAUDIO_UNSPECIFIED {
        attempts.len()
    } else {
        4
    };

    for &(dev_id, perf_mode, fmt, fallback) in &attempts[..count] {
        if let Some(built) =
            try_open_aaudio_input_once(api, dev_id, perf_mode, fmt, fallback, &failed)
        {
            return Some(built);
        }
    }
    None
}

fn try_open_aaudio_input_once(
    api: &'static AAudioApi,
    dev_id: i32,
    perf_mode: i32,
    req_fmt: i32,
    using_fallback: bool,
    failed: &Arc<AtomicBool>,
) -> Option<BuiltAAudioInput> {
    unsafe {
        let mut builder: *mut AAudioStreamBuilder = std::ptr::null_mut();
        if (api.create_stream_builder)(&mut builder) != AAUDIO_OK || builder.is_null() {
            return None;
        }

        let rb = HeapRb::<f32>::new(48_000);
        let (prod, cons) = rb.split();
        let ctx = Box::new(InputCallbackCtx {
            prod,
            channels: 1,
            is_i16: req_fmt == AAUDIO_FORMAT_PCM_I16,
            failed: Arc::clone(failed),
        });
        let ctx_ptr = Box::into_raw(ctx);

        (api.set_direction)(builder, AAUDIO_DIRECTION_INPUT);
        (api.set_sharing_mode)(builder, AAUDIO_SHARING_MODE_SHARED);
        (api.set_performance_mode)(builder, perf_mode);
        (api.set_device_id)(builder, dev_id);
        (api.set_format)(builder, req_fmt);
        (api.set_channel_count)(builder, 1);
        if let Some(set_input_preset) = api.set_input_preset {
            set_input_preset(builder, AAUDIO_INPUT_PRESET_VOICE_COMMUNICATION);
        }
        (api.set_data_callback)(builder, input_data_cb, ctx_ptr as *mut c_void);
        (api.set_error_callback)(builder, input_error_cb, ctx_ptr as *mut c_void);

        let mut stream: *mut AAudioStream = std::ptr::null_mut();
        let open_res = (api.open_stream)(builder, &mut stream);
        let _ = (api.delete_builder)(builder);
        if open_res != AAUDIO_OK || stream.is_null() {
            drop_input_ctx(ctx_ptr as *mut c_void);
            return None;
        }

        let raw_rate = (api.get_sample_rate)(stream);
        let raw_ch = (api.get_channel_count)(stream);
        let actual_fmt = (api.get_format)(stream);
        let actual_dev = (api.get_device_id)(stream);
        let Some((actual_rate, actual_ch, is_i16)) =
            validate_aaudio_stream_params(raw_rate, raw_ch, actual_fmt)
        else {
            let _ = (api.close_stream)(stream);
            drop_input_ctx(ctx_ptr as *mut c_void);
            return None;
        };

        (*ctx_ptr).channels = actual_ch;
        (*ctx_ptr).is_i16 = is_i16;

        let start_res = (api.request_start)(stream);
        if start_res != AAUDIO_OK {
            let _ = (api.close_stream)(stream);
            drop_input_ctx(ctx_ptr as *mut c_void);
            return None;
        }

        info!(
            "AAudio VoIP input opened (req_dev={}, actual_dev={}, {} Hz/{}ch, fmt={}, perf={})",
            dev_id, actual_dev, actual_rate, actual_ch, actual_fmt, perf_mode
        );

        Some(BuiltAAudioInput {
            stream: AAudioStreamHandle {
                api,
                stream,
                ctx_ptr: ctx_ptr as *mut c_void,
                drop_ctx: drop_input_ctx,
            },
            cap_cons: cons,
            sample_rate: actual_rate,
            channels: actual_ch,
            using_fallback,
        })
    }
}

pub(crate) fn open_aaudio_output(
    requested_device_id: i32,
    failed: Arc<AtomicBool>,
) -> Option<BuiltAAudioOutput> {
    let api = get_aaudio_api()?;
    let attempts = [
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_FLOAT,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_I16,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_FLOAT,
            false,
        ),
        (
            requested_device_id,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_I16,
            false,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_FLOAT,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_LOW_LATENCY,
            AAUDIO_FORMAT_PCM_I16,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_FLOAT,
            true,
        ),
        (
            AAUDIO_UNSPECIFIED,
            AAUDIO_PERFORMANCE_MODE_NONE,
            AAUDIO_FORMAT_PCM_I16,
            true,
        ),
    ];
    let count = if requested_device_id != AAUDIO_UNSPECIFIED {
        attempts.len()
    } else {
        4
    };

    for &(dev_id, perf_mode, fmt, fallback) in &attempts[..count] {
        if let Some(built) =
            try_open_aaudio_output_once(api, dev_id, perf_mode, fmt, fallback, &failed)
        {
            return Some(built);
        }
    }
    None
}

fn try_open_aaudio_output_once(
    api: &'static AAudioApi,
    dev_id: i32,
    perf_mode: i32,
    req_fmt: i32,
    using_fallback: bool,
    failed: &Arc<AtomicBool>,
) -> Option<BuiltAAudioOutput> {
    unsafe {
        let mut builder: *mut AAudioStreamBuilder = std::ptr::null_mut();
        if (api.create_stream_builder)(&mut builder) != AAUDIO_OK || builder.is_null() {
            return None;
        }

        let rb = HeapRb::<f32>::new(48_000);
        let (mut prod, cons) = rb.split();
        let ctx = Box::new(OutputCallbackCtx {
            cons,
            channels: 2,
            is_i16: req_fmt == AAUDIO_FORMAT_PCM_I16,
            failed: Arc::clone(failed),
        });
        let ctx_ptr = Box::into_raw(ctx);

        (api.set_direction)(builder, AAUDIO_DIRECTION_OUTPUT);
        (api.set_sharing_mode)(builder, AAUDIO_SHARING_MODE_SHARED);
        (api.set_performance_mode)(builder, perf_mode);
        (api.set_device_id)(builder, dev_id);
        (api.set_format)(builder, req_fmt);
        (api.set_channel_count)(builder, 2);
        if let Some(set_usage) = api.set_usage {
            set_usage(builder, AAUDIO_USAGE_VOICE_COMMUNICATION);
        }
        if let Some(set_content_type) = api.set_content_type {
            set_content_type(builder, AAUDIO_CONTENT_TYPE_SPEECH);
        }
        (api.set_data_callback)(builder, output_data_cb, ctx_ptr as *mut c_void);
        (api.set_error_callback)(builder, output_error_cb, ctx_ptr as *mut c_void);

        let mut stream: *mut AAudioStream = std::ptr::null_mut();
        let open_res = (api.open_stream)(builder, &mut stream);
        let _ = (api.delete_builder)(builder);
        if open_res != AAUDIO_OK || stream.is_null() {
            drop_output_ctx(ctx_ptr as *mut c_void);
            return None;
        }

        let raw_rate = (api.get_sample_rate)(stream);
        let raw_ch = (api.get_channel_count)(stream);
        let actual_fmt = (api.get_format)(stream);
        let actual_dev = (api.get_device_id)(stream);
        let Some((actual_rate, actual_ch, is_i16)) =
            validate_aaudio_stream_params(raw_rate, raw_ch, actual_fmt)
        else {
            let _ = (api.close_stream)(stream);
            drop_output_ctx(ctx_ptr as *mut c_void);
            return None;
        };

        (*ctx_ptr).channels = actual_ch;
        (*ctx_ptr).is_i16 = is_i16;

        let prefill_samples = (actual_rate as usize / 100) * 3;
        let silence = vec![0.0f32; prefill_samples];
        let _ = prod.push_slice(&silence);

        let start_res = (api.request_start)(stream);
        if start_res != AAUDIO_OK {
            let _ = (api.close_stream)(stream);
            drop_output_ctx(ctx_ptr as *mut c_void);
            return None;
        }

        info!(
            "AAudio VoIP output opened (req_dev={}, actual_dev={}, {} Hz/{}ch, fmt={}, perf={})",
            dev_id, actual_dev, actual_rate, actual_ch, actual_fmt, perf_mode
        );

        Some(BuiltAAudioOutput {
            stream: AAudioStreamHandle {
                api,
                stream,
                ctx_ptr: ctx_ptr as *mut c_void,
                drop_ctx: drop_output_ctx,
            },
            play_prod: prod,
            sample_rate: actual_rate,
            channels: actual_ch,
            using_fallback,
        })
    }
}

pub(crate) fn validate_aaudio_stream_params(
    raw_rate: i32,
    raw_ch: i32,
    raw_fmt: i32,
) -> Option<(u32, usize, bool)> {
    if !(8_000..=192_000).contains(&raw_rate) || !(1..=16).contains(&raw_ch) {
        return None;
    }
    let is_i16 = match raw_fmt {
        AAUDIO_FORMAT_PCM_FLOAT => false,
        AAUDIO_FORMAT_PCM_I16 => true,
        _ => return None,
    };
    Some((raw_rate as u32, raw_ch as usize, is_i16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_aaudio_stream_params_rejects_negative_and_out_of_range() {
        assert_eq!(
            validate_aaudio_stream_params(-899, 2, AAUDIO_FORMAT_PCM_FLOAT),
            None
        );
        assert_eq!(
            validate_aaudio_stream_params(48_000, -1, AAUDIO_FORMAT_PCM_FLOAT),
            None
        );
        assert_eq!(
            validate_aaudio_stream_params(48_000, 0, AAUDIO_FORMAT_PCM_FLOAT),
            None
        );
        assert_eq!(
            validate_aaudio_stream_params(48_000, 17, AAUDIO_FORMAT_PCM_FLOAT),
            None
        );
        assert_eq!(validate_aaudio_stream_params(48_000, 2, -1), None);
        assert_eq!(
            validate_aaudio_stream_params(48_000, 2, AAUDIO_FORMAT_PCM_FLOAT),
            Some((48_000, 2, false))
        );
        assert_eq!(
            validate_aaudio_stream_params(16_000, 1, AAUDIO_FORMAT_PCM_I16),
            Some((16_000, 1, true))
        );
    }
}
