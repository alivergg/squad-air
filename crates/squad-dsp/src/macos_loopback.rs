use crate::resampler::CaptureResampler;
use ringbuf::{
    traits::{Producer, Split},
    HeapRb,
};
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::{
    atomic::{AtomicBool, AtomicPtr, Ordering},
    Arc, Condvar, Mutex, OnceLock,
};
use tracing::{info, warn};

const RTLD_LAZY: i32 = 0x1;
const RTLD_DEFAULT: *mut c_void = (-2isize) as *mut c_void;

const SC_STREAM_OUTPUT_TYPE_AUDIO: isize = 1;
const BLOCK_HAS_COPY_DISPOSE: i32 = 1 << 25;

const K_AUDIO_FORMAT_FLAG_IS_FLOAT: u32 = 1;
const K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;

const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
const K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_PID_TO_PROCESS_OBJECT: u32 = u32::from_be_bytes(*b"id2p");
const K_AUDIO_TAP_PROPERTY_UID: u32 = u32::from_be_bytes(*b"tuid");
const K_AUDIO_TAP_PROPERTY_FORMAT: u32 = u32::from_be_bytes(*b"tfmt");

const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT32_TYPE: isize = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioBuffer {
    m_number_channels: u32,
    m_data_byte_size: u32,
    m_data: *mut c_void,
}

#[repr(C)]
struct StackAudioBufferList {
    m_number_buffers: u32,
    m_buffers: [AudioBuffer; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    m_sample_rate: f64,
    m_format_id: u32,
    m_format_flags: u32,
    m_bytes_per_packet: u32,
    m_frames_per_packet: u32,
    m_bytes_per_frame: u32,
    m_channels_per_frame: u32,
    m_bits_per_channel: u32,
    m_reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioObjectPropertyAddress {
    m_selector: u32,
    m_scope: u32,
    m_element: u32,
}

extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFTypeArrayCallBacks: c_void;

    fn CFRelease(cf: *const c_void);
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFNumberCreate(
        allocator: *const c_void,
        the_type: isize,
        value_ptr: *const c_void,
    ) -> *const c_void;
    fn CFDictionaryCreateMutable(
        allocator: *const c_void,
        capacity: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *mut c_void;
    fn CFDictionarySetValue(the_dict: *mut c_void, key: *const c_void, value: *const c_void);
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        num_values: isize,
        call_backs: *const c_void,
    ) -> *const c_void;
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        in_object_id: u32,
        in_address: *const AudioObjectPropertyAddress,
        in_qualifier_data_size: u32,
        in_qualifier_data: *const c_void,
        io_data_size: *mut u32,
        out_data: *mut c_void,
    ) -> i32;

    fn AudioHardwareCreateAggregateDevice(
        in_description: *const c_void,
        out_device_id: *mut u32,
    ) -> i32;

    fn AudioHardwareDestroyAggregateDevice(in_device_id: u32) -> i32;

    fn AudioDeviceCreateIOProcID(
        in_device: u32,
        in_proc: unsafe extern "C" fn(
            u32,
            *const c_void,
            *const StackAudioBufferList,
            *const c_void,
            *mut StackAudioBufferList,
            *const c_void,
            *mut c_void,
        ) -> i32,
        in_client_data: *mut c_void,
        out_io_proc_id: *mut *mut c_void,
    ) -> i32;

    fn AudioDeviceDestroyIOProcID(in_device: u32, in_io_proc_id: *mut c_void) -> i32;
    fn AudioDeviceStart(in_device: u32, in_proc_id: *mut c_void) -> i32;
    fn AudioDeviceStop(in_device: u32, in_proc_id: *mut c_void) -> i32;
}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
    fn objc_allocateClassPair(
        superclass: *mut c_void,
        name: *const c_char,
        extra_bytes: usize,
    ) -> *mut c_void;
    fn class_addIvar(
        cls: *mut c_void,
        name: *const c_char,
        size: usize,
        alignment: u8,
        types: *const c_char,
    ) -> bool;
    fn class_addMethod(
        cls: *mut c_void,
        name: *mut c_void,
        imp: *const c_void,
        types: *const c_char,
    ) -> bool;
    fn objc_registerClassPair(cls: *mut c_void);
    fn class_getInstanceVariable(cls: *mut c_void, name: *const c_char) -> *mut c_void;
    fn ivar_getOffset(ivar: *mut c_void) -> isize;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
    fn objc_retain(obj: *mut c_void) -> *mut c_void;
    fn objc_release(obj: *mut c_void);
}

#[inline]
unsafe fn sel(name: &CStr) -> *mut c_void {
    sel_registerName(name.as_ptr())
}

#[inline]
unsafe fn msg0(obj: *mut c_void, s: *mut c_void) -> *mut c_void {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s)
}

#[inline]
unsafe fn msg1_ptr(obj: *mut c_void, s: *mut c_void, a: *mut c_void) -> *mut c_void {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a)
}

#[inline]
unsafe fn msg1_usize(obj: *mut c_void, s: *mut c_void, a: usize) {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a);
}

#[inline]
unsafe fn msg1_isize(obj: *mut c_void, s: *mut c_void, a: isize) {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, isize) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a);
}

#[inline]
unsafe fn msg1_bool(obj: *mut c_void, s: *mut c_void, a: i8) {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, i8) =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, a);
}

#[inline]
unsafe fn msg0_usize(obj: *mut c_void, s: *mut c_void) -> usize {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s)
}

#[inline]
unsafe fn msg1_idx(obj: *mut c_void, s: *mut c_void, idx: usize) -> *mut c_void {
    let f: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void =
        std::mem::transmute(objc_msgSend as *const c_void);
    f(obj, s, idx)
}

