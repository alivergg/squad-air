use crate::aec::EchoCanceller;
use crate::codec::OpusEncoder10ms;
use crate::cues::{SoundCue, SoundCueBank};
use crate::denoise::{NoiseSuppressionMode, NoiseSuppressor};
use crate::hotplug::{
    select_device_with_meta, send_hotplug_cmd, spawn_hotplug_watcher, ActiveTopologySnapshot,
    ResolvedDeviceMeta, SilenceRetryGuard, WatcherSignal,
};
use crate::jitter::JitterProfile;
use crate::mixer::AudioMixer;
use crate::resampler::{CaptureResampler, PlaybackResampler};
use crate::{FRAME_SAMPLES, SAMPLE_RATE};
use anyhow::{Context, Result};
use bytes::Bytes;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dashmap::DashMap;
use ringbuf::{
    traits::{Consumer, Observer, Producer, Split},
    HeapRb,
};
use serde::{Deserialize, Serialize};
use squad_proto::PeerId;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
    mpsc::{self, Receiver, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum MicActivationMode {
    ToggleMute = 0,
    PushToTalk = 1,
    PushToMute = 2,
}

impl MicActivationMode {
    pub fn from_u8(val: u8) -> Self {
        match val {
            1 => Self::PushToTalk,
            2 => Self::PushToMute,
            _ => Self::ToggleMute,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerAudioStats {
    pub loss_pct: f32,
    pub is_speaking: bool,
}

#[derive(Debug, Clone)]
pub enum OutboundAudioFrame {
    Opus { seq_num: u64, opus_bytes: Bytes },
    DtxKeepalive { seq_num: u64 },
    StreamOpus { seq_num: u64, opus_bytes: Bytes },
}

#[derive(Debug, Clone)]
pub struct AudioEngineConfig {
    pub input_device_name: Option<String>,
    pub output_device_name: Option<String>,
    pub aec_enabled: bool,
    pub ns_mode: NoiseSuppressionMode,
    pub activation_mode: MicActivationMode,
    pub input_gain: f32,
    pub cue_gain: f32,
    pub vad_threshold_db: f32,
    pub bitrate_bps: i32,
    pub jitter_profile: JitterProfile,
}

impl Default for AudioEngineConfig {
    fn default() -> Self {
        Self {
            input_device_name: None,
            output_device_name: None,
            aec_enabled: false,
            ns_mode: NoiseSuppressionMode::Rnnoise,
            activation_mode: MicActivationMode::PushToMute,
            input_gain: 1.0,
            cue_gain: 1.0,
            vad_threshold_db: -45.0,
            bitrate_bps: 40_000,
            jitter_profile: JitterProfile::default(),
        }
    }
}

fn sanitize_cue_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 2.0)
    } else {
        1.0
    }
}

enum EngineCmd {
    PushRemoteOpus {
        peer_id: PeerId,
        seq_num: u64,
        opus_data: Vec<u8>,
    },
    PushRemoteStreamOpus {
        peer_id: PeerId,
        seq_num: u64,
        opus_data: Vec<u8>,
    },
    SetPeerGain {
        peer_id: PeerId,
        gain: f32,
    },
    SetStreamGain(f32),
    ClearStreamAudio,
    SetShareStreamAudio(bool),
    RemovePeer {
        peer_id: PeerId,
    },
    ClearPeers,
    SetAec(bool),
    SetNsMode(NoiseSuppressionMode),
    SetOpusBitrate(i32),
    SetJitterProfile(JitterProfile),
    ClearLoopback,
    SetAudioDevices {
        input_device_name: Option<String>,
        output_device_name: Option<String>,
    },
    HotplugSwap {
        rebuild_input: bool,
        rebuild_output: bool,
        devices_changed: bool,
    },
    SetStreamsActive(bool),
    PlayCue(SoundCue),
    Shutdown,
}

struct SharedAtomics {
    running: AtomicBool,
    streams_active: AtomicBool,
    monitoring_in_settings: AtomicBool,
    mic_loopback: AtomicBool,
    mic_muted: AtomicBool,
    deafened: AtomicBool,
    hotkey_active: AtomicBool,
    stream_audio_sharing: AtomicBool,
    activation_mode: AtomicU8,
    input_gain_bits: AtomicU32,
    cue_gain_bits: AtomicU32,
    vad_threshold_db_bits: AtomicU32,
    mic_level_db_bits: AtomicU32,
    jitter_target_ms: AtomicU32,
    local_speaking: AtomicBool,
    df3_available: AtomicBool,
    devices_dirty: AtomicBool,
    device_list_gen: AtomicU32,
}

#[derive(Clone)]
pub struct AudioEngineHandle {
    cmd_tx: SyncSender<EngineCmd>,
    atomics: Arc<SharedAtomics>,
    peer_stats: Arc<DashMap<PeerId, PeerAudioStats>>,
    hotplug_thread: Arc<Mutex<Option<thread::Thread>>>,
}

impl AudioEngineHandle {
    /// Запускает реальные потоки ввода/вывода `cpal` и выделенный поток `squad-dsp-worker`.
    pub fn start(
        config: AudioEngineConfig,
        outbound_tx: tokio::sync::mpsc::Sender<OutboundAudioFrame>,
    ) -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<EngineCmd>(512);
        let peer_stats = Arc::new(DashMap::new());
        let suppressor = NoiseSuppressor::new(config.ns_mode);
        let df3_avail = suppressor.is_df3_available();

        let atomics = Arc::new(SharedAtomics {
            running: AtomicBool::new(true),
            streams_active: AtomicBool::new(false),
            monitoring_in_settings: AtomicBool::new(false),
            mic_loopback: AtomicBool::new(false),
            mic_muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            hotkey_active: AtomicBool::new(false),
            stream_audio_sharing: AtomicBool::new(false),
            activation_mode: AtomicU8::new(config.activation_mode as u8),
            input_gain_bits: AtomicU32::new(config.input_gain.to_bits()),
            cue_gain_bits: AtomicU32::new(sanitize_cue_gain(config.cue_gain).to_bits()),
            vad_threshold_db_bits: AtomicU32::new(
                config.vad_threshold_db.clamp(-60.0, 0.0).to_bits(),
            ),
            mic_level_db_bits: AtomicU32::new((-60.0f32).to_bits()),
            jitter_target_ms: AtomicU32::new(config.jitter_profile.target_ms()),
            local_speaking: AtomicBool::new(false),
            df3_available: AtomicBool::new(df3_avail),
            devices_dirty: AtomicBool::new(false),
            device_list_gen: AtomicU32::new(1),
        });

        let topology_snap = Arc::new(Mutex::new(ActiveTopologySnapshot {
            preferred_input: config.input_device_name.clone(),
            preferred_output: config.output_device_name.clone(),
            ..Default::default()
        }));

        let watcher_atomics_run = Arc::clone(&atomics);
        let watcher_atomics_active = Arc::clone(&atomics);
        let watcher_snap = Arc::clone(&topology_snap);
        let watcher_cmd_tx = cmd_tx.clone();
        let watcher_handle = spawn_hotplug_watcher(
            move || watcher_atomics_run.running.load(Ordering::Acquire),
            move || {
                watcher_atomics_active
                    .streams_active
                    .load(Ordering::Acquire)
            },
            watcher_snap,
            move |signal| match signal {
                WatcherSignal::HotplugSwap {
                    rebuild_input,
                    rebuild_output,
                    devices_changed,
                } => {
                    send_hotplug_cmd(
                        &watcher_cmd_tx,
                        EngineCmd::HotplugSwap {
                            rebuild_input,
                            rebuild_output,
                            devices_changed,
                        },
                    );
                }
            },
            negotiate_device_format,
        );
        let hotplug_thread = Arc::new(Mutex::new(
            watcher_handle.as_ref().map(|h| h.thread().clone()),
        ));

        let atomics_worker = Arc::clone(&atomics);
        let stats_worker = Arc::clone(&peer_stats);
        let hotplug_thread_worker = Arc::clone(&hotplug_thread);

        thread::Builder::new()
            .name("squad-dsp-worker".into())
            .spawn(move || {
                run_audio_worker(
                    config,
                    suppressor,
                    cmd_rx,
                    outbound_tx,
                    atomics_worker,
                    stats_worker,
                    topology_snap,
                    hotplug_thread_worker,
                );
            })
            .context("failed to spawn squad-dsp-worker thread")?;

        Ok(Self {
            cmd_tx,
            atomics,
            peer_stats,
            hotplug_thread,
        })
    }

    pub fn push_remote_opus(&self, peer_id: PeerId, seq_num: u64, opus_payload: &[u8]) {
        let _ = self.cmd_tx.try_send(EngineCmd::PushRemoteOpus {
            peer_id,
            seq_num,
            opus_data: opus_payload.to_vec(),
        });
    }

    pub fn push_remote_stream_opus(&self, peer_id: PeerId, seq_num: u64, opus_payload: &[u8]) {
        let _ = self.cmd_tx.try_send(EngineCmd::PushRemoteStreamOpus {
            peer_id,
            seq_num,
            opus_data: opus_payload.to_vec(),
        });
    }

    pub fn set_stream_gain(&self, gain: f32) {
        let _ = self.cmd_tx.try_send(EngineCmd::SetStreamGain(gain));
    }

    pub fn clear_stream_audio(&self) {
        let _ = self.cmd_tx.try_send(EngineCmd::ClearStreamAudio);
    }

    pub fn set_share_stream_audio(&self, enabled: bool) {
        self.atomics
            .stream_audio_sharing
            .store(enabled, Ordering::Relaxed);
        let _ = self
            .cmd_tx
            .try_send(EngineCmd::SetShareStreamAudio(enabled));
    }

    pub fn set_peer_gain(&self, peer_id: PeerId, gain: f32) {
        let _ = self
            .cmd_tx
            .try_send(EngineCmd::SetPeerGain { peer_id, gain });
    }

    pub fn remove_peer(&self, peer_id: PeerId) {
        self.peer_stats.remove(&peer_id);
        let _ = self.cmd_tx.try_send(EngineCmd::RemovePeer { peer_id });
    }

    pub fn clear_peers(&self) {
        self.peer_stats.clear();
        let _ = self.cmd_tx.try_send(EngineCmd::ClearPeers);
    }

    pub fn set_aec_enabled(&self, enabled: bool) {
        let _ = self.cmd_tx.try_send(EngineCmd::SetAec(enabled));
    }

    pub fn set_ns_mode(&self, mode: NoiseSuppressionMode) {
        let _ = self.cmd_tx.try_send(EngineCmd::SetNsMode(mode));
    }

    pub fn set_opus_bitrate(&self, bitrate_bps: i32) {
        let _ = self.cmd_tx.try_send(EngineCmd::SetOpusBitrate(bitrate_bps));
    }

    pub fn set_jitter_profile(&self, profile: JitterProfile) {
        self.atomics
            .jitter_target_ms
            .store(profile.target_ms(), Ordering::Relaxed);
        let _ = self.cmd_tx.try_send(EngineCmd::SetJitterProfile(profile));
    }

    pub fn jitter_target_ms(&self) -> u32 {
        self.atomics.jitter_target_ms.load(Ordering::Relaxed)
    }

    pub fn set_mic_loopback(&self, enabled: bool) {
        self.atomics.mic_loopback.store(enabled, Ordering::Relaxed);
        if !enabled {
            let _ = self.cmd_tx.try_send(EngineCmd::ClearLoopback);
        }
    }

    pub fn is_mic_loopback(&self) -> bool {
        self.atomics.mic_loopback.load(Ordering::Relaxed)
    }

    pub fn play_cue(&self, cue: SoundCue) {
        if self.cue_gain() <= 0.0001 {
            return;
        }
        let _ = self.cmd_tx.try_send(EngineCmd::PlayCue(cue));
    }

    pub fn set_cue_gain(&self, gain: f32) {
        self.atomics
            .cue_gain_bits
            .store(sanitize_cue_gain(gain).to_bits(), Ordering::Relaxed);
    }

    pub fn cue_gain(&self) -> f32 {
        f32::from_bits(self.atomics.cue_gain_bits.load(Ordering::Relaxed))
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.atomics.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn is_mic_muted(&self) -> bool {
        self.atomics.mic_muted.load(Ordering::Relaxed)
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.atomics.deafened.store(deafened, Ordering::Relaxed);
    }

    pub fn is_deafened(&self) -> bool {
        self.atomics.deafened.load(Ordering::Relaxed)
    }

    /// Вызывается обработчиком глобальных хоткеев при нажатии/отпускании клавиши PTT или PTM.
    pub fn set_hotkey_held(&self, held: bool) {
        self.atomics.hotkey_active.store(held, Ordering::Relaxed);
    }

    pub fn set_activation_mode(&self, mode: MicActivationMode) {
        self.atomics
            .activation_mode
            .store(mode as u8, Ordering::Relaxed);
    }

    pub fn activation_mode(&self) -> MicActivationMode {
        MicActivationMode::from_u8(self.atomics.activation_mode.load(Ordering::Relaxed))
    }

    pub fn set_input_gain(&self, gain: f32) {
        if !gain.is_finite() {
            return;
        }
        self.atomics
            .input_gain_bits
            .store(gain.clamp(0.0, 2.5).to_bits(), Ordering::Relaxed);
    }

    pub fn set_vad_threshold_db(&self, threshold_db: f32) {
        if !threshold_db.is_finite() {
            return;
        }
        self.atomics
            .vad_threshold_db_bits
            .store(threshold_db.clamp(-60.0, 0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn vad_threshold_db(&self) -> f32 {
        f32::from_bits(self.atomics.vad_threshold_db_bits.load(Ordering::Relaxed))
    }

    pub fn local_mic_level_db(&self) -> f32 {
        f32::from_bits(self.atomics.mic_level_db_bits.load(Ordering::Relaxed))
    }

    pub fn is_local_speaking(&self) -> bool {
        self.atomics.local_speaking.load(Ordering::Relaxed)
    }

    pub fn is_df3_available(&self) -> bool {
        self.atomics.df3_available.load(Ordering::Relaxed)
    }

    pub fn peer_stats(&self, peer_id: PeerId) -> PeerAudioStats {
        self.peer_stats
            .get(&peer_id)
            .map(|r| *r.value())
            .unwrap_or_default()
    }

    pub fn shutdown(&self) {
        self.atomics.running.store(false, Ordering::Release);
        self.unpark_hotplug_watcher();
        let _ = self.cmd_tx.try_send(EngineCmd::Shutdown);
    }

    pub fn set_audio_devices(
        &self,
        input_device_name: Option<String>,
        output_device_name: Option<String>,
    ) {
        let _ = self.cmd_tx.try_send(EngineCmd::SetAudioDevices {
            input_device_name,
            output_device_name,
        });
    }

    pub fn set_streams_active(&self, active: bool) {
        let prev = self.atomics.streams_active.swap(active, Ordering::AcqRel);
        if prev != active {
            let _ = self.cmd_tx.try_send(EngineCmd::SetStreamsActive(active));
        }
    }

    pub fn take_devices_dirty(&self) -> bool {
        self.atomics.devices_dirty.swap(false, Ordering::AcqRel)
    }

    pub fn device_list_gen(&self) -> u32 {
        self.atomics.device_list_gen.load(Ordering::Acquire)
    }

    fn unpark_hotplug_watcher(&self) {
        if let Ok(guard) = self.hotplug_thread.lock() {
            if let Some(ref th) = *guard {
                th.unpark();
            }
        }
    }

    pub fn set_monitoring_in_settings(&self, enabled: bool) {
        self.atomics
            .monitoring_in_settings
            .store(enabled, Ordering::Relaxed);
    }
}

#[cfg(not(target_os = "android"))]
type PlatformInputStream = cpal::Stream;
#[cfg(not(target_os = "android"))]
type PlatformOutputStream = cpal::Stream;

#[cfg(target_os = "android")]
#[allow(dead_code)]
enum PlatformInputStream {
    AAudio(crate::android_audio::AAudioStreamHandle),
    Cpal(cpal::Stream),
}

#[cfg(target_os = "android")]
#[allow(dead_code)]
enum PlatformOutputStream {
    AAudio(crate::android_audio::AAudioStreamHandle),
    Cpal(cpal::Stream),
}

struct AudioStreams {
    _input_stream: Option<PlatformInputStream>,
    _output_stream: Option<PlatformOutputStream>,
    cap_cons: <HeapRb<f32> as Split>::Cons,
    play_prod: <HeapRb<f32> as Split>::Prod,
    cap_resampler: CaptureResampler,
    play_resampler: PlaybackResampler,
    input_stream_failed: Arc<AtomicBool>,
    output_stream_failed: Arc<AtomicBool>,
    input_meta: ResolvedDeviceMeta,
    output_meta: ResolvedDeviceMeta,
}

fn pick_48k_f32_channels<I>(ranges: I, preferred_channels: Option<u16>) -> Option<usize>
where
    I: Iterator<Item = cpal::SupportedStreamConfigRange>,
{
    let target_rate = cpal::SampleRate(SAMPLE_RATE);
    let mut fallback = None;
    for r in ranges {
        if r.sample_format() == cpal::SampleFormat::F32
            && r.min_sample_rate() <= target_rate
            && r.max_sample_rate() >= target_rate
        {
            if preferred_channels == Some(r.channels()) {
                return Some(r.channels() as usize);
            }
            if fallback.is_none() {
                fallback = Some(r.channels() as usize);
            }
        }
    }
    fallback
}

fn negotiate_device_format(dev: &cpal::Device, is_input: bool) -> (u32, usize) {
    let default_cfg = if is_input {
        dev.default_input_config().ok()
    } else {
        dev.default_output_config().ok()
    };

    if let Some(ref cfg) = default_cfg {
        if cfg.sample_rate().0 == SAMPLE_RATE
            && cfg.sample_format() == cpal::SampleFormat::F32
            && (!is_input || cfg.channels() == 1)
        {
            return (SAMPLE_RATE, (cfg.channels() as usize).max(1));
        }
    }

    let preferred_channels = if is_input {
        Some(1)
    } else {
        default_cfg.as_ref().map(|c| c.channels())
    };
    let f32_48k_channels = if is_input {
        dev.supported_input_configs()
            .ok()
            .and_then(|iter| pick_48k_f32_channels(iter, preferred_channels))
    } else {
        dev.supported_output_configs()
            .ok()
            .and_then(|iter| pick_48k_f32_channels(iter, preferred_channels))
    };

    if let Some(ch) = f32_48k_channels {
        return (SAMPLE_RATE, ch.max(1));
    }

    default_cfg
        .map(|c| {
            (
                c.sample_rate().0.clamp(8_000, 192_000),
                (c.channels() as usize).max(1),
            )
        })
        .unwrap_or((SAMPLE_RATE, if is_input { 1 } else { 2 }))
}

struct InputStreamHalf {
    stream: Option<PlatformInputStream>,
    cap_cons: <HeapRb<f32> as Split>::Cons,
    cap_resampler: CaptureResampler,
    failed: Arc<AtomicBool>,
    sample_rate: u32,
    channels: usize,
    meta: ResolvedDeviceMeta,
}

struct OutputStreamHalf {
    stream: Option<PlatformOutputStream>,
    play_prod: <HeapRb<f32> as Split>::Prod,
    play_resampler: PlaybackResampler,
    failed: Arc<AtomicBool>,
    sample_rate: u32,
    channels: usize,
    meta: ResolvedDeviceMeta,
}

fn build_input_stream_half(host: &cpal::Host, input_device_name: Option<&str>) -> InputStreamHalf {
    #[cfg(target_os = "android")]
    {
        use crate::android_audio::{
            open_aaudio_input, parse_android_device_spec, AndroidDeviceSpec,
        };

        let spec = parse_android_device_spec(input_device_name);
        if !matches!(spec, AndroidDeviceSpec::LegacyIdx) {
            let req_id = match spec {
                AndroidDeviceSpec::Id(id) => id,
                AndroidDeviceSpec::Default | AndroidDeviceSpec::LegacyIdx => 0,
            };
            let failed = Arc::new(AtomicBool::new(false));
            if let Some(built) = open_aaudio_input(req_id, Arc::clone(&failed)) {
                let resolved_name = if req_id > 0 && !built.using_fallback {
                    Some(format!("@id:{req_id}"))
                } else {
                    Some("default".to_string())
                };
                return InputStreamHalf {
                    stream: Some(PlatformInputStream::AAudio(built.stream)),
                    cap_cons: built.cap_cons,
                    cap_resampler: CaptureResampler::new(built.sample_rate),
                    failed,
                    sample_rate: built.sample_rate,
                    channels: built.channels,
                    meta: ResolvedDeviceMeta {
                        resolved_name,
                        sample_rate: built.sample_rate,
                        channels: built.channels,
                        using_fallback: built.using_fallback,
                    },
                };
            }
        }
    }

    let (in_device, resolved_name, using_fallback) =
        select_device_with_meta(host, input_device_name, true);
    let (raw_in_rate, raw_in_channels) = in_device
        .as_ref()
        .map(|d| negotiate_device_format(d, true))
        .unwrap_or((SAMPLE_RATE, 1));
    let in_rate = raw_in_rate.clamp(8_000, 192_000);
    let in_channels = raw_in_channels.max(1);

    let capture_rb = HeapRb::<f32>::new((in_rate as usize) / 2);
    let (mut cap_prod, cap_cons) = capture_rb.split();
    let failed = Arc::new(AtomicBool::new(false));
    let failed_cb = Arc::clone(&failed);

    let stream = in_device.and_then(|dev| {
        let stream_cfg = cpal::StreamConfig {
            channels: in_channels as u16,
            sample_rate: cpal::SampleRate(in_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        dev.build_input_stream(
            &stream_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if in_channels == 1 {
                    let _ = cap_prod.push_slice(data);
                } else {
                    for frame in data.chunks(in_channels) {
                        let mono = frame.iter().copied().sum::<f32>() / (frame.len() as f32);
                        let _ = cap_prod.try_push(mono);
                    }
                }
            },
            move |_err| {
                failed_cb.store(true, Ordering::Relaxed);
            },
            None,
        )
        .ok()
    });

    let stream = stream.and_then(|s| {
        if let Err(err) = s.play() {
            warn!("Audio input stream play failed: {err}");
            failed.store(true, Ordering::Relaxed);
            None
        } else {
            #[cfg(target_os = "android")]
            let wrapped = PlatformInputStream::Cpal(s);
            #[cfg(not(target_os = "android"))]
            let wrapped = s;
            Some(wrapped)
        }
    });

    InputStreamHalf {
        stream,
        cap_cons,
        cap_resampler: CaptureResampler::new(in_rate),
        failed,
        sample_rate: in_rate,
        channels: in_channels,
        meta: ResolvedDeviceMeta {
            resolved_name,
            sample_rate: in_rate,
            channels: in_channels,
            using_fallback,
        },
    }
}

fn build_output_stream_half(
    host: &cpal::Host,
    output_device_name: Option<&str>,
) -> OutputStreamHalf {
    #[cfg(target_os = "android")]
    {
        use crate::android_audio::{
            open_aaudio_output, parse_android_device_spec, AndroidDeviceSpec,
        };

        let spec = parse_android_device_spec(output_device_name);
        if !matches!(spec, AndroidDeviceSpec::LegacyIdx) {
            let req_id = match spec {
                AndroidDeviceSpec::Id(id) => id,
                AndroidDeviceSpec::Default | AndroidDeviceSpec::LegacyIdx => 0,
            };
            let failed = Arc::new(AtomicBool::new(false));
            if let Some(built) = open_aaudio_output(req_id, Arc::clone(&failed)) {
                let resolved_name = if req_id > 0 && !built.using_fallback {
                    Some(format!("@id:{req_id}"))
                } else {
                    Some("default".to_string())
                };
                return OutputStreamHalf {
                    stream: Some(PlatformOutputStream::AAudio(built.stream)),
                    play_prod: built.play_prod,
                    play_resampler: PlaybackResampler::new(built.sample_rate),
                    failed,
                    sample_rate: built.sample_rate,
                    channels: built.channels,
                    meta: ResolvedDeviceMeta {
                        resolved_name,
                        sample_rate: built.sample_rate,
                        channels: built.channels,
                        using_fallback: built.using_fallback,
                    },
                };
            }
        }
    }

    let (out_device, resolved_name, using_fallback) =
        select_device_with_meta(host, output_device_name, false);
    let (raw_out_rate, raw_out_channels) = out_device
        .as_ref()
        .map(|d| negotiate_device_format(d, false))
        .unwrap_or((SAMPLE_RATE, 2));
    let out_rate = raw_out_rate.clamp(8_000, 192_000);
    let out_channels = raw_out_channels.max(1);

    let playback_rb = HeapRb::<f32>::new((out_rate as usize) / 2);
    let (mut play_prod, mut play_cons) = playback_rb.split();

    let prefill_samples = (out_rate as usize / 100) * 3;
    let silence_prefill = vec![0.0f32; prefill_samples];
    let _ = play_prod.push_slice(&silence_prefill);

    let failed = Arc::new(AtomicBool::new(false));
    let failed_cb = Arc::clone(&failed);

    let stream = out_device.and_then(|dev| {
        let stream_cfg = cpal::StreamConfig {
            channels: out_channels as u16,
            sample_rate: cpal::SampleRate(out_rate),
            buffer_size: cpal::BufferSize::Default,
        };
        dev.build_output_stream(
            &stream_cfg,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                if out_channels == 1 {
                    let popped = play_cons.pop_slice(data);
                    data[popped..].fill(0.0);
                } else if out_channels == 2 {
                    for frame in data.chunks_mut(2) {
                        let sample = play_cons.try_pop().unwrap_or(0.0);
                        frame.fill(sample);
                    }
                } else {
                    for frame in data.chunks_mut(out_channels) {
                        let sample = play_cons.try_pop().unwrap_or(0.0);
                        if frame.len() >= 2 {
                            frame[0] = sample;
                            frame[1] = sample;
                            frame[2..].fill(0.0);
                        } else {
                            frame.fill(sample);
                        }
                    }
                }
            },
            move |_err| {
                failed_cb.store(true, Ordering::Relaxed);
            },
            None,
        )
        .ok()
    });

    let stream = stream.and_then(|s| {
        if let Err(err) = s.play() {
            warn!("Audio output stream play failed: {err}");
            failed.store(true, Ordering::Relaxed);
            None
        } else {
            #[cfg(target_os = "android")]
            let wrapped = PlatformOutputStream::Cpal(s);
            #[cfg(not(target_os = "android"))]
            let wrapped = s;
            Some(wrapped)
        }
    });

    OutputStreamHalf {
        stream,
        play_prod,
        play_resampler: PlaybackResampler::new(out_rate),
        failed,
        sample_rate: out_rate,
        channels: out_channels,
        meta: ResolvedDeviceMeta {
            resolved_name,
            sample_rate: out_rate,
            channels: out_channels,
            using_fallback,
        },
    }
}

fn build_audio_streams(
    host: &cpal::Host,
    input_device_name: Option<&str>,
    output_device_name: Option<&str>,
) -> AudioStreams {
    let input = build_input_stream_half(host, input_device_name);
    let output = build_output_stream_half(host, output_device_name);

    info!(
        "SquadAir DSP worker active (In: {:?} {} Hz/{}ch{}, Out: {:?} {} Hz/{}ch{})",
        input.meta.resolved_name,
        input.sample_rate,
        input.channels,
        if input.meta.using_fallback {
            " fallback"
        } else {
            ""
        },
        output.meta.resolved_name,
        output.sample_rate,
        output.channels,
        if output.meta.using_fallback {
            " fallback"
        } else {
            ""
        },
    );

    AudioStreams {
        _input_stream: input.stream,
        _output_stream: output.stream,
        cap_cons: input.cap_cons,
        play_prod: output.play_prod,
        cap_resampler: input.cap_resampler,
        play_resampler: output.play_resampler,
        input_stream_failed: input.failed,
        output_stream_failed: output.failed,
        input_meta: input.meta,
        output_meta: output.meta,
    }
}

#[allow(dead_code)]
enum LoopbackHandle {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    Cpal(cpal::Stream),
    #[cfg(target_os = "macos")]
    MacOs(crate::macos_loopback::MacOsLoopbackCapture),
    #[cfg(target_os = "windows")]
    WinProcess(crate::win_loopback::WinProcessLoopback),
}

struct LoopbackStream {
    _handle: LoopbackHandle,
    cons: <HeapRb<f32> as Split>::Cons,
    resampler: CaptureResampler,
    failed: Arc<AtomicBool>,
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_f32(data: &[f32], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    if ch == 1 {
        let _ = prod.push_slice(data);
    } else {
        for frame in data.chunks_exact(ch) {
            let sum: f32 = frame.iter().copied().sum();
            let _ = prod.try_push(sum / (ch as f32));
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_i16(data: &[i16], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    let scale = 1.0 / (32768.0 * (ch as f32));
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| s as f32).sum();
        let _ = prod.try_push(sum * scale);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_u16(data: &[u16], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    let scale = 1.0 / (32768.0 * (ch as f32));
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| (s as f32) - 32768.0).sum();
        let _ = prod.try_push(sum * scale);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_i32(data: &[i32], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    let scale = 1.0 / (2_147_483_648.0 * (ch as f32));
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| s as f32).sum();
        let _ = prod.try_push(sum * scale);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_i8(data: &[i8], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    let scale = 1.0 / (128.0 * (ch as f32));
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| s as f32).sum();
        let _ = prod.try_push(sum * scale);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux", test))]
fn push_downmixed_u8(data: &[u8], channels: usize, prod: &mut <HeapRb<f32> as Split>::Prod) {
    let ch = channels.max(1);
    let scale = 1.0 / (128.0 * (ch as f32));
    for frame in data.chunks_exact(ch) {
        let sum: f32 = frame.iter().map(|&s| (s as f32) - 128.0).sum();
        let _ = prod.try_push(sum * scale);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn build_cpal_loopback_from_device(
    dev: &cpal::Device,
    cfg: cpal::SupportedStreamConfig,
    label: &str,
) -> Option<LoopbackStream> {
    let lb_rate = cfg.sample_rate().0.clamp(8_000, 192_000);
    let lb_channels = (cfg.channels() as usize).max(1);
    let sample_format = cfg.sample_format();
    let stream_cfg: cpal::StreamConfig = cfg.into();

    let rb = HeapRb::<f32>::new((lb_rate as usize) / 2);
    let (mut prod, cons) = rb.split();
    let failed = Arc::new(AtomicBool::new(false));
    let failed_cb = Arc::clone(&failed);

    let err_cb = move |_err| {
        failed_cb.store(true, Ordering::Relaxed);
    };

    let build_res = match sample_format {
        cpal::SampleFormat::F32 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                push_downmixed_f32(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::I16 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                push_downmixed_i16(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::U16 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                push_downmixed_u16(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::I32 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[i32], _: &cpal::InputCallbackInfo| {
                push_downmixed_i32(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::I8 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[i8], _: &cpal::InputCallbackInfo| {
                push_downmixed_i8(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::U8 => dev.build_input_stream(
            &stream_cfg,
            move |data: &[u8], _: &cpal::InputCallbackInfo| {
                push_downmixed_u8(data, lb_channels, &mut prod);
            },
            err_cb,
            None,
        ),
        other => {
            warn!("{label} unsupported sample format: {other:?}");
            return None;
        }
    };

    let stream = match build_res {
        Ok(s) => s,
        Err(err) => {
            warn!("{label} build_input_stream failed ({lb_rate} Hz/{lb_channels}ch/{sample_format:?}): {err}");
            return None;
        }
    };
    if let Err(err) = stream.play() {
        warn!("{label} stream.play() failed: {err}");
        return None;
    }

    info!("{label} active ({lb_rate} Hz/{lb_channels}ch/{sample_format:?})");
    Some(LoopbackStream {
        _handle: LoopbackHandle::Cpal(stream),
        cons,
        resampler: CaptureResampler::new(lb_rate),
        failed,
    })
}

fn build_loopback_stream(
    host: &cpal::Host,
    output_device_name: Option<&str>,
) -> Option<LoopbackStream> {
    #[cfg(target_os = "macos")]
    {
        let _ = output_device_name;
        if let Some(native) = crate::macos_loopback::start_macos_native_loopback() {
            return Some(LoopbackStream {
                _handle: LoopbackHandle::MacOs(native.handle),
                cons: native.cons,
                resampler: native.resampler,
                failed: native.failed,
            });
        }
        let target_dev = host.input_devices().ok().and_then(|mut iter| {
            iter.find(|d| {
                d.name()
                    .map(|n| {
                        let low = n.to_ascii_lowercase();
                        low.contains("blackhole")
                            || low.contains("loopback")
                            || low.contains("soundflower")
                    })
                    .unwrap_or(false)
            })
        });
        let dev = target_dev?;
        let cfg = match dev.default_input_config() {
            Ok(c) => c,
            Err(err) => {
                warn!("macOS virtual loopback default_input_config failed: {err}");
                return None;
            }
        };
        build_cpal_loopback_from_device(&dev, cfg, "macOS virtual loopback")
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(proc_lb) = crate::win_loopback::start_windows_process_loopback() {
            return Some(LoopbackStream {
                _handle: LoopbackHandle::WinProcess(proc_lb.handle),
                cons: proc_lb.cons,
                resampler: proc_lb.resampler,
                failed: proc_lb.failed,
            });
        }
        let dev = host
            .default_output_device()
            .or_else(|| select_device_with_meta(host, output_device_name, false).0)?;
        let cfg = match dev.default_output_config() {
            Ok(c) => c,
            Err(err) => {
                warn!("Windows WASAPI loopback default_output_config failed: {err}");
                return None;
            }
        };
        build_cpal_loopback_from_device(&dev, cfg, "Windows WASAPI loopback")
    }

    #[cfg(target_os = "linux")]
    {
        let _ = output_device_name;
        let devs: Vec<cpal::Device> = host
            .input_devices()
            .ok()
            .map(|iter| iter.collect())
            .unwrap_or_default();
        let target_dev = devs
            .iter()
            .find(|d| {
                d.name()
                    .map(|n| {
                        let low = n.to_ascii_lowercase();
                        low.contains(".monitor") || low.starts_with("monitor of ")
                    })
                    .unwrap_or(false)
            })
            .or_else(|| {
                devs.iter().find(|d| {
                    d.name()
                        .map(|n| {
                            let low = n.to_ascii_lowercase();
                            low.contains("loopback") || low.contains("monitor")
                        })
                        .unwrap_or(false)
                })
            });
        let dev = target_dev?;
        let cfg = match dev.default_input_config() {
            Ok(c) => c,
            Err(err) => {
                warn!("Linux monitor loopback default_input_config failed: {err}");
                return None;
            }
        };
        build_cpal_loopback_from_device(dev, cfg, "Linux monitor loopback")
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (host, output_device_name);
        None
    }
}

struct WorkerAudioCtx<'a> {
    host: &'a cpal::Host,
    atomics: &'a SharedAtomics,
    peer_stats: &'a Arc<DashMap<PeerId, PeerAudioStats>>,
    suppressor: &'a mut NoiseSuppressor,
    aec: &'a mut EchoCanceller,
    encoder: &'a mut OpusEncoder10ms,
    mixer: &'a mut AudioMixer,
    streams: &'a mut Option<AudioStreams>,
    stream_loopback: &'a mut Option<LoopbackStream>,
    current_input_dev: &'a mut Option<String>,
    current_output_dev: &'a mut Option<String>,
    silent_capture_ticks: &'a mut u32,
    silent_out_frames: &'a mut u8,
    last_hotplug_attempt: &'a mut Instant,
    error_backoff: &'a mut Duration,
    last_loopback_retry: &'a mut Instant,
    loopback_backoff: &'a mut Duration,
    silence_guard: &'a mut SilenceRetryGuard,
    topology_snap: &'a Arc<Mutex<ActiveTopologySnapshot>>,
}

fn sync_topology_snapshot(
    topology_snap: &Arc<Mutex<ActiveTopologySnapshot>>,
    preferred_input: &Option<String>,
    preferred_output: &Option<String>,
    streams: Option<&AudioStreams>,
) {
    if let Ok(mut guard) = topology_snap.lock() {
        guard.preferred_input = preferred_input.clone();
        guard.preferred_output = preferred_output.clone();
        if let Some(active) = streams {
            guard.active_input = active.input_meta.clone();
            guard.active_output = active.output_meta.clone();
            guard.input_stream_ok = active._input_stream.is_some()
                && !active.input_stream_failed.load(Ordering::Relaxed);
            guard.output_stream_ok = active._output_stream.is_some()
                && !active.output_stream_failed.load(Ordering::Relaxed);
        } else {
            guard.active_input = ResolvedDeviceMeta::default();
            guard.active_output = ResolvedDeviceMeta::default();
            guard.input_stream_ok = false;
            guard.output_stream_ok = false;
        }
    }
}

fn rebuild_input_half(
    host: &cpal::Host,
    active: &mut AudioStreams,
    preferred_input: Option<&str>,
    silent_capture_ticks: &mut u32,
) -> bool {
    drop(active._input_stream.take());
    let input = build_input_stream_half(host, preferred_input);
    let ok = input.stream.is_some();
    if ok {
        info!(
            "Hot-plug recovered audio input: {:?} ({} Hz/{}ch{})",
            input.meta.resolved_name,
            input.sample_rate,
            input.channels,
            if input.meta.using_fallback {
                " fallback"
            } else {
                ""
            }
        );
    }
    active._input_stream = input.stream;
    active.cap_cons = input.cap_cons;
    active.cap_resampler = input.cap_resampler;
    active.input_stream_failed = input.failed;
    active.input_meta = input.meta;
    *silent_capture_ticks = 0;
    ok
}

fn rebuild_output_half(
    host: &cpal::Host,
    active: &mut AudioStreams,
    preferred_output: Option<&str>,
    stream_loopback: &mut Option<LoopbackStream>,
    share_stream_audio: bool,
    silent_out_frames: &mut u8,
) -> bool {
    drop(stream_loopback.take());
    drop(active._output_stream.take());
    let output = build_output_stream_half(host, preferred_output);
    let ok = output.stream.is_some();
    if ok {
        info!(
            "Hot-plug recovered audio output: {:?} ({} Hz/{}ch{})",
            output.meta.resolved_name,
            output.sample_rate,
            output.channels,
            if output.meta.using_fallback {
                " fallback"
            } else {
                ""
            }
        );
    }
    active._output_stream = output.stream;
    active.play_prod = output.play_prod;
    active.play_resampler = output.play_resampler;
    active.output_stream_failed = output.failed;
    active.output_meta = output.meta;
    *silent_out_frames = 0;
    if share_stream_audio {
        *stream_loopback = build_loopback_stream(host, preferred_output);
    }
    ok
}

fn handle_engine_cmd(cmd: EngineCmd, ctx: &mut WorkerAudioCtx<'_>) -> bool {
    match cmd {
        EngineCmd::PushRemoteOpus {
            peer_id,
            seq_num,
            opus_data,
        } => ctx.mixer.push_peer_opus(peer_id, seq_num, opus_data),
        EngineCmd::PushRemoteStreamOpus {
            peer_id,
            seq_num,
            opus_data,
        } => ctx.mixer.push_stream_opus(peer_id, seq_num, opus_data),
        EngineCmd::SetPeerGain { peer_id, gain } => ctx.mixer.set_peer_gain(peer_id, gain),
        EngineCmd::SetStreamGain(gain) => ctx.mixer.set_stream_gain(gain),
        EngineCmd::ClearStreamAudio => ctx.mixer.clear_stream_audio(),
        EngineCmd::SetShareStreamAudio(enabled) => {
            if enabled && ctx.streams.is_some() {
                *ctx.stream_loopback =
                    build_loopback_stream(ctx.host, ctx.current_output_dev.as_deref());
                *ctx.last_loopback_retry = Instant::now();
                *ctx.loopback_backoff = Duration::from_secs(1);
                if ctx.stream_loopback.is_none() {
                    warn!("Screen-share audio loopback failed to start; auto-recovery scheduled");
                }
            } else {
                drop(ctx.stream_loopback.take());
                *ctx.loopback_backoff = Duration::from_secs(1);
            }
        }
        EngineCmd::RemovePeer { peer_id } => {
            ctx.mixer.remove_peer(peer_id);
            ctx.peer_stats.remove(&peer_id);
        }
        EngineCmd::ClearPeers => {
            ctx.mixer.clear_peers();
            ctx.peer_stats.clear();
        }
        EngineCmd::SetAec(enabled) => ctx.aec.set_enabled(enabled),
        EngineCmd::SetNsMode(mode) => ctx.suppressor.set_mode(mode),
        EngineCmd::SetOpusBitrate(bitrate_bps) => {
            if let Err(e) = ctx.encoder.set_bitrate(bitrate_bps) {
                warn!("Opus set_bitrate error: {e}");
            }
        }
        EngineCmd::SetJitterProfile(profile) => ctx.mixer.set_jitter_profile(profile),
        EngineCmd::ClearLoopback => ctx.mixer.clear_loopback(),
        EngineCmd::SetAudioDevices {
            input_device_name,
            output_device_name,
        } => {
            let input_changed = *ctx.current_input_dev != input_device_name;
            let output_changed = *ctx.current_output_dev != output_device_name;
            *ctx.current_input_dev = input_device_name.clone();
            *ctx.current_output_dev = output_device_name.clone();
            if let Some(active) = ctx.streams.as_mut() {
                let need_in = input_changed
                    || active._input_stream.is_none()
                    || active.input_stream_failed.load(Ordering::Relaxed);
                let need_out = output_changed
                    || active._output_stream.is_none()
                    || active.output_stream_failed.load(Ordering::Relaxed);
                if need_in {
                    rebuild_input_half(
                        ctx.host,
                        active,
                        ctx.current_input_dev.as_deref(),
                        ctx.silent_capture_ticks,
                    );
                }
                if need_out {
                    rebuild_output_half(
                        ctx.host,
                        active,
                        ctx.current_output_dev.as_deref(),
                        ctx.stream_loopback,
                        ctx.atomics.stream_audio_sharing.load(Ordering::Relaxed),
                        ctx.silent_out_frames,
                    );
                }
                if need_in || need_out {
                    *ctx.last_hotplug_attempt = Instant::now();
                    *ctx.error_backoff = Duration::from_secs(1);
                    ctx.silence_guard.on_topology_changed();
                }
            } else {
                *ctx.silent_capture_ticks = 0;
                *ctx.silent_out_frames = 0;
                *ctx.last_hotplug_attempt = Instant::now();
                *ctx.error_backoff = Duration::from_secs(1);
                ctx.silence_guard.on_topology_changed();
            }
            sync_topology_snapshot(
                ctx.topology_snap,
                ctx.current_input_dev,
                ctx.current_output_dev,
                ctx.streams.as_ref(),
            );
        }
        EngineCmd::HotplugSwap {
            rebuild_input,
            rebuild_output,
            devices_changed,
        } => {
            *ctx.error_backoff = Duration::from_secs(1);
            ctx.silence_guard.on_topology_changed();
            if devices_changed {
                ctx.atomics.devices_dirty.store(true, Ordering::Release);
                ctx.atomics.device_list_gen.fetch_add(1, Ordering::AcqRel);
            }
            if let Some(active) = ctx.streams.as_mut() {
                if rebuild_input {
                    rebuild_input_half(
                        ctx.host,
                        active,
                        ctx.current_input_dev.as_deref(),
                        ctx.silent_capture_ticks,
                    );
                }
                if rebuild_output {
                    rebuild_output_half(
                        ctx.host,
                        active,
                        ctx.current_output_dev.as_deref(),
                        ctx.stream_loopback,
                        ctx.atomics.stream_audio_sharing.load(Ordering::Relaxed),
                        ctx.silent_out_frames,
                    );
                }
                if rebuild_input || rebuild_output {
                    *ctx.last_hotplug_attempt = Instant::now();
                    sync_topology_snapshot(
                        ctx.topology_snap,
                        ctx.current_input_dev,
                        ctx.current_output_dev,
                        Some(active),
                    );
                }
            }
        }
        EngineCmd::SetStreamsActive(active) => {
            if !active {
                drop(ctx.stream_loopback.take());
            }
        }
        EngineCmd::PlayCue(cue) => {
            ctx.mixer.set_cue_gain(f32::from_bits(
                ctx.atomics.cue_gain_bits.load(Ordering::Relaxed),
            ));
            ctx.mixer.trigger_cue(cue);
        }
        EngineCmd::Shutdown => return false,
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn run_audio_worker(
    config: AudioEngineConfig,
    mut suppressor: NoiseSuppressor,
    cmd_rx: Receiver<EngineCmd>,
    outbound_tx: tokio::sync::mpsc::Sender<OutboundAudioFrame>,
    atomics: Arc<SharedAtomics>,
    peer_stats: Arc<DashMap<PeerId, PeerAudioStats>>,
    topology_snap: Arc<Mutex<ActiveTopologySnapshot>>,
    hotplug_thread: Arc<Mutex<Option<thread::Thread>>>,
) {
    let host = cpal::default_host();
    let mut streams: Option<AudioStreams> = None;
    let mut current_input_dev = config.input_device_name.clone();
    let mut current_output_dev = config.output_device_name.clone();
    let mut silent_capture_ticks: u32 = 0;
    let mut silent_out_frames: u8 = 0;
    let mut last_hotplug_attempt = Instant::now();
    let mut error_backoff = Duration::from_secs(1);
    let mut last_loopback_retry = Instant::now() - Duration::from_secs(10);
    let mut loopback_backoff = Duration::from_secs(1);
    let mut silence_guard = SilenceRetryGuard::new(2);
    let mut stream_loopback: Option<LoopbackStream> = None;
    let mut stream_encoder = OpusEncoder10ms::new_audio(96_000).expect("Stream Opus encoder init");
    let mut stream_seq_num: u64 = 1;
    let mut stream_frame_48k = [0.0f32; FRAME_SAMPLES];

    let mut aec = EchoCanceller::new(config.aec_enabled);
    let mut encoder = OpusEncoder10ms::new(config.bitrate_bps).expect("Opus encoder init");
    let cue_bank = SoundCueBank::load_optional();
    let mut mixer = AudioMixer::new(cue_bank, config.jitter_profile);
    mixer.set_cue_gain(f32::from_bits(
        atomics.cue_gain_bits.load(Ordering::Relaxed),
    ));

    let mut raw_in_buf = [0.0f32; 960];
    let mut frame_48k = [0.0f32; FRAME_SAMPLES];
    let mut mixed_48k = [0.0f32; FRAME_SAMPLES];
    let mut resampled_out = Vec::with_capacity(1024);
    let mut underrun_cushion = Vec::with_capacity(2048);

    let mut seq_num: u64 = 1;
    let mut dtx_seq_num: u64 = 1;
    let mut voice_hold_frames: u8 = 0;
    let mut smoothed_db: f32 = -60.0;
    let mut last_dtx_keepalive = Instant::now();
    let mut next_tick = Instant::now();
    let tick_duration = Duration::from_millis(10);

    while atomics.running.load(Ordering::Relaxed) {
        if streams.is_none()
            && !atomics.streams_active.load(Ordering::Relaxed)
            && !mixer.has_active_cues()
        {
            match cmd_rx.recv() {
                Ok(cmd) => {
                    let mut ctx = WorkerAudioCtx {
                        host: &host,
                        atomics: &atomics,
                        peer_stats: &peer_stats,
                        suppressor: &mut suppressor,
                        aec: &mut aec,
                        encoder: &mut encoder,
                        mixer: &mut mixer,
                        streams: &mut streams,
                        stream_loopback: &mut stream_loopback,
                        current_input_dev: &mut current_input_dev,
                        current_output_dev: &mut current_output_dev,
                        silent_capture_ticks: &mut silent_capture_ticks,
                        silent_out_frames: &mut silent_out_frames,
                        last_hotplug_attempt: &mut last_hotplug_attempt,
                        error_backoff: &mut error_backoff,
                        last_loopback_retry: &mut last_loopback_retry,
                        loopback_backoff: &mut loopback_backoff,
                        silence_guard: &mut silence_guard,
                        topology_snap: &topology_snap,
                    };
                    if !handle_engine_cmd(cmd, &mut ctx) {
                        return;
                    }
                }
                Err(_) => return,
            }
        }

        while let Ok(cmd) = cmd_rx.try_recv() {
            let mut ctx = WorkerAudioCtx {
                host: &host,
                atomics: &atomics,
                peer_stats: &peer_stats,
                suppressor: &mut suppressor,
                aec: &mut aec,
                encoder: &mut encoder,
                mixer: &mut mixer,
                streams: &mut streams,
                stream_loopback: &mut stream_loopback,
                current_input_dev: &mut current_input_dev,
                current_output_dev: &mut current_output_dev,
                silent_capture_ticks: &mut silent_capture_ticks,
                silent_out_frames: &mut silent_out_frames,
                last_hotplug_attempt: &mut last_hotplug_attempt,
                error_backoff: &mut error_backoff,
                last_loopback_retry: &mut last_loopback_retry,
                loopback_backoff: &mut loopback_backoff,
                silence_guard: &mut silence_guard,
                topology_snap: &topology_snap,
            };
            if !handle_engine_cmd(cmd, &mut ctx) {
                return;
            }
        }

        let should_run = atomics.streams_active.load(Ordering::Relaxed) || mixer.has_active_cues();
        if !should_run {
            if streams.is_some() {
                drop(stream_loopback.take());
                drop(streams.take());
                sync_topology_snapshot(
                    &topology_snap,
                    &current_input_dev,
                    &current_output_dev,
                    None,
                );
                atomics.local_speaking.store(false, Ordering::Relaxed);
                atomics
                    .mic_level_db_bits
                    .store((-60.0f32).to_bits(), Ordering::Relaxed);
            }
            continue;
        }

        if streams.is_none() {
            silent_capture_ticks = 0;
            silent_out_frames = 0;
            last_hotplug_attempt = Instant::now();
            error_backoff = Duration::from_secs(1);
            silence_guard.on_topology_changed();
            let built = build_audio_streams(
                &host,
                current_input_dev.as_deref(),
                current_output_dev.as_deref(),
            );
            sync_topology_snapshot(
                &topology_snap,
                &current_input_dev,
                &current_output_dev,
                Some(&built),
            );
            streams = Some(built);
            if let Ok(guard) = hotplug_thread.lock() {
                if let Some(ref th) = *guard {
                    th.unpark();
                }
            }
            if atomics.stream_audio_sharing.load(Ordering::Relaxed) {
                stream_loopback = build_loopback_stream(&host, current_output_dev.as_deref());
                last_loopback_retry = Instant::now();
                loopback_backoff = Duration::from_secs(1);
            }
            next_tick = Instant::now();
        }

        let Some(active) = streams.as_mut() else {
            continue;
        };

        let in_failed = active.input_stream_failed.load(Ordering::Relaxed);
        let out_failed = active.output_stream_failed.load(Ordering::Relaxed);
        let in_stalled = active._input_stream.is_some() && silent_capture_ticks >= 150;

        if (in_failed || out_failed || in_stalled)
            && last_hotplug_attempt.elapsed() >= error_backoff
        {
            let mut did_rebuild = false;
            let mut any_recovered = false;

            if in_failed || (in_stalled && silence_guard.try_consume_rebuild()) {
                did_rebuild = true;
                any_recovered |= rebuild_input_half(
                    &host,
                    active,
                    current_input_dev.as_deref(),
                    &mut silent_capture_ticks,
                );
            } else if in_stalled {
                silent_capture_ticks = 0;
            }

            if out_failed {
                did_rebuild = true;
                any_recovered |= rebuild_output_half(
                    &host,
                    active,
                    current_output_dev.as_deref(),
                    &mut stream_loopback,
                    atomics.stream_audio_sharing.load(Ordering::Relaxed),
                    &mut silent_out_frames,
                );
            }

            if did_rebuild {
                last_hotplug_attempt = Instant::now();
                error_backoff = if any_recovered {
                    Duration::from_secs(1)
                } else {
                    (error_backoff * 2).min(Duration::from_secs(8))
                };
                sync_topology_snapshot(
                    &topology_snap,
                    &current_input_dev,
                    &current_output_dev,
                    Some(active),
                );
            }
        }

        let deafened = atomics.deafened.load(Ordering::Relaxed);
        mixer.set_cue_gain(f32::from_bits(
            atomics.cue_gain_bits.load(Ordering::Relaxed),
        ));
        let has_audio = mixer.mix_frame(deafened, &mut mixed_48k);
        if has_audio || silent_out_frames < 4 {
            if has_audio {
                silent_out_frames = 0;
            } else {
                silent_out_frames = silent_out_frames.saturating_add(1);
            }
            aec.feed_render_frame(&mixed_48k);

            active
                .play_resampler
                .process_frame(&mixed_48k, &mut resampled_out);
            if active.play_prod.occupied_len() == 0 && !resampled_out.is_empty() {
                underrun_cushion.clear();
                underrun_cushion.resize(resampled_out.len() * 2, 0.0);
                let _ = active.play_prod.push_slice(&underrun_cushion);
            }
            if active.play_prod.occupied_len() <= resampled_out.len() * 8 {
                let _ = active.play_prod.push_slice(&resampled_out);
            } else {
                let frame_energy: f32 =
                    mixed_48k.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32);
                if frame_energy > 1e-6 {
                    let _ = active.play_prod.push_slice(&resampled_out);
                }
            }
        }

        for (peer_id, snap) in mixer.peer_snapshots() {
            peer_stats.insert(
                peer_id,
                PeerAudioStats {
                    loss_pct: snap.loss_pct,
                    is_speaking: snap.is_speaking,
                },
            );
        }

        let muted = atomics.mic_muted.load(Ordering::Relaxed);
        let hotkey_held = atomics.hotkey_active.load(Ordering::Relaxed);
        let mode = MicActivationMode::from_u8(atomics.activation_mode.load(Ordering::Relaxed));
        let monitoring = atomics.monitoring_in_settings.load(Ordering::Relaxed)
            || atomics.mic_loopback.load(Ordering::Relaxed);
        let gate_open = !muted
            && !deafened
            && match mode {
                MicActivationMode::ToggleMute => true,
                MicActivationMode::PushToTalk => hotkey_held,
                MicActivationMode::PushToMute => !hotkey_held,
            };

        let mut got_capture_samples = false;
        if !gate_open && !monitoring {
            while active.cap_cons.occupied_len() > 0 {
                let n = active.cap_cons.pop_slice(&mut raw_in_buf);
                if n > 0 {
                    got_capture_samples = true;
                }
            }
            voice_hold_frames = 0;
            smoothed_db = -60.0;
            atomics
                .mic_level_db_bits
                .store((-60.0f32).to_bits(), Ordering::Relaxed);
            atomics.local_speaking.store(false, Ordering::Relaxed);
            if mixer.has_peers() && last_dtx_keepalive.elapsed() >= Duration::from_millis(1500) {
                let current_dtx_seq = dtx_seq_num;
                dtx_seq_num = dtx_seq_num.wrapping_add(1);
                let _ = outbound_tx.try_send(OutboundAudioFrame::DtxKeepalive {
                    seq_num: current_dtx_seq,
                });
                last_dtx_keepalive = Instant::now();
            }
        } else {
            while active.cap_cons.occupied_len() > 0 {
                let n = active.cap_cons.pop_slice(&mut raw_in_buf);
                if n > 0 {
                    got_capture_samples = true;
                }
                active.cap_resampler.push_samples(&raw_in_buf[..n]);
            }

            while active.cap_resampler.pop_frame(&mut frame_48k) {
                let gain = f32::from_bits(atomics.input_gain_bits.load(Ordering::Relaxed));
                if (gain - 1.0).abs() > 0.001 {
                    for s in &mut frame_48k {
                        *s = (*s * gain).clamp(-1.0, 1.0);
                    }
                }

                let peak = frame_48k.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
                if peak > 1e-6 || aec.is_enabled() {
                    aec.process_capture_frame(&mut frame_48k);
                    suppressor.process_frame(&mut frame_48k);
                }

                let rms =
                    (frame_48k.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
                let raw_db = (20.0 * rms.max(1e-5).log10()).clamp(-60.0, 0.0);
                if raw_db > smoothed_db {
                    smoothed_db = raw_db;
                } else {
                    smoothed_db = smoothed_db * 0.82 + raw_db * 0.18;
                }
                atomics
                    .mic_level_db_bits
                    .store(smoothed_db.to_bits(), Ordering::Relaxed);

                let vad_threshold_db =
                    f32::from_bits(atomics.vad_threshold_db_bits.load(Ordering::Relaxed));
                if !muted && !deafened {
                    if raw_db >= vad_threshold_db || smoothed_db >= vad_threshold_db {
                        voice_hold_frames = 25;
                    } else {
                        voice_hold_frames = voice_hold_frames.saturating_sub(1);
                    }
                } else {
                    voice_hold_frames = 0;
                }

                if atomics.mic_loopback.load(Ordering::Relaxed)
                    && !muted
                    && !deafened
                    && voice_hold_frames > 0
                {
                    mixer.push_loopback_frame(&frame_48k);
                }

                let is_speaking_now = gate_open && voice_hold_frames > 0;
                atomics
                    .local_speaking
                    .store(is_speaking_now, Ordering::Relaxed);

                if is_speaking_now {
                    match encoder.encode_speech_frame(&frame_48k) {
                        Ok(opus_vec) => {
                            let current_seq = seq_num;
                            seq_num = seq_num.wrapping_add(1);
                            let _ = outbound_tx.try_send(OutboundAudioFrame::Opus {
                                seq_num: current_seq,
                                opus_bytes: Bytes::from(opus_vec),
                            });
                            last_dtx_keepalive = Instant::now();
                        }
                        Err(e) => warn!("Opus encode error: {e}"),
                    }
                } else if mixer.has_peers()
                    && last_dtx_keepalive.elapsed() >= Duration::from_millis(1500)
                {
                    let current_dtx_seq = dtx_seq_num;
                    dtx_seq_num = dtx_seq_num.wrapping_add(1);
                    let _ = outbound_tx.try_send(OutboundAudioFrame::DtxKeepalive {
                        seq_num: current_dtx_seq,
                    });
                    last_dtx_keepalive = Instant::now();
                }
            }
        }

        if got_capture_samples {
            silent_capture_ticks = 0;
            silence_guard.on_audio_received();
        } else {
            silent_capture_ticks = silent_capture_ticks.saturating_add(1);
        }

        if atomics.stream_audio_sharing.load(Ordering::Relaxed) {
            let needs_lb = stream_loopback
                .as_ref()
                .is_none_or(|lb| lb.failed.load(Ordering::Relaxed));
            if needs_lb && last_loopback_retry.elapsed() >= loopback_backoff {
                drop(stream_loopback.take());
                stream_loopback = build_loopback_stream(&host, current_output_dev.as_deref());
                last_loopback_retry = Instant::now();
                if stream_loopback.is_some() {
                    loopback_backoff = Duration::from_secs(1);
                } else {
                    loopback_backoff = (loopback_backoff * 2).min(Duration::from_secs(8));
                    warn!(
                        "Screen-share audio loopback unavailable; retrying in {:?}",
                        loopback_backoff
                    );
                }
            }
            if let Some(ref mut lb) = stream_loopback {
                while lb.cons.occupied_len() > 0 {
                    let n = lb.cons.pop_slice(&mut raw_in_buf);
                    lb.resampler.push_samples(&raw_in_buf[..n]);
                }
                while lb.resampler.pop_frame(&mut stream_frame_48k) {
                    let peak = stream_frame_48k.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
                    if peak > 1e-5 {
                        if let Ok(opus_vec) = stream_encoder.encode_speech_frame(&stream_frame_48k)
                        {
                            let cur_seq = stream_seq_num;
                            stream_seq_num = stream_seq_num.wrapping_add(1);
                            let _ = outbound_tx.try_send(OutboundAudioFrame::StreamOpus {
                                seq_num: cur_seq,
                                opus_bytes: Bytes::from(opus_vec),
                            });
                        }
                    }
                }
            }
        }

        // 4. Точный 10-мс таймер цикла микшера
        next_tick += tick_duration;
        let now = Instant::now();
        if next_tick > now {
            thread::sleep(next_tick - now);
        } else if now.duration_since(next_tick) > Duration::from_millis(50) {
            next_tick = now;
        }
    }
}

pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|iter| iter.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn list_output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|iter| iter.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_dsp_pipeline_sine_roundtrip_and_dtx() {
        let mut cap_resampler = CaptureResampler::new(44_100);
        let mut aec = EchoCanceller::new(true);
        let mut suppressor = NoiseSuppressor::new(NoiseSuppressionMode::Rnnoise);
        let mut encoder = OpusEncoder10ms::new(40_000).unwrap();
        let cue_bank = SoundCueBank::load_optional();
        let mut mixer = AudioMixer::new(cue_bank, JitterProfile::Balanced);

        // 1. Генерируем 200 мс чистой синусоиды 440 Гц на 44.1 кГц
        let total_in = 44_100 / 5;
        let mut sine_44k = Vec::with_capacity(total_in);
        for i in 0..total_in {
            let t = i as f32 / 44_100.0;
            sine_44k.push((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.5);
        }
        cap_resampler.push_samples(&sine_44k);

        let mut frame_48k = [0.0f32; FRAME_SAMPLES];
        let silence_render = [0.0f32; FRAME_SAMPLES];
        let mut opus_packets = Vec::new();

        while cap_resampler.pop_frame(&mut frame_48k) {
            aec.feed_render_frame(&silence_render);
            aec.process_capture_frame(&mut frame_48k);
            suppressor.process_frame(&mut frame_48k);

            if let Some(opus_bytes) = encoder.encode_frame(&frame_48k).unwrap() {
                opus_packets.push(opus_bytes);
            }
        }

        let encoded_packets = opus_packets.len();
        assert!(
            encoded_packets >= 15,
            "Expected at least 15 encoded 10ms frames, got {encoded_packets}"
        );

        // 2. Проверяем выход микшера и работу индивидуального ползунка громкости (150%)
        mixer.set_peer_gain(1, 1.5);
        let mut out_mixed = [0.0f32; FRAME_SAMPLES];
        let mut active_energy_frames = 0usize;

        for (idx, pkt) in opus_packets.into_iter().enumerate() {
            mixer.push_peer_opus(1, (idx + 1) as u64, pkt);
            if idx >= 1 {
                mixer.mix_frame(false, &mut out_mixed);
                let rms =
                    (out_mixed.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
                if rms > 0.01 {
                    active_energy_frames += 1;
                }
            }
        }
        assert!(
            active_energy_frames >= 10,
            "Expected audible mixed output frames, got {active_energy_frames}"
        );

        // 3. Проверяем DTX на абсолютной тишине (не должен генерировать пакеты)
        let silence_48k = [0.0f32; FRAME_SAMPLES];
        assert!(encoder.encode_frame(&silence_48k).unwrap().is_none());
    }

    #[test]
    fn test_stream_audio_opus_encode_jitter_mix_roundtrip() {
        let mut cap_resampler = CaptureResampler::new(44_100);
        let mut stream_encoder = OpusEncoder10ms::new_audio(96_000).unwrap();
        let cue_bank = SoundCueBank::load_optional();
        let mut mixer = AudioMixer::new(cue_bank, JitterProfile::Balanced);

        let total_in = 44_100 / 5;
        let mut music_44k = Vec::with_capacity(total_in);
        for i in 0..total_in {
            let t = i as f32 / 44_100.0;
            music_44k.push(
                0.35 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.20 * (2.0 * std::f32::consts::PI * 6_000.0 * t).sin(),
            );
        }
        cap_resampler.push_samples(&music_44k);

        let mut frame_48k = [0.0f32; FRAME_SAMPLES];
        let mut seq = 1u64;
        let mut encoded_count = 0usize;
        while cap_resampler.pop_frame(&mut frame_48k) {
            let pkt = stream_encoder.encode_speech_frame(&frame_48k).unwrap();
            let frame_msg = OutboundAudioFrame::StreamOpus {
                seq_num: seq,
                opus_bytes: Bytes::from(pkt),
            };
            if let OutboundAudioFrame::StreamOpus {
                seq_num,
                opus_bytes,
            } = frame_msg
            {
                mixer.push_stream_opus(1, seq_num, opus_bytes.to_vec());
                encoded_count += 1;
            }
            seq += 1;
        }
        assert!(encoded_count >= 18);

        let mut out = [0.0f32; FRAME_SAMPLES];
        let mut audible_frames = 0usize;
        for _ in 0..encoded_count {
            if mixer.mix_frame(false, &mut out) {
                let rms = (out.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32)).sqrt();
                if rms > 0.05 {
                    audible_frames += 1;
                }
            }
        }
        assert!(
            audible_frames >= 10,
            "Expected audible mixed stream audio frames, got {audible_frames}"
        );

        // Deafened must mute stream audio even when stream packets exist.
        let pkt_extra = stream_encoder.encode_speech_frame(&frame_48k).unwrap();
        mixer.push_stream_opus(1, seq, pkt_extra);
        assert!(!mixer.mix_frame(true, &mut out));
        assert!(out.iter().all(|&s| s == 0.0));

        // Zero stream gain silences stream audio.
        mixer.set_stream_gain(0.0);
        let pkt_extra2 = stream_encoder.encode_speech_frame(&frame_48k).unwrap();
        mixer.push_stream_opus(1, seq + 1, pkt_extra2);
        assert!(!mixer.mix_frame(false, &mut out));
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn test_loopback_sample_format_downmix_conversion() {
        let rb = HeapRb::<f32>::new(64);
        let (mut prod, mut cons) = rb.split();

        push_downmixed_f32(&[0.5, -0.5, 0.25, 0.75], 2, &mut prod);
        let mut out = [0.0f32; 2];
        assert_eq!(cons.pop_slice(&mut out), 2);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);

        push_downmixed_i16(&[16384, 16384, -32768, 0], 2, &mut prod);
        assert_eq!(cons.pop_slice(&mut out), 2);
        assert!((out[0] - 0.5).abs() < 1e-3);
        assert!((out[1] - (-0.5)).abs() < 1e-3);

        push_downmixed_u16(&[32768, 32768, 65535, 32768], 2, &mut prod);
        assert_eq!(cons.pop_slice(&mut out), 2);
        assert!((out[0] - 0.0).abs() < 1e-4);
        assert!((out[1] - 0.5).abs() < 1e-3);

        push_downmixed_i32(
            &[1_073_741_824, 1_073_741_824, -1_073_741_824, 0],
            2,
            &mut prod,
        );
        assert_eq!(cons.pop_slice(&mut out), 2);
        assert!((out[0] - 0.5).abs() < 1e-4);
        assert!((out[1] - (-0.25)).abs() < 1e-4);

        push_downmixed_i8(&[64, 64], 2, &mut prod);
        let mut one = [0.0f32; 1];
        assert_eq!(cons.pop_slice(&mut one), 1);
        assert!((one[0] - 0.5).abs() < 1e-2);

        push_downmixed_u8(&[128, 128], 2, &mut prod);
        assert_eq!(cons.pop_slice(&mut one), 1);
        assert!((one[0] - 0.0).abs() < 1e-2);
    }
}
