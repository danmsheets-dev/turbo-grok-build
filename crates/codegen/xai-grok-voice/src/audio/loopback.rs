//! WASAPI loopback: capture what Windows is *playing* (Teams/Zoom mix).
//!
//! Opens the default console and communications **render** endpoints with
//! `AUDCLNT_STREAMFLAGS_LOOPBACK`, downmixes to 16 kHz mono, and optionally
//! mixes the microphone so the local talker is included on a headset.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tokio::sync::mpsc as async_mpsc;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    eCommunications, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{GUID, PCWSTR};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
// KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x0000_0003_0000_0010_8000_00aa_0038_9b71);

use super::capture::{CaptureHandle, spawn_mic_to_sync, spawn_pcm_bridge};
use super::pcm::{
    frames_to_mono_i16_f32, frames_to_mono_i16_i16, i16_to_le_bytes, le_bytes_to_i16,
    mix_i16_frames, resample_mono_i16,
};
use crate::error::VoiceError;

const FRAME_SAMPLES: usize = 320; // 20 ms at 16 kHz
const MIX_TICK: Duration = Duration::from_millis(20);

#[derive(Clone, Copy)]
struct MixFormat {
    channels: u16,
    sample_rate: u32,
    bits: u16,
    is_float: bool,
    block_align: u16,
}

pub struct MeetingCaptureReport {
    pub used_loopback: bool,
    pub mic: bool,
    pub device_labels: Vec<String>,
}

pub fn spawn_loopback_mix(
    sample_rate: u32,
    pcm_tx: async_mpsc::Sender<Vec<u8>>,
    include_mic: bool,
) -> Result<(CaptureHandle, MeetingCaptureReport), VoiceError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (mix_tx, mix_rx) = mpsc::sync_channel::<Vec<u8>>(64);
    let bridge = spawn_pcm_bridge(mix_rx, pcm_tx);

    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String, String>>(4);
    let endpoints = match thread::spawn(list_render_endpoints).join() {
        Ok(Ok(e)) if !e.is_empty() => e,
        Ok(Ok(_)) => {
            bridge.abort();
            return Err(VoiceError::Config(
                "no WASAPI render endpoints for loopback".into(),
            ));
        }
        Ok(Err(e)) => {
            bridge.abort();
            return Err(e);
        }
        Err(_) => {
            bridge.abort();
            return Err(VoiceError::Config(
                "WASAPI enumerator thread panicked".into(),
            ));
        }
    };

    let mut source_rxs: Vec<mpsc::Receiver<Vec<u8>>> = Vec::new();
    let mut child_threads: Vec<JoinHandle<()>> = Vec::new();
    let mut labels: Vec<String> = Vec::new();

    for (role, id) in endpoints {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let stop_c = Arc::clone(&stop);
        let ready = ready_tx.clone();
        let thread = thread::spawn(move || {
            run_loopback_thread(role, id, sample_rate, tx, stop_c, ready);
        });
        source_rxs.push(rx);
        child_threads.push(thread);
    }
    drop(ready_tx);

    let mut opened = 0usize;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while opened < child_threads.len() && std::time::Instant::now() < deadline {
        let wait = deadline.saturating_duration_since(std::time::Instant::now());
        match ready_rx.recv_timeout(wait) {
            Ok(Ok(label)) => {
                tracing::info!(device = %label, "WASAPI loopback opened");
                labels.push(label);
                opened += 1;
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "WASAPI loopback endpoint failed");
                opened += 1;
            }
            Err(_) => break,
        }
    }

    if labels.is_empty() {
        stop.store(true, Ordering::Release);
        for t in child_threads {
            let _ = t.join();
        }
        bridge.abort();
        return Err(VoiceError::Config(
            "WASAPI loopback could not open a render device".into(),
        ));
    }

    let mut mic_ok = false;
    if include_mic {
        let (mic_tx, mic_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        match spawn_mic_to_sync(sample_rate, mic_tx, Arc::clone(&stop)) {
            Ok(th) => {
                source_rxs.push(mic_rx);
                child_threads.push(th);
                mic_ok = true;
                labels.push("microphone".into());
            }
            Err(e) => {
                tracing::warn!(error = %e, "meeting mix: microphone unavailable; loopback only");
            }
        }
    }

    let stop_mix = Arc::clone(&stop);
    let mixer = thread::spawn(move || {
        run_mixer(source_rxs, mix_tx, stop_mix);
        for t in child_threads {
            let _ = t.join();
        }
    });

    Ok((
        CaptureHandle::from_parts(stop, mixer, bridge),
        MeetingCaptureReport {
            used_loopback: true,
            mic: mic_ok,
            device_labels: labels,
        },
    ))
}

