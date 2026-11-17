use cpal::traits::{DeviceTrait, HostTrait};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ResolvedDeviceMeta {
    pub resolved_name: Option<String>,
    pub sample_rate: u32,
    pub channels: usize,
    pub using_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ActiveTopologySnapshot {
    pub preferred_input: Option<String>,
    pub preferred_output: Option<String>,
    pub active_input: ResolvedDeviceMeta,
    pub active_output: ResolvedDeviceMeta,
    pub input_stream_ok: bool,
    pub output_stream_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ObservedTopologySide {
    pub all_names: Vec<String>,
    pub default_name: Option<String>,
    pub default_rate: u32,
    pub default_channels: usize,
    pub preferred_rate: Option<(u32, usize)>,
}

/// Pure policy deciding whether an input or output audio half needs rebuilding.
/// Designed so every OS device switch, unplug fallback, replug recovery, and BT HFP
/// sample-rate transition is deterministic and unit-testable without hardware.
pub(crate) fn should_rebuild_endpoint(
    preferred_name: Option<&str>,
    active: &ResolvedDeviceMeta,
    active_stream_ok: bool,
    observed: &ObservedTopologySide,
) -> bool {
    let Some(target) = preferred_name else {
        if !active_stream_ok {
            return observed.default_name.is_some();
        }
        if observed.default_name != active.resolved_name {
            return true;
        }
        return observed.default_name.is_some()
            && (observed.default_rate, observed.default_channels)
                != (active.sample_rate, active.channels);
    };

    let preferred_present = observed.all_names.iter().any(|n| n == target);
    if active.using_fallback && preferred_present {
        return true;
    }
    if !active.using_fallback && !preferred_present {
        return true;
    }
    if active.using_fallback && !preferred_present {
        if !active_stream_ok {
            return observed.default_name.is_some();
        }
        if observed.default_name != active.resolved_name {
            return true;
        }
        return observed.default_name.is_some()
            && (observed.default_rate, observed.default_channels)
                != (active.sample_rate, active.channels);
    }

    if !active_stream_ok {
        return true;
    }
    observed
        .preferred_rate
        .is_some_and(|rc| rc != (active.sample_rate, active.channels))
}

/// Caps repeated silence-triggered rebuilds on unchanged hardware (e.g. muted virtual drivers)
/// so an idle input device does not cause continuous stream teardown loops.
pub(crate) struct SilenceRetryGuard {
    consecutive_rebuilds: u8,
    max_rebuilds_per_stall: u8,
}

impl SilenceRetryGuard {
    pub fn new(max_rebuilds_per_stall: u8) -> Self {
        Self {
            consecutive_rebuilds: 0,
            max_rebuilds_per_stall,
        }
    }

    pub fn on_audio_received(&mut self) {
        self.consecutive_rebuilds = 0;
    }

    pub fn on_topology_changed(&mut self) {
        self.consecutive_rebuilds = 0;
    }

    pub fn try_consume_rebuild(&mut self) -> bool {
        if self.consecutive_rebuilds < self.max_rebuilds_per_stall {
            self.consecutive_rebuilds = self.consecutive_rebuilds.saturating_add(1);
            true
        } else {
            false
        }
    }
}

pub(crate) fn select_device_with_meta(
    host: &cpal::Host,
    preferred_name: Option<&str>,
    is_input: bool,
) -> (Option<cpal::Device>, Option<String>, bool) {
    if let Some(target) = preferred_name {
        let devices = if is_input {
            host.input_devices().ok()
        } else {
            host.output_devices().ok()
        };
        if let Some(mut iter) = devices {
            if let Some(found) = iter.find(|d| d.name().map(|n| n == target).unwrap_or(false)) {
                let name = found.name().ok().or_else(|| Some(target.to_string()));
                return (Some(found), name, false);
            }
        }
    }

    let fallback = if is_input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    let resolved_name = fallback.as_ref().and_then(|d| d.name().ok());
    let using_fallback = preferred_name.is_some();
    (fallback, resolved_name, using_fallback)
}

pub(crate) fn observe_topology_side<F>(
    host: &cpal::Host,
    preferred_name: Option<&str>,
    is_input: bool,
    negotiate_fn: F,
) -> ObservedTopologySide
where
    F: Fn(&cpal::Device, bool) -> (u32, usize),
{
    let mut all_names = Vec::new();
    let mut preferred_rate = None;

    let devices = if is_input {
        host.input_devices().ok()
    } else {
        host.output_devices().ok()
    };
    if let Some(iter) = devices {
        for dev in iter {
            if let Ok(name) = dev.name() {
                if preferred_name == Some(name.as_str()) {
                    preferred_rate = Some(negotiate_fn(&dev, is_input));
                }
                all_names.push(name);
            }
        }
    }
    all_names.sort();

    let default_dev = if is_input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    let default_name = default_dev.as_ref().and_then(|d| d.name().ok());
    let (default_rate, default_channels) = default_dev
        .as_ref()
        .map(|d| negotiate_fn(d, is_input))
        .unwrap_or((crate::SAMPLE_RATE, if is_input { 1 } else { 2 }));

    ObservedTopologySide {
        all_names,
        default_name,
        default_rate,
        default_channels,
        preferred_rate,
    }
}

pub(crate) enum WatcherSignal {
    HotplugSwap {
        rebuild_input: bool,
        rebuild_output: bool,
        devices_changed: bool,
    },
}

#[cfg(target_os = "macos")]
mod macos_ca {
    use std::ffi::c_void;
    use std::thread::Thread;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
    const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;
    const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = u32::from_be_bytes(*b"dev#");
    const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = u32::from_be_bytes(*b"dIn ");
    const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE: u32 = u32::from_be_bytes(*b"dOut");

    type AudioObjectPropertyListenerProc =
        extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut c_void) -> i32;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(
            in_object_id: u32,
            in_address: *const AudioObjectPropertyAddress,
            in_listener: AudioObjectPropertyListenerProc,
            in_client_data: *mut c_void,
        ) -> i32;

        fn AudioObjectRemovePropertyListener(
            in_object_id: u32,
            in_address: *const AudioObjectPropertyAddress,
            in_listener: AudioObjectPropertyListenerProc,
            in_client_data: *mut c_void,
        ) -> i32;
    }

    extern "C" fn listener_cb(
        _in_object_id: u32,
        _in_number_addresses: u32,
        _in_addresses: *const AudioObjectPropertyAddress,
        in_client_data: *mut c_void,
    ) -> i32 {
        if !in_client_data.is_null() {
            // Only unpark the dedicated watcher thread; never call CoreAudio APIs inside HAL callback.
            let th = unsafe { &*(in_client_data as *const Thread) };
            th.unpark();
        }
        0
    }

    pub struct CoreAudioListenerGuard {
        client_data: *mut Thread,
        addrs: [AudioObjectPropertyAddress; 3],
    }

    impl CoreAudioListenerGuard {
        pub fn register(thread: Thread) -> Self {
            let client_data = Box::into_raw(Box::new(thread));
            let addrs = [
                AudioObjectPropertyAddress {
                    m_selector: K_AUDIO_HARDWARE_PROPERTY_DEVICES,
                    m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
                    m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
                },
                AudioObjectPropertyAddress {
                    m_selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE,
                    m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
                    m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
                },
                AudioObjectPropertyAddress {
                    m_selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_OUTPUT_DEVICE,
                    m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
                    m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
                },
            ];
            for addr in &addrs {
                unsafe {
                    AudioObjectAddPropertyListener(
                        K_AUDIO_OBJECT_SYSTEM_OBJECT,
                        addr,
                        listener_cb,
                        client_data as *mut c_void,
                    );
                }
            }
            Self { client_data, addrs }
        }
    }

    impl Drop for CoreAudioListenerGuard {
        fn drop(&mut self) {
            for addr in &self.addrs {
                unsafe {
                    AudioObjectRemovePropertyListener(
                        K_AUDIO_OBJECT_SYSTEM_OBJECT,
                        addr,
                        listener_cb,
                        self.client_data as *mut c_void,
                    );
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            unsafe {
                drop(Box::from_raw(self.client_data));
            }
        }
    }
}

pub(crate) fn spawn_hotplug_watcher<R, A, F, S>(
    is_running: R,
    is_streams_active: A,
    topology_snap: Arc<Mutex<ActiveTopologySnapshot>>,
    send_signal: S,
    negotiate_fn: F,
) -> Option<thread::JoinHandle<()>>
where
    R: Fn() -> bool + Send + 'static,
    A: Fn() -> bool + Send + 'static,
    F: Fn(&cpal::Device, bool) -> (u32, usize) + Send + 'static,
    S: Fn(WatcherSignal) + Send + 'static,
{
    thread::Builder::new()
        .name("squad-audio-hotplug".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            let _ca_guard = macos_ca::CoreAudioListenerGuard::register(thread::current());

            let host = cpal::default_host();
            let mut last_in_names: Option<Vec<String>> = None;
            let mut last_out_names: Option<Vec<String>> = None;
            let mut last_swap_sent = std::time::Instant::now() - Duration::from_secs(5);

            while is_running() {
                if !is_streams_active() {
                    last_in_names = None;
                    last_out_names = None;
                    thread::park();
                    continue;
                }

                // Short debounce lets CoreAudio/WASAPI finish BT HFP profile transitions.
                thread::sleep(Duration::from_millis(60));
                if !is_running() || !is_streams_active() {
                    continue;
                }

                let snap = match topology_snap.lock() {
                    Ok(guard) => guard.clone(),
                    Err(poisoned) => poisoned.into_inner().clone(),
                };

                let obs_in = observe_topology_side(
                    &host,
                    snap.preferred_input.as_deref(),
                    true,
                    &negotiate_fn,
                );
                let obs_out = observe_topology_side(
                    &host,
                    snap.preferred_output.as_deref(),
                    false,
                    &negotiate_fn,
                );

                let list_changed = last_in_names
                    .as_ref()
                    .is_some_and(|prev| prev != &obs_in.all_names)
                    || last_out_names
                        .as_ref()
                        .is_some_and(|prev| prev != &obs_out.all_names);
                last_in_names = Some(obs_in.all_names.clone());
                last_out_names = Some(obs_out.all_names.clone());

                let rebuild_input = should_rebuild_endpoint(
                    snap.preferred_input.as_deref(),
                    &snap.active_input,
                    snap.input_stream_ok,
                    &obs_in,
                );
                let rebuild_output = should_rebuild_endpoint(
                    snap.preferred_output.as_deref(),
                    &snap.active_output,
                    snap.output_stream_ok,
                    &obs_out,
                );

                if (rebuild_input || rebuild_output || list_changed)
                    && last_swap_sent.elapsed() >= Duration::from_millis(450)
                {
                    last_swap_sent = std::time::Instant::now();
                    send_signal(WatcherSignal::HotplugSwap {
                        rebuild_input,
                        rebuild_output,
                        devices_changed: list_changed || rebuild_input || rebuild_output,
                    });
                }

                let poll_interval = if cfg!(target_os = "macos") {
                    Duration::from_millis(1200)
                } else {
                    Duration::from_millis(900)
                };
                thread::park_timeout(poll_interval);
            }
        })
        .ok()
}

pub(crate) fn send_hotplug_cmd<T>(tx: &SyncSender<T>, cmd: T) {
    let _ = tx.try_send(cmd);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(
        resolved_name: Option<&str>,
        sample_rate: u32,
        channels: usize,
        using_fallback: bool,
    ) -> ResolvedDeviceMeta {
        ResolvedDeviceMeta {
            resolved_name: resolved_name.map(str::to_string),
            sample_rate,
            channels,
            using_fallback,
        }
    }

    fn observed(
        all: &[&str],
        default_name: Option<&str>,
        default_rate: u32,
        default_channels: usize,
        preferred_rate: Option<(u32, usize)>,
    ) -> ObservedTopologySide {
        ObservedTopologySide {
            all_names: all.iter().map(|s| (*s).to_string()).collect(),
            default_name: default_name.map(str::to_string),
            default_rate,
            default_channels,
            preferred_rate,
        }
    }

    #[test]
    fn test_system_default_switches_when_os_default_changes_or_bt_rate_shifts() {
        let active = meta(Some("MacBook Pro Speakers"), 48_000, 2, false);
        let unchanged = observed(
            &["MacBook Pro Speakers"],
            Some("MacBook Pro Speakers"),
            48_000,
            2,
            None,
        );
        assert!(!should_rebuild_endpoint(None, &active, true, &unchanged));

        let airpods_default = observed(
            &["AirPods Pro", "MacBook Pro Speakers"],
            Some("AirPods Pro"),
            48_000,
            2,
            None,
        );
        assert!(should_rebuild_endpoint(
            None,
            &active,
            true,
            &airpods_default
        ));

        // Bluetooth switches from A2DP 48kHz stereo to HFP 24kHz mono with same name.
        let bt_active = meta(Some("AirPods Pro"), 48_000, 2, false);
        let bt_hfp = observed(&["AirPods Pro"], Some("AirPods Pro"), 24_000, 1, None);
        assert!(should_rebuild_endpoint(None, &bt_active, true, &bt_hfp));
    }

    #[test]
    fn test_preferred_device_unplug_fallback_and_replug_recovery() {
        let headset = "HyperX Cloud";
        let active_headset = meta(Some(headset), 48_000, 2, false);

        // Headset still present at matching format -> no rebuild.
        let obs_present = observed(
            &[headset, "Realtek Audio"],
            Some("Realtek Audio"),
            48_000,
            2,
            Some((48_000, 2)),
        );
        assert!(!should_rebuild_endpoint(
            Some(headset),
            &active_headset,
            true,
            &obs_present
        ));

        // Headset unplugged -> immediately rebuild to fall back onto OS default.
        let obs_unplugged = observed(&["Realtek Audio"], Some("Realtek Audio"), 48_000, 2, None);
        assert!(should_rebuild_endpoint(
            Some(headset),
            &active_headset,
            true,
            &obs_unplugged
        ));

        // Currently running on fallback speakers -> stays put until headset returns or default changes.
        let active_fallback = meta(Some("Realtek Audio"), 48_000, 2, true);
        assert!(!should_rebuild_endpoint(
            Some(headset),
            &active_fallback,
            true,
            &obs_unplugged
        ));

        // Headset plugged back in -> immediately recover preferred device!
        assert!(should_rebuild_endpoint(
            Some(headset),
            &active_fallback,
            true,
            &obs_present
        ));
    }

    #[test]
    fn test_no_hardware_does_not_thrash_until_device_appears() {
        let no_dev = meta(None, 48_000, 1, false);
        let empty = observed(&[], None, 48_000, 1, None);
        assert!(!should_rebuild_endpoint(None, &no_dev, false, &empty));

        let usb_mic_added = observed(&["Blue Yeti"], Some("Blue Yeti"), 48_000, 1, None);
        assert!(should_rebuild_endpoint(
            None,
            &no_dev,
            false,
            &usb_mic_added
        ));
    }

    #[test]
    fn test_silence_retry_guard_caps_and_resets() {
        let mut guard = SilenceRetryGuard::new(2);
        assert!(guard.try_consume_rebuild());
        assert!(guard.try_consume_rebuild());
        assert!(!guard.try_consume_rebuild());

        guard.on_audio_received();
        assert!(guard.try_consume_rebuild());

        guard.on_topology_changed();
        assert!(guard.try_consume_rebuild());
        assert!(guard.try_consume_rebuild());
        assert!(!guard.try_consume_rebuild());
    }
}