// ============================================================================
// Block helpers (memory-safe stack/heap copy+dispose via Arc)
// ============================================================================

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
    copy: unsafe extern "C" fn(*mut c_void, *const c_void),
    dispose: unsafe extern "C" fn(*const c_void),
}

#[repr(C)]
struct ArcBlock<T> {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: *const BlockDescriptor,
    state: *const T,
}

unsafe extern "C" fn block_copy_arc<T>(dst: *mut c_void, src: *const c_void) {
    let src_blk = &*(src as *const ArcBlock<T>);
    let dst_blk = &mut *(dst as *mut ArcBlock<T>);
    if !src_blk.state.is_null() {
        Arc::increment_strong_count(src_blk.state);
    }
    dst_blk.state = src_blk.state;
}

unsafe extern "C" fn block_dispose_arc<T>(blk_ptr: *const c_void) {
    let blk = &*(blk_ptr as *const ArcBlock<T>);
    if !blk.state.is_null() {
        drop(Arc::from_raw(blk.state));
    }
}

impl<T> Drop for ArcBlock<T> {
    fn drop(&mut self) {
        if !self.state.is_null() {
            unsafe {
                drop(Arc::from_raw(self.state));
            }
            self.state = std::ptr::null();
        }
    }
}

// ============================================================================
// Primary: ScreenCaptureKit SCStream audio-only capture (macOS 13.0+)
// ============================================================================

type CmGetFormatDescFn = unsafe extern "C" fn(*mut c_void) -> *const c_void;
type CmGetAsbdFn = unsafe extern "C" fn(*const c_void) -> *const AudioStreamBasicDescription;
type CmGetAblFn = unsafe extern "C" fn(
    *mut c_void,
    *mut usize,
    *mut c_void,
    usize,
    *const c_void,
    *const c_void,
    u32,
    *mut *mut c_void,
) -> i32;

type DispatchSyncFFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, unsafe extern "C" fn(*mut c_void));

struct ScKitFns {
    get_format_desc: CmGetFormatDescFn,
    get_asbd: CmGetAsbdFn,
    get_abl: CmGetAblFn,
    dispatch_sync_f: DispatchSyncFFn,
    stack_block_isa: *const c_void,
}

unsafe impl Send for ScKitFns {}
unsafe impl Sync for ScKitFns {}

static SCKIT_FNS: OnceLock<Option<ScKitFns>> = OnceLock::new();

fn get_sckit_fns() -> Option<&'static ScKitFns> {
    SCKIT_FNS
        .get_or_init(|| unsafe {
            let sc = dlopen(
                c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit".as_ptr(),
                RTLD_LAZY,
            );
            let cm = dlopen(
                c"/System/Library/Frameworks/CoreMedia.framework/CoreMedia".as_ptr(),
                RTLD_LAZY,
            );
            if sc.is_null() || cm.is_null() {
                return None;
            }
            let libsystem = dlopen(c"/usr/lib/libSystem.B.dylib".as_ptr(), RTLD_LAZY);
            let dispatch_sync_sym = if !libsystem.is_null() {
                let s = dlsym(libsystem, c"dispatch_sync_f".as_ptr());
                if s.is_null() {
                    dlsym(RTLD_DEFAULT, c"dispatch_sync_f".as_ptr())
                } else {
                    s
                }
            } else {
                dlsym(RTLD_DEFAULT, c"dispatch_sync_f".as_ptr())
            };
            let stack_block_isa = dlsym(RTLD_DEFAULT, c"_NSConcreteStackBlock".as_ptr());
            let get_format_desc = dlsym(cm, c"CMSampleBufferGetFormatDescription".as_ptr());
            let get_asbd = dlsym(
                cm,
                c"CMAudioFormatDescriptionGetStreamBasicDescription".as_ptr(),
            );
            let get_abl = dlsym(
                cm,
                c"CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer".as_ptr(),
            );
            if stack_block_isa.is_null()
                || dispatch_sync_sym.is_null()
                || get_format_desc.is_null()
                || get_asbd.is_null()
                || get_abl.is_null()
            {
                return None;
            }
            if objc_getClass(c"SCShareableContent".as_ptr()).is_null()
                || objc_getClass(c"SCContentFilter".as_ptr()).is_null()
                || objc_getClass(c"SCStreamConfiguration".as_ptr()).is_null()
                || objc_getClass(c"SCStream".as_ptr()).is_null()
            {
                return None;
            }
            Some(ScKitFns {
                get_format_desc: std::mem::transmute::<*mut c_void, CmGetFormatDescFn>(
                    get_format_desc,
                ),
                get_asbd: std::mem::transmute::<*mut c_void, CmGetAsbdFn>(get_asbd),
                get_abl: std::mem::transmute::<*mut c_void, CmGetAblFn>(get_abl),
                dispatch_sync_f: std::mem::transmute::<*mut c_void, DispatchSyncFFn>(
                    dispatch_sync_sym,
                ),
                stack_block_isa,
            })
        })
        .as_ref()
}

struct ScAudioCallbackCtx {
    prod: <HeapRb<f32> as Split>::Prod,
    failed: Arc<AtomicBool>,
    fns: &'static ScKitFns,
}

static SC_DELEGATE_CLASS: OnceLock<(usize, isize)> = OnceLock::new();