fn run_mixer(
    rxs: Vec<mpsc::Receiver<Vec<u8>>>,
    mix_tx: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) {
    let mut bufs: Vec<Vec<i16>> = vec![Vec::new(); rxs.len()];
    while !stop.load(Ordering::Acquire) {
        thread::sleep(MIX_TICK);
        for (i, rx) in rxs.iter().enumerate() {
            while let Ok(bytes) = rx.try_recv() {
                bufs[i].extend(le_bytes_to_i16(&bytes));
            }
            if bufs[i].len() > FRAME_SAMPLES * 10 {
                let drop = bufs[i].len() - FRAME_SAMPLES * 4;
                bufs[i].drain(..drop);
            }
        }
        let frames: Vec<Vec<i16>> = bufs
            .iter()
            .map(|b| b.iter().take(FRAME_SAMPLES).copied().collect())
            .collect();
        let mixed = mix_i16_frames(&frames, FRAME_SAMPLES);
        for b in &mut bufs {
            let n = b.len().min(FRAME_SAMPLES);
            if n > 0 {
                b.drain(..n);
            }
        }
        if mixed.is_empty() {
            continue;
        }
        let _ = mix_tx.try_send(i16_to_le_bytes(&mixed));
    }
}

fn list_render_endpoints() -> Result<Vec<(&'static str, String)>, VoiceError> {
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = list_render_endpoints_inner();
        if initialized {
            CoUninitialize();
        }
        result
    }
}

unsafe fn list_render_endpoints_inner() -> Result<Vec<(&'static str, String)>, VoiceError> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| {
                VoiceError::Config(format!("WASAPI enumerator: {e}"))
            })?;
        let mut out: Vec<(&'static str, String)> = Vec::new();
        let mut seen = Vec::<String>::new();
        for (role, erole) in [("console", eConsole), ("communications", eCommunications)] {
            let device = match enumerator.GetDefaultAudioEndpoint(eRender, erole) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let id = device_id(&device).unwrap_or_default();
            if id.is_empty() || seen.iter().any(|s| s == &id) {
                continue;
            }
            seen.push(id.clone());
            out.push((role, id));
        }
        Ok(out)
    }
}

unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    unsafe {
        let pw = device.GetId().ok()?;
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const _));
        s
    }
}

fn run_loopback_thread(
    role: &'static str,
    id: String,
    target_rate: u32,
    tx: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    ready: SyncSender<Result<String, String>>,
) {
    let label = format!("{role} render");
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        match open_and_pump(role, &id, target_rate, &tx, &stop, &ready, &label) {
            Ok(()) => {}
            Err(e) => {
                let _ = ready.send(Err(format!("{label}: {e}")));
            }
        }
        if initialized {
            CoUninitialize();
        }
    }
}

unsafe fn open_and_pump(
    role: &'static str,
    id: &str,
    target_rate: u32,
    tx: &SyncSender<Vec<u8>>,
    stop: &Arc<AtomicBool>,
    ready: &SyncSender<Result<String, String>>,
    label: &str,
) -> Result<(), String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("enumerator: {e}"))?;

        let device = device_from_id(&enumerator, id).or_else(|_| {
            let erole = if role == "communications" {
                eCommunications
            } else {
                eConsole
            };
            enumerator
                .GetDefaultAudioEndpoint(eRender, erole)
                .map_err(|e| format!("endpoint: {e}"))
        })?;

        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("activate: {e}"))?;
        let fmt_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("mix format: {e}"))?;
        if fmt_ptr.is_null() {
            return Err("null mix format".into());
        }
        let _fmt_free = CoTaskMemGuard(fmt_ptr);
        let mix = parse_mix_format(fmt_ptr)?;

        let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                0,
                0,
                fmt_ptr,
                None,
            )
            .map_err(|e| format!("initialize loopback: {e}"))?;

        let event =
            CreateEventW(None, false, false, PCWSTR::null()).map_err(|e| format!("event: {e}"))?;
        let _event_guard = EventGuard(event);
        client
            .SetEventHandle(event)
            .map_err(|e| format!("SetEventHandle: {e}"))?;

        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("capture client: {e}"))?;
        client.Start().map_err(|e| format!("start: {e}"))?;
        struct StopOnDrop<'a>(&'a IAudioClient);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                let _ = unsafe { self.0.Stop() };
            }
        }
        let _stop = StopOnDrop(&client);
        let _ = ready.send(Ok(label.to_string()));
        let _ = pump_capture(capture, event, mix, target_rate, tx, stop);
        Ok(())
    }
}

