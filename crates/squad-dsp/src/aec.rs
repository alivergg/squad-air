use crate::{FRAME_SAMPLES, SAMPLE_RATE};
use aec3::voip::VoipAec3;

pub struct EchoCanceller {
    enabled: bool,
    inner: Option<VoipAec3>,
    render_scratch: [f32; FRAME_SAMPLES],
    out_scratch: [f32; FRAME_SAMPLES],
}

impl EchoCanceller {
    pub fn new(enabled: bool) -> Self {
        let inner = if enabled { Self::build_aec3() } else { None };
        Self {
            enabled,
            inner,
            render_scratch: [0.0; FRAME_SAMPLES],
            out_scratch: [0.0; FRAME_SAMPLES],
        }
    }

    fn build_aec3() -> Option<VoipAec3> {
        VoipAec3::builder(SAMPLE_RATE as usize, 1, 1)
            .initial_delay_ms(40)
            .enable_high_pass(true)
            .build()
            .ok()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled {
            if self.inner.is_none() {
                self.inner = Self::build_aec3();
            }
        } else {
            self.inner = None;
        }
        self.enabled = enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Передает 10-мс референсный кадр с выхода микшера (то, что звучит в наушниках/колонках).
    #[inline]
    pub fn feed_render_frame(&mut self, render_frame: &[f32; FRAME_SAMPLES]) {
        self.render_scratch.copy_from_slice(render_frame);
        if self.enabled {
            if let Some(aec) = self.inner.as_mut() {
                let _ = aec.handle_render_frame(&self.render_scratch);
            }
        }
    }

    /// Очищает микрофонный кадр от акустического эха.
    #[inline]
    pub fn process_capture_frame(&mut self, capture_frame: &mut [f32; FRAME_SAMPLES]) {
        if !self.enabled {
            return;
        }
        if let Some(aec) = self.inner.as_mut() {
            if aec
                .process_capture_frame(capture_frame, false, &mut self.out_scratch)
                .is_ok()
            {
                capture_frame.copy_from_slice(&self.out_scratch);
            }
        }
    }
}