fn get_sc_delegate_class() -> Option<(*mut c_void, isize)> {
    let &(cls_addr, ivar_offset) = SC_DELEGATE_CLASS.get_or_init(|| unsafe {
        let ns_object = objc_getClass(c"NSObject".as_ptr());
        if ns_object.is_null() {
            return (0, 0);
        }
        let cls = objc_allocateClassPair(ns_object, c"SquadAirScAudioDelegate".as_ptr(), 0);
        if cls.is_null() {
            let existing = objc_getClass(c"SquadAirScAudioDelegate".as_ptr());
            if existing.is_null() {
                return (0, 0);
            }
            let ivar = class_getInstanceVariable(existing, c"_ctx".as_ptr());
            return (existing as usize, ivar_getOffset(ivar));
        }

        class_addIvar(
            cls,
            c"_ctx".as_ptr(),
            std::mem::size_of::<*mut c_void>(),
            std::mem::align_of::<*mut c_void>().trailing_zeros() as u8,
            c"^v".as_ptr(),
        );
        class_addMethod(
            cls,
            sel(c"stream:didOutputSampleBuffer:ofType:"),
            sc_did_output_sample_buffer as *const c_void,
            c"v@:@@q".as_ptr(),
        );
        class_addMethod(
            cls,
            sel(c"stream:didStopWithError:"),
            sc_did_stop_with_error as *const c_void,
            c"v@:@@".as_ptr(),
        );
        objc_registerClassPair(cls);
        let ivar = class_getInstanceVariable(cls, c"_ctx".as_ptr());
        (cls as usize, ivar_getOffset(ivar))
    });
    if cls_addr == 0 {
        None
    } else {
        Some((cls_addr as *mut c_void, ivar_offset))
    }
}

#[inline]
unsafe fn delegate_ctx_slot(
    this: *mut c_void,
    ivar_offset: isize,
) -> *const AtomicPtr<ScAudioCallbackCtx> {
    (this as *const u8).offset(ivar_offset) as *const AtomicPtr<ScAudioCallbackCtx>
}

#[inline]
unsafe fn read_delegate_ctx(this: *mut c_void) -> *mut ScAudioCallbackCtx {
    if this.is_null() {
        return std::ptr::null_mut();
    }
    let Some((_, ivar_offset)) = get_sc_delegate_class() else {
        return std::ptr::null_mut();
    };
    (*delegate_ctx_slot(this, ivar_offset)).load(Ordering::Acquire)
}

#[inline]
unsafe fn write_delegate_ctx(this: *mut c_void, ivar_offset: isize, ctx: *mut ScAudioCallbackCtx) {
    (*delegate_ctx_slot(this, ivar_offset)).store(ctx, Ordering::Release);
}

unsafe extern "C" fn clear_delegate_ctx_work(delegate: *mut c_void) {
    if delegate.is_null() {
        return;
    }
    if let Some((_, ivar_offset)) = get_sc_delegate_class() {
        write_delegate_ctx(delegate, ivar_offset, std::ptr::null_mut());
    }
}

unsafe extern "C" fn sc_did_output_sample_buffer(
    this: *mut c_void,
    _cmd: *mut c_void,
    _stream: *mut c_void,
    sample_buffer: *mut c_void,
    output_type: isize,
) {
    if output_type != SC_STREAM_OUTPUT_TYPE_AUDIO || sample_buffer.is_null() {
        return;
    }
    let ctx_ptr = read_delegate_ctx(this);
    if ctx_ptr.is_null() {
        return;
    }
    let ctx = &mut *ctx_ptr;

    let fmt_desc = (ctx.fns.get_format_desc)(sample_buffer);
    if fmt_desc.is_null() {
        return;
    }
    let asbd_ptr = (ctx.fns.get_asbd)(fmt_desc);
    if asbd_ptr.is_null() {
        return;
    }
    let asbd = *asbd_ptr;
    let is_float = (asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_FLOAT) != 0;
    let is_non_interleaved = (asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED) != 0;

    let mut abl = StackAudioBufferList {
        m_number_buffers: 0,
        m_buffers: [AudioBuffer::default(); 8],
    };
    let mut needed: usize = 0;
    let mut block_buf: *mut c_void = std::ptr::null_mut();
    let status = (ctx.fns.get_abl)(
        sample_buffer,
        &mut needed,
        &mut abl as *mut _ as *mut c_void,
        std::mem::size_of::<StackAudioBufferList>(),
        std::ptr::null(),
        std::ptr::null(),
        0,
        &mut block_buf,
    );

    if status == 0 {
        let num_bufs = (abl.m_number_buffers as usize).min(8);
        if is_float && asbd.m_bits_per_channel == 32 && num_bufs > 0 {
            if is_non_interleaved || num_bufs >= 2 {
                let buf0 = &abl.m_buffers[0];
                if !buf0.m_data.is_null() {
                    let frames = (buf0.m_data_byte_size as usize) / std::mem::size_of::<f32>();
                    if num_bufs == 1 {
                        let s0 = std::slice::from_raw_parts(buf0.m_data as *const f32, frames);
                        let _ = ctx.prod.push_slice(s0);
                    } else {
                        let buf1 = &abl.m_buffers[1];
                        if !buf1.m_data.is_null() {
                            let frames1 =
                                (buf1.m_data_byte_size as usize) / std::mem::size_of::<f32>();
                            let n = frames.min(frames1);
                            let s0 = std::slice::from_raw_parts(buf0.m_data as *const f32, n);
                            let s1 = std::slice::from_raw_parts(buf1.m_data as *const f32, n);
                            for i in 0..n {
                                let _ = ctx.prod.try_push((s0[i] + s1[i]) * 0.5);
                            }
                        }
                    }
                }
            } else {
                let buf = &abl.m_buffers[0];
                if !buf.m_data.is_null() {
                    let ch = (buf.m_number_channels.max(asbd.m_channels_per_frame) as usize).max(1);
                    let total = (buf.m_data_byte_size as usize) / std::mem::size_of::<f32>();
                    let samples = std::slice::from_raw_parts(buf.m_data as *const f32, total);
                    if ch == 1 {
                        let _ = ctx.prod.push_slice(samples);
                    } else {
                        for frame in samples.chunks_exact(ch) {
                            let sum: f32 = frame.iter().copied().sum();
                            let _ = ctx.prod.try_push(sum / (ch as f32));
                        }
                    }
                }
            }
        } else if !is_float && asbd.m_bits_per_channel == 16 && num_bufs > 0 {
            let buf = &abl.m_buffers[0];
            if !buf.m_data.is_null() {
                let ch = (buf.m_number_channels.max(asbd.m_channels_per_frame) as usize).max(1);
                let total = (buf.m_data_byte_size as usize) / std::mem::size_of::<i16>();
                let samples = std::slice::from_raw_parts(buf.m_data as *const i16, total);
                for frame in samples.chunks_exact(ch) {
                    let sum: f32 = frame.iter().map(|&s| (s as f32) / 32768.0).sum();
                    let _ = ctx.prod.try_push(sum / (ch as f32));
                }
            }
        }
    }

    if !block_buf.is_null() {
        CFRelease(block_buf);
    }
}

