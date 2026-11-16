use crate::aec::EchoCanceller;
use crate::codec::OpusEncoder10ms;
use crate::cues::{SoundCue, SoundCueBank};
use crate::denoise::{NoiseSuppressionMode, NoiseSuppressor};
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
    Arc,
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
}

#[derive(Debug, Clone)]
pub struct AudioEngineConfig {
    pub input_device_name: Option<String>,
    pub output_device_name: Option<String>,
    pub aec_enabled: bool,
    pub ns_mode: NoiseSuppressionMode,
    pub activation_mode: MicActivationMode,
    pub input_gain: f32,
    pub vad_threshold_db: f32,
    pub bitrate_bps: i32,
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
            vad_threshold_db: -45.0,
            bitrate_bps: 40_000,
        }
    }
}

enum EngineCmd {
    PushRemoteOpus {
        peer_id: PeerId,
        seq_num: u64,
        opus_data: Vec<u8>,
    },
    SetPeerGain {
        peer_id: PeerId,
        gain: f32,
    },
    RemovePeer {
        peer_id: PeerId,
    },
    ClearPeers,
    SetAec(bool),
    SetNsMode(NoiseSuppressionMode),
    SetAudioDevices {
        input_device_name: Option<String>,
        output_device_name: Option<String>,
    },
    PlayCue(SoundCue),
    Shutdown,
}

struct SharedAtomics {
    running: AtomicBool,
    mic_muted: AtomicBool,
    deafened: AtomicBool,
    hotkey_active: AtomicBool,
    activation_mode: AtomicU8,
    input_gain_bits: AtomicU32,
    vad_threshold_db_bits: AtomicU32,
    mic_level_db_bits: AtomicU32,
    local_speaking: AtomicBool,
    df3_available: AtomicBool,
}

#[derive(Clone)]
pub struct AudioEngineHandle {
    cmd_tx: SyncSender<EngineCmd>,
    atomics: Arc<SharedAtomics>,
    peer_stats: Arc<DashMap<PeerId, PeerAudioStats>>,
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
            mic_muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            hotkey_active: AtomicBool::new(false),
            activation_mode: AtomicU8::new(config.activation_mode as u8),
            input_gain_bits: AtomicU32::new(config.input_gain.to_bits()),
            vad_threshold_db_bits: AtomicU32::new(
                config.vad_threshold_db.clamp(-60.0, 0.0).to_bits(),
            ),
            mic_level_db_bits: AtomicU32::new((-60.0f32).to_bits()),
            local_speaking: AtomicBool::new(false),
            df3_available: AtomicBool::new(df3_avail),
        });

        let atomics_worker = Arc::clone(&atomics);
        let stats_worker = Arc::clone(&peer_stats);

        thread::Builder::new()
            .name("squad-dsp-worker".into())
            .spawn(move || {
                run_audio_worker(config, suppressor, cmd_rx, outbound_tx, atomics_worker, stats_worker);
            })
            .context("failed to spawn squad-dsp-worker thread")?;

        Ok(Self {
            cmd_tx,
            atomics,
            peer_stats,
        })
    }

    pub fn push_remote_opus(&self, peer_id: PeerId, seq_num: u64, opus_payload: &[u8]) {
        let _ = self.cmd_tx.try_send(EngineCmd::PushRemoteOpus {
            peer_id,
            seq_num,
            opus_data: opus_payload.to_vec(),
        });
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

    pub fn play_cue(&self, cue: SoundCue) {
        let _ = self.cmd_tx.try_send(EngineCmd::PlayCue(cue));
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
        self.atomics
            .input_gain_bits
            .store(gain.clamp(0.0, 2.5).to_bits(), Ordering::Relaxed);
    }

    pub fn set_vad_threshold_db(&self, threshold_db: f32) {
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
        self.atomics.running.store(false, Ordering::Relaxed);
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
}

struct AudioStreams {
    _input_stream: Option<cpal::Stream>,
    _output_stream: Option<cpal::Stream>,
    cap_cons: <HeapRb<f32> as Split>::Cons,
    play_prod: <HeapRb<f32> as Split>::Prod,
    cap_resampler: CaptureResampler,
    play_resampler: PlaybackResampler,
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
        if cfg.sample_rate().0 == SAMPLE_RATE && cfg.sample_format() == cpal::SampleFormat::F32 {
            return (SAMPLE_RATE, cfg.channels() as usize);
        }
    }

    let preferred_channels = default_cfg.as_ref().map(|c| c.channels());
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
        return (SAMPLE_RATE, ch);
    }

    default_cfg
        .map(|c| (c.sample_rate().0, c.channels() as usize))
        .unwrap_or((SAMPLE_RATE, if is_input { 1 } else { 2 }))
}

