use anyhow::Result;
use rand::Rng;
use serde::{Deserialize, Serialize};
use squad_dsp::{JitterProfile, MicActivationMode, NoiseSuppressionMode};
use squad_proto::ClientUuid;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

const ADJECTIVES: &[&str] = &[
    "Swift", "Silent", "Frost", "Shadow", "Cobalt", "Apex", "Nova", "Iron", "Rapid", "Solar",
];

const ANIMALS: &[&str] = &[
    "Falcon", "Wolf", "Viper", "Lynx", "Raven", "Hawk", "Fox", "Tiger", "Eagle", "Bear",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedChannel {
    pub name: String,
    pub invite_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub nickname: String,
    pub client_uuid: ClientUuid,
    pub rendezvous_server: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub aec_enabled: bool,
    pub ns_mode: NoiseSuppressionMode,
    pub activation_mode: MicActivationMode,
    pub input_gain: f32,
    #[serde(default = "default_vad_threshold_db")]
    pub vad_threshold_db: f32,
    pub hotkey_ptm: String,
    pub hotkey_ptt: String,
    pub hotkey_toggle_mute: String,
    #[serde(default = "default_hotkey_deafen")]
    pub hotkey_deafen: String,
    #[serde(default)]
    pub jitter_profile: JitterProfile,
    #[serde(default = "default_opus_bitrate_bps")]
    pub opus_bitrate_bps: i32,
    #[serde(default)]
    pub peer_volumes: HashMap<String, f32>,
    #[serde(default)]
    pub saved_channels: Vec<SavedChannel>,
    #[serde(default)]
    pub custom_avatar_hash: Option<String>,
    #[serde(default = "default_minimize_to_tray")]
    pub minimize_to_tray: bool,
    #[serde(default = "default_stream_volume")]
    pub stream_volume: f32,
    #[serde(default = "default_overlay_enabled")]
    pub overlay_enabled: bool,
    #[serde(default)]
    pub overlay_speaking_only: bool,
    #[serde(default = "default_overlay_opacity")]
    pub overlay_opacity: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nickname: generate_default_nickname(),
            client_uuid: *Uuid::new_v4().as_bytes(),
            rendezvous_server: "127.0.0.1:3478".to_string(),
            input_device: None,
            output_device: None,
            aec_enabled: false,
            ns_mode: NoiseSuppressionMode::Rnnoise,
            activation_mode: MicActivationMode::PushToMute,
            input_gain: 1.0,
            vad_threshold_db: default_vad_threshold_db(),
            hotkey_ptm: "KeyV".to_string(),
            hotkey_ptt: "CapsLock".to_string(),
            hotkey_toggle_mute: "F9".to_string(),
            hotkey_deafen: default_hotkey_deafen(),
            jitter_profile: JitterProfile::default(),
            opus_bitrate_bps: default_opus_bitrate_bps(),
            peer_volumes: HashMap::new(),
            saved_channels: Vec::new(),
            custom_avatar_hash: None,
            minimize_to_tray: default_minimize_to_tray(),
            stream_volume: default_stream_volume(),
            overlay_enabled: default_overlay_enabled(),
            overlay_speaking_only: false,
            overlay_opacity: default_overlay_opacity(),
        }
    }
}

