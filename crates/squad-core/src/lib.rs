pub mod avatar_core;
pub mod config;
pub mod crypto_identity;
pub mod mem;
pub mod net;
pub mod storage_paths;

#[cfg(feature = "android-ffi")]
pub mod android_jni;
#[cfg(feature = "android-ffi")]
pub mod android_video;
#[cfg(feature = "android-ffi")]
pub mod mobile_api;

#[cfg(feature = "android-ffi")]
uniffi::setup_scaffolding!();
