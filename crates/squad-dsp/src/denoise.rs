use crate::FRAME_SAMPLES;
use nnnoiseless::DenoiseState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use tract_onnx::prelude::*;

const PCM16_SCALE: f32 = 32768.0;
const INV_PCM16_SCALE: f32 = 1.0 / 32768.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NoiseSuppressionMode {
    Off,
    #[default]
    Rnnoise,
    DeepFilterNet3,
}

type TractRunnable = SimplePlan<TypedFact, Box<dyn TypedOp>, Graph<TypedFact, Box<dyn TypedOp>>>;

pub struct NoiseSuppressor {
    mode: NoiseSuppressionMode,
    rnnoise: Box<DenoiseState<'static>>,
    df3_model: Option<TractRunnable>,
    df3_available: bool,
    scaled_in: [f32; FRAME_SAMPLES],
    scaled_out: [f32; FRAME_SAMPLES],
}

impl NoiseSuppressor {
    pub fn new(mode: NoiseSuppressionMode) -> Self {
        let df3_model = Self::try_load_df3_model();
        let df3_available = df3_model.is_some();

        if mode == NoiseSuppressionMode::DeepFilterNet3 && !df3_available {
            info!(
                "deepfilternet3.onnx not found in assets/models; using RNNoise fallback automatically"
            );
        }

        Self {
            mode,
            rnnoise: DenoiseState::new(),
            df3_model,
            df3_available,
            scaled_in: [0.0; FRAME_SAMPLES],
            scaled_out: [0.0; FRAME_SAMPLES],
        }
    }

    fn candidate_model_paths() -> Vec<PathBuf> {
        let mut paths = vec![PathBuf::from("assets/models/deepfilternet3.onnx")];
        if let Some(mut cfg) = dirs::config_dir() {
            cfg.push("squad-air");
            cfg.push("models");
            cfg.push("deepfilternet3.onnx");
            paths.push(cfg);
        }
        paths
    }

    fn try_load_df3_model() -> Option<TractRunnable> {
        for path in Self::candidate_model_paths() {
            if path.exists() {
                match Self::load_onnx(&path) {
                    Ok(plan) => {
                        info!("Loaded DeepFilterNet3 ONNX model from {}", path.display());
                        return Some(plan);
                    }
                    Err(e) => {
                        warn!("Failed to initialize ONNX model at {}: {e}", path.display());
                    }
                }
            }
        }
        None
    }

    fn load_onnx(path: &Path) -> TractResult<TractRunnable> {
        tract_onnx::onnx()
            .model_for_path(path)?
            .with_input_fact(0, f32::fact([1, FRAME_SAMPLES]).into())?
            .into_optimized()?
            .into_runnable()
    }

    pub fn set_mode(&mut self, mode: NoiseSuppressionMode) {
        self.mode = mode;
    }

    pub fn mode(&self) -> NoiseSuppressionMode {
        self.mode
    }

    pub fn is_df3_available(&self) -> bool {
        self.df3_available
    }

    /// Обрабатывает 480 сэмплов (10 мс @ 48 кГц) на месте.
    #[inline]
    pub fn process_frame(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        match self.mode {
            NoiseSuppressionMode::Off => {}
            NoiseSuppressionMode::Rnnoise => {
                self.run_rnnoise(frame);
            }
            NoiseSuppressionMode::DeepFilterNet3 => {
                if let Some(plan) = self.df3_model.as_ref() {
                    if Self::run_df3_tract(plan, frame).is_err() {
                        self.run_rnnoise(frame);
                    }
                } else {
                    self.run_rnnoise(frame);
                }
            }
        }
    }

    #[inline]
    fn run_rnnoise(&mut self, frame: &mut [f32; FRAME_SAMPLES]) {
        for (dst, &src) in self.scaled_in.iter_mut().zip(frame.iter()) {
            *dst = (src * PCM16_SCALE).clamp(-32768.0, 32767.0);
        }
        self.rnnoise
            .process_frame(&mut self.scaled_out, &self.scaled_in);
        for (dst, &src) in frame.iter_mut().zip(self.scaled_out.iter()) {
            *dst = (src * INV_PCM16_SCALE).clamp(-1.0, 1.0);
        }
    }

    fn run_df3_tract(plan: &TractRunnable, frame: &mut [f32; FRAME_SAMPLES]) -> TractResult<()> {
        let input_tensor =
            tract_ndarray::Array2::from_shape_vec((1, FRAME_SAMPLES), frame.to_vec())?
                .into_tensor();
        let mut outputs = plan.run(tvec!(input_tensor.into()))?;
        if let Some(first) = outputs.pop() {
            let view = first.to_array_view::<f32>()?;
            if let Some(slice) = view.as_slice() {
                if slice.len() >= FRAME_SAMPLES {
                    for (dst, &src) in frame.iter_mut().zip(slice[..FRAME_SAMPLES].iter()) {
                        *dst = src.clamp(-1.0, 1.0);
                    }
                }
            }
        }
        Ok(())
    }
}
