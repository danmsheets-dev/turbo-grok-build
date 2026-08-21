//! Shared PCM helpers: downmix, resample, mix.

/// Average interleaved frames to mono i16.
pub(super) fn frames_to_mono_i16_i16(data: &[i16], channels: usize) -> Vec<i16> {
    if channels <= 1 {
        return data.to_vec();
    }
    let mut mono = Vec::with_capacity(data.len() / channels);
    for frame in data.chunks_exact(channels) {
        let mut sum: i32 = 0;
        for sample in frame {
            sum += i32::from(*sample);
        }
        let avg = (sum / channels as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        mono.push(avg as i16);
    }
    mono
}

pub(super) fn frames_to_mono_i16_f32(data: &[f32], channels: usize) -> Vec<i16> {
    if channels == 0 {
        return Vec::new();
    }
    if channels == 1 {
        return data.iter().copied().map(f32_to_i16).collect();
    }
    let mut mono = Vec::with_capacity(data.len() / channels);
    for frame in data.chunks_exact(channels) {
        let mut sum = 0.0f32;
        for sample in frame {
            sum += *sample;
        }
        mono.push(f32_to_i16(sum / channels as f32));
    }
    mono
}

fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

pub(super) fn resample_mono_i16(samples: &[i16], input_rate: u32, output_rate: u32) -> Vec<i16> {
    if samples.is_empty() || input_rate == 0 || output_rate == 0 {
        return Vec::new();
    }
    if input_rate == output_rate {
        return samples.to_vec();
    }

    let output_len =
        ((samples.len() as u64 * u64::from(output_rate)) / u64::from(input_rate)).max(1) as usize;
    let step = f64::from(input_rate) / f64::from(output_rate);
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * step;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let s0 = f64::from(samples[idx]);
        let s1 = f64::from(*samples.get(idx + 1).unwrap_or(&samples[idx]));
        let sample = s0 + (s1 - s0) * frac;
        let clamped = sample.round().max(f64::from(i16::MIN)).min(f64::from(i16::MAX));
        output.push(clamped as i16);
    }

    output
}

/// Mix equal-length (or pad-short) i16 streams with saturation.
pub(super) fn mix_i16_frames(streams: &[Vec<i16>], frame: usize) -> Vec<i16> {
    if frame == 0 || streams.is_empty() {
        return Vec::new();
    }
    let mut acc = vec![0i32; frame];
    let mut any = false;
    for s in streams {
        let n = s.len().min(frame);
        if n == 0 {
            continue;
        }
        any = true;
        for (i, sample) in s.iter().take(n).enumerate() {
            acc[i] += i32::from(*sample);
        }
    }
    if !any {
        return Vec::new();
    }
    acc.into_iter()
        .map(|v| v.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
        .collect()
}

pub(super) fn le_bytes_to_i16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

pub(super) fn i16_to_le_bytes(pcm: &[i16]) -> Vec<u8> {
    pcm.iter().flat_map(|s| s.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_rate() {
        let input: Vec<i16> = (0..48).map(|i| (i * 100) as i16).collect();
        let out = resample_mono_i16(&input, 48_000, 16_000);
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn downmix_stereo_averages() {
        let stereo = [1000i16, 3000, 0, 0];
        let mono = frames_to_mono_i16_i16(&stereo, 2);
        assert_eq!(mono, vec![2000, 0]);
    }

    #[test]
    fn mix_saturates() {
        let a = vec![20_000i16, 20_000];
        let b = vec![20_000i16, -20_000];
        let m = mix_i16_frames(&[a, b], 2);
        assert_eq!(m[0], i16::MAX);
        assert_eq!(m[1], 0);
    }
}