unsafe extern "C" fn sc_did_stop_with_error(
    this: *mut c_void,
    _cmd: *mut c_void,
    _stream: *mut c_void,
    _error: *mut c_void,
) {
    let ctx_ptr = read_delegate_ctx(this);
    if !ctx_ptr.is_null() {
        (*ctx_ptr).failed.store(true, Ordering::Relaxed);
    }
}

struct ShareableContentWait {
    lock: Mutex<(bool, usize)>,
    cv: Condvar,
}

unsafe extern "C" fn shareable_content_cb(
    blk_ptr: *mut c_void,
    content: *mut c_void,
    error: *mut c_void,
) {
    let blk = &*(blk_ptr as *const ArcBlock<ShareableContentWait>);
    if blk.state.is_null() {
        return;
    }
    let state = &*blk.state;
    let mut display_addr = 0usize;
    if error.is_null() && !content.is_null() {
        let displays = msg0(content, sel(c"displays"));
        if !displays.is_null() && msg0_usize(displays, sel(c"count")) > 0 {
            let disp = msg1_idx(displays, sel(c"objectAtIndex:"), 0);
            if !disp.is_null() {
                objc_retain(disp);
                display_addr = disp as usize;
            }
        }
    }
    let mut guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    *guard = (true, display_addr);
    state.cv.notify_all();
}

struct ErrorCompletionWait {
    lock: Mutex<(bool, bool)>,
    cv: Condvar,
}

unsafe extern "C" fn err_completion_cb(blk_ptr: *mut c_void, error: *mut c_void) {
    let blk = &*(blk_ptr as *const ArcBlock<ErrorCompletionWait>);
    if blk.state.is_null() {
        return;
    }
    let state = &*blk.state;
    let mut guard = state.lock.lock().unwrap_or_else(|e| e.into_inner());
    *guard = (true, error.is_null());
    state.cv.notify_all();
}

static SHAREABLE_CONTENT_DESC: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<ArcBlock<ShareableContentWait>>(),
    copy: block_copy_arc::<ShareableContentWait>,
    dispose: block_dispose_arc::<ShareableContentWait>,
};

static ERR_COMPLETION_DESC: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<ArcBlock<ErrorCompletionWait>>(),
    copy: block_copy_arc::<ErrorCompletionWait>,
    dispose: block_dispose_arc::<ErrorCompletionWait>,
};

pub(crate) struct ScKitStreamGuard {
    stream: *mut c_void,
    filter: *mut c_void,
    cfg: *mut c_void,
    delegate: *mut c_void,
    queue: *mut c_void,
    ctx_ptr: *mut ScAudioCallbackCtx,
    fns: &'static ScKitFns,
}

unsafe impl Send for ScKitStreamGuard {}

impl Drop for ScKitStreamGuard {
    fn drop(&mut self) {
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let mut err: *mut c_void = std::ptr::null_mut();
            let remove_fn: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                isize,
                *mut *mut c_void,
            ) -> i8 = std::mem::transmute(objc_msgSend as *const c_void);
            let _ = remove_fn(
                self.stream,
                sel(c"removeStreamOutput:type:error:"),
                self.delegate,
                SC_STREAM_OUTPUT_TYPE_AUDIO,
                &mut err,
            );

            let wait = Arc::new(ErrorCompletionWait {
                lock: Mutex::new((false, false)),
                cv: Condvar::new(),
            });
            let mut blk = ArcBlock {
                isa: self.fns.stack_block_isa,
                flags: BLOCK_HAS_COPY_DISPOSE,
                reserved: 0,
                invoke: err_completion_cb as *const c_void,
                descriptor: &ERR_COMPLETION_DESC,
                state: Arc::into_raw(Arc::clone(&wait)),
            };
            let stop_fn: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) =
                std::mem::transmute(objc_msgSend as *const c_void);
            stop_fn(
                self.stream,
                sel(c"stopCaptureWithCompletionHandler:"),
                &mut blk as *mut _ as *mut c_void,
            );

            let mut completed = false;
            {
                let guard = wait.lock.lock().unwrap_or_else(|e| e.into_inner());
                let res =
                    wait.cv
                        .wait_timeout_while(guard, std::time::Duration::from_secs(2), |s| !s.0);
                if let Ok((g, _)) = res {
                    completed = g.0;
                }
            }
            drop(blk);

            clear_delegate_ctx_work(self.delegate);
            (self.fns.dispatch_sync_f)(self.queue, self.delegate, clear_delegate_ctx_work);

            objc_release(self.stream);
            objc_release(self.filter);
            objc_release(self.cfg);
            objc_release(self.delegate);
            dispatch_release(self.queue);

            if completed && !self.ctx_ptr.is_null() {
                drop(Box::from_raw(self.ctx_ptr));
                self.ctx_ptr = std::ptr::null_mut();
            }
            objc_autoreleasePoolPop(pool);
        }
    }
}