impl AppConfig {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|mut dir| {
            dir.push("squad-air");
            dir.push("config.toml");
            dir
        })
    }

    pub fn load_or_create() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };

        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(cfg) = toml::from_str::<Self>(&contents) {
                    return cfg;
                }
                if !contents.trim().is_empty() {
                    let _ = fs::copy(&path, path.with_extension("toml.bak"));
                }
            }
        }

        let cfg = Self::default();
        let _ = cfg.save();
        cfg
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = Self::config_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
                fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
            #[cfg(not(unix))]
            {
                fs::create_dir_all(parent)?;
            }
        }
        let body = toml::to_string_pretty(self)?;
        let tmp_path = path.with_extension("toml.tmp");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        let write_res = (|| -> Result<()> {
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        drop(file);
        if let Err(err) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(err);
        }
        if let Err(err) = fs::rename(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(err.into());
        }
        Ok(())
    }

    pub fn client_uuid(&self) -> ClientUuid {
        self.client_uuid
    }

    pub fn peer_volume(&self, client_uuid: &ClientUuid) -> f32 {
        let key = uuid_key(client_uuid);
        self.peer_volumes.get(&key).copied().unwrap_or(1.0)
    }

    pub fn set_peer_volume(&mut self, client_uuid: &ClientUuid, gain: f32) {
        let key = uuid_key(client_uuid);
        self.peer_volumes.insert(key, gain.clamp(0.0, 2.0));
        let _ = self.save();
    }

    pub fn add_saved_channel(&mut self, name: String, invite_uri: String) {
        let trimmed_uri = invite_uri.trim().to_string();
        if trimmed_uri.is_empty() {
            return;
        }
        let clean_name = name.trim();
        let final_name = if clean_name.is_empty() {
            "Squad Channel".to_string()
        } else {
            clean_name.to_string()
        };
        if let Some(existing) = self
            .saved_channels
            .iter_mut()
            .find(|c| c.invite_uri == trimmed_uri)
        {
            existing.name = final_name;
        } else {
            self.saved_channels.push(SavedChannel {
                name: final_name,
                invite_uri: trimmed_uri,
            });
        }
        let _ = self.save();
    }

    pub fn remove_saved_channel(&mut self, invite_uri: &str) {
        let before = self.saved_channels.len();
        self.saved_channels.retain(|c| c.invite_uri != invite_uri);
        if self.saved_channels.len() != before {
            let _ = self.save();
        }
    }
}

fn default_vad_threshold_db() -> f32 {
    -45.0
}

fn default_hotkey_deafen() -> String {
    "F10".to_string()
}

fn default_opus_bitrate_bps() -> i32 {
    40_000
}

fn default_minimize_to_tray() -> bool {
    true
}

fn default_stream_volume() -> f32 {
    1.0
}

fn default_overlay_enabled() -> bool {
    true
}

fn default_overlay_opacity() -> f32 {
    0.82
}

fn uuid_key(client_uuid: &ClientUuid) -> String {
    Uuid::from_bytes(*client_uuid).to_string()
}

fn generate_default_nickname() -> String {
    let mut rng = rand::thread_rng();
    let adj = ADJECTIVES[rng.gen_range(0..ADJECTIVES.len())];
    let animal = ANIMALS[rng.gen_range(0..ANIMALS.len())];
    let digits = rng.gen_range(100..1000);
    format!("{adj}{animal}-{digits}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_config_overlay_defaults_and_serde_roundtrip() {
        let mut cfg = AppConfig::default();
        assert!(cfg.overlay_enabled);
        assert!(!cfg.overlay_speaking_only);
        assert!((cfg.overlay_opacity - 0.82).abs() < 1e-4);
        assert_eq!(cfg.hotkey_deafen, "F10");
        assert_eq!(cfg.opus_bitrate_bps, 40_000);
        assert_eq!(
            format!("{:?}", cfg.jitter_profile),
            format!("{:?}", JitterProfile::default())
        );

        cfg.overlay_enabled = false;
        cfg.overlay_speaking_only = true;
        cfg.overlay_opacity = 0.65;
        cfg.hotkey_deafen = "F11".to_string();
        cfg.opus_bitrate_bps = 64_000;

        let encoded = toml::to_string_pretty(&cfg).unwrap();
        let decoded: AppConfig = toml::from_str(&encoded).unwrap();
        assert!(!decoded.overlay_enabled);
        assert!(decoded.overlay_speaking_only);
        assert!((decoded.overlay_opacity - 0.65).abs() < 1e-4);
        assert_eq!(decoded.hotkey_deafen, "F11");
        assert_eq!(decoded.opus_bitrate_bps, 64_000);
        assert_eq!(
            format!("{:?}", decoded.jitter_profile),
            format!("{:?}", cfg.jitter_profile)
        );

        let legacy_toml: String = encoded
            .lines()
            .filter(|line| {
                !line.starts_with("hotkey_deafen =")
                    && !line.starts_with("jitter_profile =")
                    && !line.starts_with("opus_bitrate_bps =")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let legacy_decoded: AppConfig = toml::from_str(&legacy_toml).unwrap();
        assert_eq!(legacy_decoded.hotkey_deafen, "F10");
        assert_eq!(legacy_decoded.opus_bitrate_bps, 40_000);
        assert_eq!(
            format!("{:?}", legacy_decoded.jitter_profile),
            format!("{:?}", JitterProfile::default())
        );
    }
}
