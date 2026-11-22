use android_logger::Config;
use jni::objects::{GlobalRef, JClass, JObject};
use jni::JNIEnv;
use log::LevelFilter;
use std::sync::{Once, OnceLock};

static APP_CONTEXT_REF: OnceLock<GlobalRef> = OnceLock::new();
static NDK_CONTEXT_ONCE: Once = Once::new();

#[no_mangle]
pub unsafe extern "system" fn Java_app_squadair_mobile_core_SquadNativeBridge_nativeInitContext(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    android_logger::init_once(
        Config::default()
            .with_max_level(LevelFilter::Info)
            .with_tag("SquadAirRust"),
    );

    let Ok(jvm) = env.get_java_vm() else {
        log::error!("Failed to acquire JavaVM in nativeInitContext");
        return;
    };
    let Ok(global_ref) = env.new_global_ref(&context) else {
        log::error!("Failed to promote Application Context to GlobalRef");
        return;
    };

    let stored = APP_CONTEXT_REF.get_or_init(|| global_ref);
    NDK_CONTEXT_ONCE.call_once(|| {
        let vm_ptr = jvm.get_java_vm_pointer();
        unsafe {
            ndk_context::initialize_android_context(
                vm_ptr.cast(),
                stored.as_obj().as_raw().cast(),
            );
        }
        log::info!("Initialized ndk_context for SquadAir Rust core");
    });
}
