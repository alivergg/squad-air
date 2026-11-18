use crate::{FRAME_SAMPLES, SAMPLE_RATE};
use rubato::{FftFixedIn, FftFixedOut, Resampler};
use std::collections::VecDeque;

const MAX_FIFO_SAMPLES: usize = 96_000;

fn cap_fifo(fifo: &mut VecDeque<f32>) {
    if fifo.len() > MAX_FIFO_SAMPLES {
        let excess = fifo.len() - MAX_FIFO_SAMPLES;
        fifo.drain(..excess);
    }
}

/// Преобразует входной моно-поток произвольной частоты (например, 44.1 кГц)
/// в строгие кадры по 480 сэмплов (10 мс @ 48 кГц).
pub struct CaptureResampler {
    in_rate: u32,
    fifo_in: VecDeque<f32>,
    fifo_out: VecDeque<f32>,
    resampler: Option<FftFixedOut<f32>>,
    input_scratch: Vec<Vec<f32>>,
    output_scratch: Vec<Vec<f32>>,
}

impl CaptureResampler {
    pub fn new(in_rate: u32) -> Self {
        let resampler = if in_rate != SAMPLE_RATE {
            FftFixedOut::<f32>::new(in_rate as usize, SAMPLE_RATE as usize, FRAME_SAMPLES, 1, 1)
                .ok()
        } else {
            None
        };

        let max_in = resampler
            .as_ref()
            .map(|r| r.input_frames_max())
            .unwrap_or(FRAME_SAMPLES);

        Self {
            in_rate,
            fifo_in: VecDeque::with_capacity(max_in * 4),
            fifo_out: VecDeque::with_capacity(FRAME_SAMPLES * 4),
            resampler,
            input_scratch: vec![vec![0.0f32; max_in]],
            output_scratch: vec![vec![0.0f32; FRAME_SAMPLES]],
        }
    }

    pub fn input_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        if self.resampler.is_none() {
            self.fifo_out.extend(samples.iter().copied());
            cap_fifo(&mut self.fifo_out);
            return;
        }
        self.fifo_in.extend(samples.iter().copied());
        cap_fifo(&mut self.fifo_in);

        if let Some(resampler) = self.resampler.as_mut() {
            loop {
                let needed = resampler.input_frames_next();
                if self.fifo_in.len() < needed {
                    break;
                }
                self.input_scratch[0].resize(needed, 0.0);
                for slot in &mut self.input_scratch[0] {
                    *slot = self.fifo_in.pop_front().unwrap_or(0.0);
                }

                if resampler
                    .process_into_buffer(&self.input_scratch, &mut self.output_scratch, None)
                    .is_ok()
                {
                    self.fifo_out
                        .extend(self.output_scratch[0][..FRAME_SAMPLES].iter().copied());
                }
            }
            cap_fifo(&mut self.fifo_out);
        }
    }

    /// Извлекает готовый 10-миллисекундный кадр (480 сэмплов @ 48 кГц), если он накоплен.
    pub fn pop_frame(&mut self, out: &mut [f32; FRAME_SAMPLES]) -> bool {
        if self.fifo_out.len() < FRAME_SAMPLES {
            return false;
        }
        for slot in out.iter_mut() {
            *slot = self.fifo_out.pop_front().unwrap_or(0.0);
        }
        true
    }
}

/// Преобразует смикшированные 480-сэмпловые кадры (48 кГц) в частоту выходного устройства.
pub struct PlaybackResampler {
    out_rate: u32,
    resampler: Option<FftFixedIn<f32>>,
    input_scratch: Vec<Vec<f32>>,
    output_scratch: Vec<Vec<f32>>,
}

impl PlaybackResampler {
    pub fn new(out_rate: u32) -> Self {
        let resampler = if out_rate != SAMPLE_RATE {
            FftFixedIn::<f32>::new(SAMPLE_RATE as usize, out_rate as usize, FRAME_SAMPLES, 1, 1)
                .ok()
        } else {
            None
        };

        let max_out = resampler
            .as_ref()
            .map(|r| r.output_frames_max())
            .unwrap_or(FRAME_SAMPLES);

        Self {
            out_rate,
            resampler,
            input_scratch: vec![vec![0.0f32; FRAME_SAMPLES]],
            output_scratch: vec![vec![0.0f32; max_out]],
        }
    }

    pub fn output_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn process_frame(&mut self, frame_48k: &[f32; FRAME_SAMPLES], out: &mut Vec<f32>) {
        out.clear();
        if let Some(resampler) = self.resampler.as_mut() {
            self.input_scratch[0].copy_from_slice(frame_48k);
            if let Ok((_, out_len)) =
                resampler.process_into_buffer(&self.input_scratch, &mut self.output_scratch, None)
            {
                out.extend_from_slice(&self.output_scratch[0][..out_len]);
                return;
            }
        }
        out.extend_from_slice(frame_48k);
    }
}