fn try_start_screencapturekit() -> Option<BuiltMacOsLoopback> {
    unsafe {
        if !CGPreflightScreenCaptureAccess() {
            warn!("ScreenCaptureKit audio skipped: Screen Recording permission not granted");
            return None;
        }
        let fns = get_sckit_fns()?;
        let (delegate_cls, ivar_offset) = get_sc_delegate_class()?;

        let pool = objc_autoreleasePoolPush();

        let wait = Arc::new(ShareableContentWait {
            lock: Mutex::new((false, 0)),
            cv: Condvar::new(),
        });
        let mut content_blk = ArcBlock {
            isa: fns.stack_block_isa,
            flags: BLOCK_HAS_COPY_DISPOSE,
            reserved: 0,
            invoke: shareable_content_cb as *const c_void,
            descriptor: &SHAREABLE_CONTENT_DESC,
            state: Arc::into_raw(Arc::clone(&wait)),
        };

        let sc_content_cls = objc_getClass(c"SCShareableContent".as_ptr());
        msg1_ptr(
            sc_content_cls,
            sel(c"getShareableContentWithCompletionHandler:"),
            &mut content_blk as *mut _ as *mut c_void,
        );

        let display_ptr = {
            let guard = wait.lock.lock().unwrap_or_else(|e| e.into_inner());
            let (g, _) = wait
                .cv
                .wait_timeout_while(guard, std::time::Duration::from_secs(3), |s| !s.0)
                .ok()?;
            g.1 as *mut c_void
        };
        drop(content_blk);

        if display_ptr.is_null() {
            objc_autoreleasePoolPop(pool);
            warn!("ScreenCaptureKit getShareableContentWithCompletionHandler returned no display");
            return None;
        }

        let ns_array_cls = objc_getClass(c"NSArray".as_ptr());
        let empty_arr = msg0(ns_array_cls, sel(c"array"));
        let filter_cls = objc_getClass(c"SCContentFilter".as_ptr());
        let filter_alloc = msg0(filter_cls, sel(c"alloc"));
        let init_filter_fn: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> *mut c_void = std::mem::transmute(objc_msgSend as *const c_void);
        let filter = init_filter_fn(
            filter_alloc,
            sel(c"initWithDisplay:excludingWindows:"),
            display_ptr,
            empty_arr,
        );
        objc_release(display_ptr);
        if filter.is_null() {
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let cfg_cls = objc_getClass(c"SCStreamConfiguration".as_ptr());
        let cfg = msg0(msg0(cfg_cls, sel(c"alloc")), sel(c"init"));
        if cfg.is_null() {
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            return None;
        }
        msg1_usize(cfg, sel(c"setWidth:"), 2);
        msg1_usize(cfg, sel(c"setHeight:"), 2);
        let set_min_interval: unsafe extern "C" fn(*mut c_void, *mut c_void, CMTime) =
            std::mem::transmute(objc_msgSend as *const c_void);
        set_min_interval(
            cfg,
            sel(c"setMinimumFrameInterval:"),
            CMTime {
                value: 1,
                timescale: 1,
                flags: 1,
                epoch: 0,
            },
        );
        msg1_bool(cfg, sel(c"setCapturesAudio:"), 1);
        msg1_bool(cfg, sel(c"setExcludesCurrentProcessAudio:"), 1);
        msg1_isize(cfg, sel(c"setSampleRate:"), 48_000);
        msg1_isize(cfg, sel(c"setChannelCount:"), 2);

        let rb = HeapRb::<f32>::new(48_000 / 2);
        let (prod, cons) = rb.split();
        let failed = Arc::new(AtomicBool::new(false));
        let ctx_ptr = Box::into_raw(Box::new(ScAudioCallbackCtx {
            prod,
            failed: Arc::clone(&failed),
            fns,
        }));

        let delegate = msg0(msg0(delegate_cls, sel(c"alloc")), sel(c"init"));
        if delegate.is_null() {
            drop(Box::from_raw(ctx_ptr));
            objc_release(cfg);
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            return None;
        }
        write_delegate_ctx(delegate, ivar_offset, ctx_ptr);

        let stream_cls = objc_getClass(c"SCStream".as_ptr());
        let stream_alloc = msg0(stream_cls, sel(c"alloc"));
        let init_stream_fn: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
            *mut c_void,
        ) -> *mut c_void = std::mem::transmute(objc_msgSend as *const c_void);
        let stream = init_stream_fn(
            stream_alloc,
            sel(c"initWithFilter:configuration:delegate:"),
            filter,
            cfg,
            delegate,
        );
        if stream.is_null() {
            objc_release(delegate);
            drop(Box::from_raw(ctx_ptr));
            objc_release(cfg);
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let queue = dispatch_queue_create(c"com.squadair.sckit.audio".as_ptr(), std::ptr::null());
        if queue.is_null() {
            objc_release(stream);
            objc_release(delegate);
            drop(Box::from_raw(ctx_ptr));
            objc_release(cfg);
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let mut add_err: *mut c_void = std::ptr::null_mut();
        let add_output_fn: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            *mut c_void,
            isize,
            *mut c_void,
            *mut *mut c_void,
        ) -> i8 = std::mem::transmute(objc_msgSend as *const c_void);
        let added = add_output_fn(
            stream,
            sel(c"addStreamOutput:type:sampleHandlerQueue:error:"),
            delegate,
            SC_STREAM_OUTPUT_TYPE_AUDIO,
            queue,
            &mut add_err,
        );
        if added == 0 {
            dispatch_release(queue);
            objc_release(stream);
            objc_release(delegate);
            drop(Box::from_raw(ctx_ptr));
            objc_release(cfg);
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            warn!("ScreenCaptureKit addStreamOutput(Audio) failed");
            return None;
        }

        let start_wait = Arc::new(ErrorCompletionWait {
            lock: Mutex::new((false, false)),
            cv: Condvar::new(),
        });
        let mut start_blk = ArcBlock {
            isa: fns.stack_block_isa,
            flags: BLOCK_HAS_COPY_DISPOSE,
            reserved: 0,
            invoke: err_completion_cb as *const c_void,
            descriptor: &ERR_COMPLETION_DESC,
            state: Arc::into_raw(Arc::clone(&start_wait)),
        };
        msg1_ptr(
            stream,
            sel(c"startCaptureWithCompletionHandler:"),
            &mut start_blk as *mut _ as *mut c_void,
        );

        let started_ok = {
            let guard = start_wait.lock.lock().unwrap_or_else(|e| e.into_inner());
            let res =
                start_wait
                    .cv
                    .wait_timeout_while(guard, std::time::Duration::from_secs(3), |s| !s.0);
            res.ok().is_some_and(|(g, _)| g.0 && g.1)
        };
        drop(start_blk);

        if !started_ok {
            clear_delegate_ctx_work(delegate);
            (fns.dispatch_sync_f)(queue, delegate, clear_delegate_ctx_work);
            dispatch_release(queue);
            objc_release(stream);
            objc_release(delegate);
            drop(Box::from_raw(ctx_ptr));
            objc_release(cfg);
            objc_release(filter);
            objc_autoreleasePoolPop(pool);
            warn!("ScreenCaptureKit startCaptureWithCompletionHandler failed or timed out");
            return None;
        }

        objc_autoreleasePoolPop(pool);
        info!("macOS ScreenCaptureKit audio loopback active (48000 Hz/2ch, excludes own process)");

        Some(BuiltMacOsLoopback {
            handle: MacOsLoopbackCapture::ScreenCaptureKit(ScKitStreamGuard {
                stream,
                filter,
                cfg,
                delegate,
                queue,
                ctx_ptr,
                fns,
            }),
            cons,
            resampler: CaptureResampler::new(48_000),
            failed,
        })
    }
}

// ============================================================================
// Secondary: CoreAudio Process Tap (macOS 14.2+)
// ============================================================================

type CreateTapFn = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
type DestroyTapFn = unsafe extern "C" fn(u32) -> i32;

struct CoreAudioTapCtx {
    prod: <HeapRb<f32> as Split>::Prod,
    channels: usize,
    is_float: bool,
    is_non_interleaved: bool,
}

unsafe extern "C" fn process_tap_io_proc(
    _in_device: u32,
    _in_now: *const c_void,
    in_input_data: *const StackAudioBufferList,
    _in_input_time: *const c_void,
    _out_output_data: *mut StackAudioBufferList,
    _in_output_time: *const c_void,
    in_client_data: *mut c_void,
) -> i32 {
    if in_input_data.is_null() || in_client_data.is_null() {
        return 0;
    }
    let ctx = &mut *(in_client_data as *mut CoreAudioTapCtx);
    let abl = &*in_input_data;
    let num_bufs = (abl.m_number_buffers as usize).min(8);
    if num_bufs == 0 || !ctx.is_float {
        return 0;
    }

    if ctx.is_non_interleaved || num_bufs >= 2 {
        let b0 = &abl.m_buffers[0];
        if !b0.m_data.is_null() {
            let frames = (b0.m_data_byte_size as usize) / std::mem::size_of::<f32>();
            if num_bufs == 1 {
                let s0 = std::slice::from_raw_parts(b0.m_data as *const f32, frames);
                let _ = ctx.prod.push_slice(s0);
            } else {
                let b1 = &abl.m_buffers[1];
                if !b1.m_data.is_null() {
                    let frames1 = (b1.m_data_byte_size as usize) / std::mem::size_of::<f32>();
                    let n = frames.min(frames1);
                    let s0 = std::slice::from_raw_parts(b0.m_data as *const f32, n);
                    let s1 = std::slice::from_raw_parts(b1.m_data as *const f32, n);
                    for i in 0..n {
                        let _ = ctx.prod.try_push((s0[i] + s1[i]) * 0.5);
                    }
                }
            }
        }
    } else {
        let b = &abl.m_buffers[0];
        if !b.m_data.is_null() {
            let ch = (b.m_number_channels as usize).max(ctx.channels).max(1);
            let total = (b.m_data_byte_size as usize) / std::mem::size_of::<f32>();
            let samples = std::slice::from_raw_parts(b.m_data as *const f32, total);
            if ch == 1 {
                let _ = ctx.prod.push_slice(samples);
            } else {
                for frame in samples.chunks_exact(ch) {
                    let sum: f32 = frame.iter().copied().sum();
                    let _ = ctx.prod.try_push(sum / (ch as f32));
                }
            }
        }
    }
    0
}

pub(crate) struct ProcessTapGuard {
    agg_id: u32,
    ioproc_id: *mut c_void,
    tap_id: u32,
    destroy_tap: DestroyTapFn,
    ctx_ptr: *mut CoreAudioTapCtx,
}

unsafe impl Send for ProcessTapGuard {}

impl Drop for ProcessTapGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.ioproc_id.is_null() {
                let _ = AudioDeviceStop(self.agg_id, self.ioproc_id);
                let _ = AudioDeviceDestroyIOProcID(self.agg_id, self.ioproc_id);
            }
            if self.agg_id != 0 {
                let _ = AudioHardwareDestroyAggregateDevice(self.agg_id);
            }
            if self.tap_id != 0 {
                let _ = (self.destroy_tap)(self.tap_id);
            }
            if !self.ctx_ptr.is_null() {
                drop(Box::from_raw(self.ctx_ptr));
            }
        }
    }
}