fn build_audio_streams(
    host: &cpal::Host,
    input_device_name: Option<&str>,
    output_device_name: Option<&str>,
) -> AudioStreams {
    let in_device = select_device(host, input_device_name, true);
    let out_device = select_device(host, output_device_name, false);

    let (in_rate, in_channels) = in_device
        .as_ref()
        .map(|d| negotiate_device_format(d, true))
        .unwrap_or((SAMPLE_RATE, 1));

    let (out_rate, out_channels) = out_device
        .as_ref()
        .map(|d| negotiate_device_format(d, false))
        .unwrap_or((SAMPLE_RATE, 2));

    let capture_rb = HeapRb::<f32>::new((in_rate as usize) / 2);
    let (mut cap_prod, cap_cons) = capture_rb.split();

    let playback_rb = HeapRb::<f32>::new((out_rate as usize) / 2);
    let (mut play_prod, mut play_cons) = playback_rb.split();

    let prefill_samples = (out_rate as usize / 100) * 3;
    let silence_prefill = vec![0.0f32; prefill_samples];
    let _ = play_prod.push_slice(&silence_prefill);

    let _input_stream = in_device.and_then(|dev| {
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
            |err| {
                let _ = err;
            },
            None,
        )
        .ok()
    });

    let _output_stream = out_device.and_then(|dev| {
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
                } else {
                    for frame in data.chunks_mut(out_channels) {
                        let sample = play_cons.try_pop().unwrap_or(0.0);
                        frame.fill(sample);
                    }
                }
            },
            |err| {
                let _ = err;
            },
            None,
        )
        .ok()
    });

    if let Some(ref s) = _input_stream {
        let _ = s.play();
    }
    if let Some(ref s) = _output_stream {
        let _ = s.play();
    }

    info!(
        "Squad-Air DSP worker active (Capture: {} Hz/{}ch, Playback: {} Hz/{}ch)",
        in_rate, in_channels, out_rate, out_channels
    );

    AudioStreams {
        _input_stream,
        _output_stream,
        cap_cons,
        play_prod,
        cap_resampler: CaptureResampler::new(in_rate),
        play_resampler: PlaybackResampler::new(out_rate),
    }
}