struct CoTaskMemGuard(*mut WAVEFORMATEX);
impl Drop for CoTaskMemGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
        }
    }
}

struct EventGuard(HANDLE);
impl Drop for EventGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

unsafe fn device_from_id(enumerator: &IMMDeviceEnumerator, id: &str) -> Result<IMMDevice, String> {
    unsafe {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        enumerator
            .GetDevice(PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("GetDevice: {e}"))
    }
}

unsafe fn parse_mix_format(ptr: *mut WAVEFORMATEX) -> Result<MixFormat, String> {
    unsafe {
        let fmt = &*ptr;
        let mut is_float = fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
        if fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE && fmt.cbSize >= 22 {
            let sub = std::ptr::addr_of!((*ptr.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat);
            is_float = std::ptr::read_unaligned(sub) == SUBTYPE_IEEE_FLOAT;
        } else if fmt.wFormatTag == WAVE_FORMAT_PCM {
            is_float = false;
        }
        if fmt.nChannels == 0 || fmt.nSamplesPerSec == 0 || fmt.nBlockAlign == 0 {
            return Err("invalid mix format".into());
        }
        Ok(MixFormat {
            channels: fmt.nChannels,
            sample_rate: fmt.nSamplesPerSec,
            bits: fmt.wBitsPerSample,
            is_float,
            block_align: fmt.nBlockAlign,
        })
    }
}

unsafe fn pump_capture(
    capture: IAudioCaptureClient,
    event: HANDLE,
    mix: MixFormat,
    target_rate: u32,
    tx: &SyncSender<Vec<u8>>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    unsafe {
        while !stop.load(Ordering::Acquire) {
            let wr = WaitForSingleObject(event, 50);
            if wr != WAIT_OBJECT_0 && !stop.load(Ordering::Acquire) {
                // timeout is fine
            }
            loop {
                let pkt = match capture.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if pkt == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                if !silent && !data.is_null() && frames > 0 {
                    if let Some(pcm) = packet_to_pcm16(data, frames, mix, target_rate) {
                        if !pcm.is_empty() {
                            let _ = tx.try_send(i16_to_le_bytes(&pcm));
                        }
                    }
                }
                let _ = capture.ReleaseBuffer(frames);
            }
        }
    }
    Ok(())
}

unsafe fn packet_to_pcm16(
    data: *mut u8,
    frames: u32,
    mix: MixFormat,
    target_rate: u32,
) -> Option<Vec<i16>> {
    let nbytes = frames as usize * mix.block_align as usize;
    if nbytes == 0 {
        return None;
    }
    let mut raw = vec![0u8; nbytes];
    unsafe {
        std::ptr::copy_nonoverlapping(data, raw.as_mut_ptr(), nbytes);
    }
    let channels = mix.channels as usize;
    let mono = if mix.is_float && mix.bits == 32 {
        let mut f32s = Vec::with_capacity(nbytes / 4);
        for c in raw.chunks_exact(4) {
            f32s.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        frames_to_mono_i16_f32(&f32s, channels)
    } else if mix.bits == 16 {
        let mut i16s = Vec::with_capacity(nbytes / 2);
        for c in raw.chunks_exact(2) {
            i16s.push(i16::from_le_bytes([c[0], c[1]]));
        }
        frames_to_mono_i16_i16(&i16s, channels)
    } else if mix.bits == 32 && !mix.is_float {
        let mut i16s = Vec::with_capacity(nbytes / 4);
        for c in raw.chunks_exact(4) {
            let v = i32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            i16s.push((v >> 16) as i16);
        }
        frames_to_mono_i16_i16(&i16s, channels)
    } else {
        return None;
    };
    Some(resample_mono_i16(&mono, mix.sample_rate, target_rate))
}