unsafe fn cf_str(s: &CStr) -> *const c_void {
    CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), K_CF_STRING_ENCODING_UTF8)
}

unsafe fn cf_i32(val: i32) -> *const c_void {
    CFNumberCreate(
        std::ptr::null(),
        K_CF_NUMBER_SINT32_TYPE,
        &val as *const i32 as *const c_void,
    )
}

fn try_start_coreaudio_process_tap() -> Option<BuiltMacOsLoopback> {
    unsafe {
        let ca = dlopen(
            c"/System/Library/Frameworks/CoreAudio.framework/CoreAudio".as_ptr(),
            RTLD_LAZY,
        );
        if ca.is_null() {
            return None;
        }
        let create_sym = dlsym(ca, c"AudioHardwareCreateProcessTap".as_ptr());
        let destroy_sym = dlsym(ca, c"AudioHardwareDestroyProcessTap".as_ptr());
        let tap_desc_cls = objc_getClass(c"CATapDescription".as_ptr());
        if create_sym.is_null() || destroy_sym.is_null() || tap_desc_cls.is_null() {
            return None;
        }
        let create_tap: CreateTapFn = std::mem::transmute::<*mut c_void, CreateTapFn>(create_sym);
        let destroy_tap: DestroyTapFn =
            std::mem::transmute::<*mut c_void, DestroyTapFn>(destroy_sym);

        let pool = objc_autoreleasePoolPush();

        let pid = std::process::id() as i32;
        let mut own_proc_id: u32 = 0;
        let mut prop_size = std::mem::size_of::<u32>() as u32;
        let pid_addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_HARDWARE_PROPERTY_TRANSLATE_PID_TO_PROCESS_OBJECT,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let _ = AudioObjectGetPropertyData(
            K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &pid_addr,
            std::mem::size_of::<i32>() as u32,
            &pid as *const i32 as *const c_void,
            &mut prop_size,
            &mut own_proc_id as *mut u32 as *mut c_void,
        );

        let ns_array_cls = objc_getClass(c"NSArray".as_ptr());
        let exclude_arr = if own_proc_id != 0 {
            let ns_number_cls = objc_getClass(c"NSNumber".as_ptr());
            let num_fn: unsafe extern "C" fn(*mut c_void, *mut c_void, u32) -> *mut c_void =
                std::mem::transmute(objc_msgSend as *const c_void);
            let num = num_fn(ns_number_cls, sel(c"numberWithUnsignedInt:"), own_proc_id);
            msg1_ptr(ns_array_cls, sel(c"arrayWithObject:"), num)
        } else {
            msg0(ns_array_cls, sel(c"array"))
        };

        let tap_desc = msg1_ptr(
            msg0(tap_desc_cls, sel(c"alloc")),
            sel(c"initStereoGlobalTapButExcludeProcesses:"),
            exclude_arr,
        );
        if tap_desc.is_null() {
            objc_autoreleasePoolPop(pool);
            return None;
        }
        msg1_bool(tap_desc, sel(c"setPrivate:"), 1);
        msg1_isize(tap_desc, sel(c"setMuteBehavior:"), 0);

        let mut tap_id: u32 = 0;
        let tap_status = create_tap(tap_desc, &mut tap_id);
        objc_release(tap_desc);
        if tap_status != 0 || tap_id == 0 {
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let mut tap_uid_cf: *const c_void = std::ptr::null();
        let mut uid_size = std::mem::size_of::<*const c_void>() as u32;
        let uid_addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_TAP_PROPERTY_UID,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        if AudioObjectGetPropertyData(
            tap_id,
            &uid_addr,
            0,
            std::ptr::null(),
            &mut uid_size,
            &mut tap_uid_cf as *mut *const c_void as *mut c_void,
        ) != 0
            || tap_uid_cf.is_null()
        {
            let _ = destroy_tap(tap_id);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let mut asbd = AudioStreamBasicDescription::default();
        let mut fmt_size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
        let fmt_addr = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_TAP_PROPERTY_FORMAT,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let _ = AudioObjectGetPropertyData(
            tap_id,
            &fmt_addr,
            0,
            std::ptr::null(),
            &mut fmt_size,
            &mut asbd as *mut AudioStreamBasicDescription as *mut c_void,
        );

        let tap_rate = if asbd.m_sample_rate >= 8_000.0 && asbd.m_sample_rate <= 192_000.0 {
            asbd.m_sample_rate.round() as u32
        } else {
            48_000
        };
        let tap_ch = (asbd.m_channels_per_frame as usize).max(2);
        let is_float =
            (asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_FLOAT) != 0 || asbd.m_format_flags == 0;
        let is_non_interleaved =
            (asbd.m_format_flags & K_AUDIO_FORMAT_FLAG_IS_NON_INTERLEAVED) != 0;

        let tap_entry = CFDictionaryCreateMutable(
            std::ptr::null(),
            2,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let k_uid = cf_str(c"uid");
        let k_drift = cf_str(c"drift");
        let v_one = cf_i32(1);
        CFDictionarySetValue(tap_entry, k_uid, tap_uid_cf);
        CFDictionarySetValue(tap_entry, k_drift, v_one);

        let tap_list = CFArrayCreate(
            std::ptr::null(),
            &(tap_entry as *const c_void),
            1,
            &kCFTypeArrayCallBacks,
        );

        let agg_dict = CFDictionaryCreateMutable(
            std::ptr::null(),
            6,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let agg_uid_str = CString::new(format!("com.squadair.tap.{}", std::process::id())).ok()?;
        let k_name = cf_str(c"name");
        let k_private = cf_str(c"private");
        let k_stacked = cf_str(c"stacked");
        let k_tap_autostart = cf_str(c"tapautostart");
        let k_taps = cf_str(c"taps");
        let v_agg_uid = cf_str(&agg_uid_str);
        let v_agg_name = cf_str(c"SquadAir Tap");
        let v_zero = cf_i32(0);

        CFDictionarySetValue(agg_dict, k_uid, v_agg_uid);
        CFDictionarySetValue(agg_dict, k_name, v_agg_name);
        CFDictionarySetValue(agg_dict, k_private, v_one);
        CFDictionarySetValue(agg_dict, k_stacked, v_zero);
        CFDictionarySetValue(agg_dict, k_tap_autostart, v_one);
        CFDictionarySetValue(agg_dict, k_taps, tap_list);

        let mut agg_id: u32 = 0;
        let agg_status = AudioHardwareCreateAggregateDevice(agg_dict, &mut agg_id);

        CFRelease(agg_dict);
        CFRelease(tap_list);
        CFRelease(tap_entry);
        CFRelease(tap_uid_cf);
        CFRelease(k_uid);
        CFRelease(k_drift);
        CFRelease(k_name);
        CFRelease(k_private);
        CFRelease(k_stacked);
        CFRelease(k_tap_autostart);
        CFRelease(k_taps);
        CFRelease(v_agg_uid);
        CFRelease(v_agg_name);
        CFRelease(v_one);
        CFRelease(v_zero);

        if agg_status != 0 || agg_id == 0 {
            let _ = destroy_tap(tap_id);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        let rb = HeapRb::<f32>::new((tap_rate as usize) / 2);
        let (prod, cons) = rb.split();
        let ctx_ptr = Box::into_raw(Box::new(CoreAudioTapCtx {
            prod,
            channels: tap_ch,
            is_float,
            is_non_interleaved,
        }));

        let mut ioproc_id: *mut c_void = std::ptr::null_mut();
        if AudioDeviceCreateIOProcID(
            agg_id,
            process_tap_io_proc,
            ctx_ptr as *mut c_void,
            &mut ioproc_id,
        ) != 0
            || ioproc_id.is_null()
        {
            drop(Box::from_raw(ctx_ptr));
            let _ = AudioHardwareDestroyAggregateDevice(agg_id);
            let _ = destroy_tap(tap_id);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        if AudioDeviceStart(agg_id, ioproc_id) != 0 {
            let _ = AudioDeviceDestroyIOProcID(agg_id, ioproc_id);
            drop(Box::from_raw(ctx_ptr));
            let _ = AudioHardwareDestroyAggregateDevice(agg_id);
            let _ = destroy_tap(tap_id);
            objc_autoreleasePoolPop(pool);
            return None;
        }

        objc_autoreleasePoolPop(pool);
        info!(
            "macOS CoreAudio Process Tap audio loopback active ({} Hz/{}ch, excludes own PID {})",
            tap_rate, tap_ch, pid
        );

        Some(BuiltMacOsLoopback {
            handle: MacOsLoopbackCapture::CoreAudioTap(ProcessTapGuard {
                agg_id,
                ioproc_id,
                tap_id,
                destroy_tap,
                ctx_ptr,
            }),
            cons,
            resampler: CaptureResampler::new(tap_rate),
            failed: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[allow(dead_code)]
pub(crate) enum MacOsLoopbackCapture {
    ScreenCaptureKit(ScKitStreamGuard),
    CoreAudioTap(ProcessTapGuard),
}

pub(crate) struct BuiltMacOsLoopback {
    pub handle: MacOsLoopbackCapture,
    pub cons: <HeapRb<f32> as Split>::Cons,
    pub resampler: CaptureResampler,
    pub failed: Arc<AtomicBool>,
}

pub(crate) fn start_macos_native_loopback() -> Option<BuiltMacOsLoopback> {
    if let Some(built) = try_start_screencapturekit() {
        return Some(built);
    }
    try_start_coreaudio_process_tap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sckit_delegate_ctx_clear_and_dispatch_sync_barrier() {
        unsafe {
            let Some(fns) = get_sckit_fns() else {
                return;
            };
            let Some((delegate_cls, ivar_offset)) = get_sc_delegate_class() else {
                panic!("get_sc_delegate_class should succeed on macOS");
            };

            let pool = objc_autoreleasePoolPush();
            let delegate = msg0(msg0(delegate_cls, sel(c"alloc")), sel(c"init"));
            assert!(!delegate.is_null());

            let rb = HeapRb::<f32>::new(64);
            let (prod, _cons) = rb.split();
            let failed = Arc::new(AtomicBool::new(false));
            let ctx_ptr = Box::into_raw(Box::new(ScAudioCallbackCtx {
                prod,
                failed: Arc::clone(&failed),
                fns,
            }));
            write_delegate_ctx(delegate, ivar_offset, ctx_ptr);
            assert_eq!(read_delegate_ctx(delegate), ctx_ptr);

            let queue =
                dispatch_queue_create(c"com.squadair.sckit.test_drain".as_ptr(), std::ptr::null());
            assert!(!queue.is_null());

            clear_delegate_ctx_work(delegate);
            (fns.dispatch_sync_f)(queue, delegate, clear_delegate_ctx_work);
            assert!(read_delegate_ctx(delegate).is_null());

            sc_did_output_sample_buffer(
                delegate,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                SC_STREAM_OUTPUT_TYPE_AUDIO,
            );
            sc_did_stop_with_error(
                delegate,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert!(!failed.load(Ordering::Relaxed));

            dispatch_release(queue);
            objc_release(delegate);
            drop(Box::from_raw(ctx_ptr));
            objc_autoreleasePoolPop(pool);
        }
    }
}