fn run_audio_worker(
    config: AudioEngineConfig,
    mut suppressor: NoiseSuppressor,
    cmd_rx: Receiver<EngineCmd>,
    outbound_tx: tokio::sync::mpsc::Sender<OutboundAudioFrame>,
    atomics: Arc<SharedAtomics>,
    peer_stats: Arc<DashMap<PeerId, PeerAudioStats>>,
) {
    let host = cpal::default_host();
    let mut streams = Some(build_audio_streams(
        &host,
        config.input_device_name.as_deref(),
        config.output_device_name.as_deref(),
    ));
    let mut aec = EchoCanceller::new(config.aec_enabled);
    let mut encoder = OpusEncoder10ms::new(config.bitrate_bps).expect("Opus encoder init");
    let cue_bank = SoundCueBank::load_optional();
    let mut mixer = AudioMixer::new(cue_bank);

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
        // 1. Дренируем управляющие команды без блокировки
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                EngineCmd::PushRemoteOpus {
                    peer_id,
                    seq_num,
                    opus_data,
                } => mixer.push_peer_opus(peer_id, seq_num, opus_data),
                EngineCmd::SetPeerGain { peer_id, gain } => mixer.set_peer_gain(peer_id, gain),
                EngineCmd::RemovePeer { peer_id } => mixer.remove_peer(peer_id),
                EngineCmd::ClearPeers => mixer.clear_peers(),
                EngineCmd::SetAec(enabled) => aec.set_enabled(enabled),
                EngineCmd::SetNsMode(mode) => suppressor.set_mode(mode),
                EngineCmd::SetAudioDevices {
                    input_device_name,
                    output_device_name,
                } => {
                    // Drop existing streams first so OS backends can re-acquire the device cleanly.
                    drop(streams.take());
                    streams = Some(build_audio_streams(
                        &host,
                        input_device_name.as_deref(),
                        output_device_name.as_deref(),
                    ));
                }
                EngineCmd::PlayCue(cue) => mixer.trigger_cue(cue),
                EngineCmd::Shutdown => return,
            }
        }

        let Some(active) = streams.as_mut() else {
            continue;
        };

        // 2. Сводим выходной 10-мс кадр всех участников комнаты и пушим референс в AEC3
        let deafened = atomics.deafened.load(Ordering::Relaxed);
        mixer.mix_frame(deafened, &mut mixed_48k);
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

        for (peer_id, snap) in mixer.peer_snapshots() {
            peer_stats.insert(
                peer_id,
                PeerAudioStats {
                    loss_pct: snap.loss_pct,
                    is_speaking: snap.is_speaking,
                },
            );
        }

        // 3. Забираем сырые сэмплы с микрофона и прогоняем через Capture DSP-тракт
        while active.cap_cons.occupied_len() > 0 {
            let n = active.cap_cons.pop_slice(&mut raw_in_buf);
            active.cap_resampler.push_samples(&raw_in_buf[..n]);
        }

        while active.cap_resampler.pop_frame(&mut frame_48k) {
            let gain = f32::from_bits(atomics.input_gain_bits.load(Ordering::Relaxed));
            if (gain - 1.0).abs() > 0.001 {
                for s in &mut frame_48k {
                    *s = (*s * gain).clamp(-1.0, 1.0);
                }
            }

            // Строгий порядок: сначала AEC3, затем шумоподавление (RNNoise / DeepFilterNet3)
            aec.process_capture_frame(&mut frame_48k);
            suppressor.process_frame(&mut frame_48k);

            // Вычисляем уровень сигнала микрофона в dBFS со сглаживанием для UI и Voice Gate
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

            // Проверяем гейт микрофона (ToggleMute / PushToTalk / PushToMute) + Voice Sensitivity с удержанием
            let muted = atomics.mic_muted.load(Ordering::Relaxed);
            let hotkey_held = atomics.hotkey_active.load(Ordering::Relaxed);
            let mode = MicActivationMode::from_u8(atomics.activation_mode.load(Ordering::Relaxed));

            let gate_open = !muted
                && !deafened
                && match mode {
                    MicActivationMode::ToggleMute => true,
                    MicActivationMode::PushToTalk => hotkey_held,
                    MicActivationMode::PushToMute => !hotkey_held,
                };

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
            } else if last_dtx_keepalive.elapsed() >= Duration::from_millis(500) {
                let current_dtx_seq = dtx_seq_num;
                dtx_seq_num = dtx_seq_num.wrapping_add(1);
                let _ = outbound_tx.try_send(OutboundAudioFrame::DtxKeepalive {
                    seq_num: current_dtx_seq,
                });
                last_dtx_keepalive = Instant::now();
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

fn select_device(
    host: &cpal::Host,
    preferred_name: Option<&str>,
    is_input: bool,
) -> Option<cpal::Device> {
    if let Some(target) = preferred_name {
        let devices = if is_input {
            host.input_devices().ok()
        } else {
            host.output_devices().ok()
        };
        if let Some(mut iter) = devices {
            if let Some(found) = iter.find(|d| d.name().map(|n| n == target).unwrap_or(false)) {
                return Some(found);
            }
        }
    }
    if is_input {
        host.default_input_device()
    } else {
        host.default_output_device()
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
        let mut mixer = AudioMixer::new(cue_bank);

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
                let rms = (out_mixed.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32))
                    .sqrt();
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
}