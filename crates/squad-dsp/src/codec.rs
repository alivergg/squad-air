use crate::FRAME_SAMPLES;
use audiopus::{
    coder::{Decoder, Encoder},
    packet::Packet,
    Application, Bitrate, Channels, MutSignals, SampleRate,
};
use thiserror::Error;

const MAX_OPUS_PACKET_BYTES: usize = 512;

#[derive(Debug, Error)]
pub enum OpusCodecError {
    #[error("Opus internal error: {0}")]
    Opus(#[from] audiopus::Error),
}

pub struct OpusEncoder10ms {
    encoder: Encoder,
    scratch_out: [u8; MAX_OPUS_PACKET_BYTES],
}

impl OpusEncoder10ms {
    pub fn new(bitrate_bps: i32) -> Result<Self, OpusCodecError> {
        let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        encoder.set_bitrate(Bitrate::BitsPerSecond(bitrate_bps))?;
        encoder.set_inband_fec(true)?;
        encoder.set_packet_loss_perc(5)?;
        let _ = encoder.set_complexity(8);

        Ok(Self {
            encoder,
            scratch_out: [0u8; MAX_OPUS_PACKET_BYTES],
        })
    }

    /// Кодирует 480 сэмплов (10 мс).
    /// Возвращает `Ok(None)`, если кадр является тишиной/комфортным шумом DTX (`len <= 2`),
    /// чтобы не отправлять лишние пакеты в сеть.
    pub fn encode_frame(
        &mut self,
        pcm_480: &[f32; FRAME_SAMPLES],
    ) -> Result<Option<Vec<u8>>, OpusCodecError> {
        // Быстрая проверка на цифровую тишину (DTX gate < -65 dBFS)
        let energy: f32 = pcm_480.iter().map(|&x| x * x).sum::<f32>() / (FRAME_SAMPLES as f32);
        if energy < 3.16e-7 {
            return Ok(None);
        }

        let encoded_len = self.encoder.encode_float(pcm_480, &mut self.scratch_out)?;
        if encoded_len <= 2 {
            // DTX silence / comfort noise TOC-only frame
            return Ok(None);
        }
        Ok(Some(self.scratch_out[..encoded_len].to_vec()))
    }

    pub fn set_bitrate(&mut self, bitrate_bps: i32) -> Result<(), OpusCodecError> {
        self.encoder
            .set_bitrate(Bitrate::BitsPerSecond(bitrate_bps.clamp(12_000, 128_000)))?;
        Ok(())
    }

    pub fn encode_speech_frame(
        &mut self,
        pcm_480: &[f32; FRAME_SAMPLES],
    ) -> Result<Vec<u8>, OpusCodecError> {
        let encoded_len = self.encoder.encode_float(pcm_480, &mut self.scratch_out)?;
        Ok(self.scratch_out[..encoded_len].to_vec())
    }
}

pub struct OpusDecoder10ms {
    decoder: Decoder,
}

impl OpusDecoder10ms {
    pub fn new() -> Result<Self, OpusCodecError> {
        let decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;
        Ok(Self { decoder })
    }

    /// Декодирует нормальный Opus-пакет в 480 сэмплов.
    pub fn decode_frame(
        &mut self,
        opus_bytes: &[u8],
        out_pcm: &mut [f32; FRAME_SAMPLES],
    ) -> Result<(), OpusCodecError> {
        let packet = Packet::try_from(opus_bytes)?;
        let signals = MutSignals::try_from(&mut out_pcm[..])?;
        let _decoded_samples = self.decoder.decode_float(Some(packet), signals, false)?;
        Ok(())
    }

    /// Декодирует потерянный пакет с использованием In-Band FEC из следующего пакета (`next_opus_bytes`),
    /// либо через встроенный PLC (Packet Loss Concealment), если `next_opus_bytes == None`.
    pub fn decode_missing(
        &mut self,
        next_opus_bytes: Option<&[u8]>,
        out_pcm: &mut [f32; FRAME_SAMPLES],
    ) -> Result<(), OpusCodecError> {
        let signals = MutSignals::try_from(&mut out_pcm[..])?;
        if let Some(next_bytes) = next_opus_bytes {
            let packet = Packet::try_from(next_bytes)?;
            let _ = self.decoder.decode_float(Some(packet), signals, true)?;
        } else {
            let _ = self.decoder.decode_float(None, signals, false)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opus_encoder_set_bitrate() {
        let mut encoder = OpusEncoder10ms::new(40_000).unwrap();
        assert!(encoder.set_bitrate(24_000).is_ok());
        assert!(encoder.set_bitrate(64_000).is_ok());
        assert!(encoder.set_bitrate(1_000).is_ok());
        assert!(encoder.set_bitrate(500_000).is_ok());

        let mut pcm = [0.0f32; FRAME_SAMPLES];
        for (i, sample) in pcm.iter_mut().enumerate() {
            let t = i as f32 / 48_000.0;
            *sample = (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.4;
        }
        assert!(encoder.encode_speech_frame(&pcm).is_ok());
    }
}
