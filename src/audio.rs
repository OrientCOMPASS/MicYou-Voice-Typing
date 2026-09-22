//! Prompt-sound engine with adjustable volume.
//!
//! Memory design (two resident copies per sound, per the "no compounding"
//! requirement):
//! * **master** — the immutable original WAV bytes, leaked once at init;
//! * **current** — the working copy actually handed to `PlaySoundA`,
//!   regenerated **from the master** on every volume change.
//!
//! Scaling always recomputes `master × volume`, never `current × volume`, so
//! repeated adjustments can neither compound gains nor accumulate rounding
//! drift (the 连乘陷阱). At volume 100 % the working pointer is simply the
//! master — zero extra memory, byte-identical playback.
//!
//! Swap safety: `PlaySoundA(SND_ASYNC)` keeps reading the buffer *after* the
//! call returns, so a superseded buffer must never be freed while a playback
//! may still be in flight. The working pointer is swapped atomically
//! (`AtomicPtr`) and retired buffers are intentionally leaked — each is a few
//! KB and only volume changes (rare, user-driven) retire one, so growth is
//! bounded by the number of adjustments, not by playback count.

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use std::sync::OnceLock;
use windows::core::PCSTR;
use windows::Win32::Media::Audio::{PlaySoundA, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

/// Parsed location + sample format of the WAV `data` chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PcmLayout {
    /// Byte offset of the sample data inside the file.
    pub off: usize,
    /// Byte length of the sample data (clamped to the actual buffer).
    pub len: usize,
    /// wFormatTag from the `fmt ` chunk (1 = PCM, 3 = IEEE float).
    pub format: u16,
    /// wBitsPerSample from the `fmt ` chunk.
    pub bits: u16,
}

/// One resident sound: immutable master + atomically swappable working copy.
struct Channel {
    master: &'static [u8],
    pcm: Option<PcmLayout>,
    current: AtomicPtr<u8>,
}

impl Channel {
    fn new(data: Vec<u8>) -> Self {
        let master: &'static [u8] = Box::leak(data.into_boxed_slice());
        let pcm = parse_wav(master);
        let ptr = if master.is_empty() {
            std::ptr::null_mut()
        } else {
            master.as_ptr() as *mut u8
        };
        Channel {
            master,
            pcm,
            current: AtomicPtr::new(ptr),
        }
    }

    /// Rebuild the working copy as `master × volume` and swap it in.
    fn rescale(&self, volume: f32) {
        let buf: &'static [u8] = if volume >= 0.999_9 {
            self.master // unity gain: point straight at the master
        } else {
            Box::leak(scale_wav(self.master, self.pcm, volume).into_boxed_slice())
        };
        // The previously current buffer is deliberately NOT freed (see module
        // docs): an async playback may still be reading it.
        self.current.store(buf.as_ptr() as *mut u8, Ordering::SeqCst);
    }

    fn play(&self) {
        let ptr = self.current.load(Ordering::Acquire);
        if ptr.is_null() {
            return; // no asset shipped / empty file — stay silent like v0.1
        }
        unsafe {
            let _ = PlaySoundA(
                PCSTR::from_raw(ptr as *const u8),
                None,
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            );
        }
    }

    /// Test/diagnostic view of the live working buffer.
    #[cfg(test)]
    pub(crate) fn current_bytes(&self) -> &'static [u8] {
        let ptr = self.current.load(Ordering::Acquire);
        if ptr.is_null() {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(ptr as *const u8, self.master.len()) }
    }
}

struct Inner {
    start: Channel,
    end: Channel,
    volume_bits: AtomicU32,
}

/// Cheap handle (shared `&'static Inner`) — every clone controls the same
/// resident buffers, so the hotkey thread and the config-reload path always
/// agree on volume.
#[derive(Clone)]
pub struct AudioEngine {
    inner: &'static Inner,
}

/// Process-wide handle so `reload_settings` (lib.rs) can reach the engine
/// without threading another global through the hotkey module.
static GLOBAL: OnceLock<AudioEngine> = OnceLock::new();

pub fn set_global(engine: AudioEngine) {
    let _ = GLOBAL.set(engine);
}

pub fn global() -> Option<&'static AudioEngine> {
    GLOBAL.get()
}

impl AudioEngine {
    /// Takes ownership of the raw WAV bytes (loaded from `assets/` at init)
    /// and makes them resident for the lifetime of the process.
    pub fn new(start_data: Vec<u8>, end_data: Vec<u8>) -> Self {
        let inner = Box::leak(Box::new(Inner {
            start: Channel::new(start_data),
            end: Channel::new(end_data),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
        }));
        AudioEngine { inner }
    }

    /// Linear volume in `0.0..=1.0`. Idempotent: re-applying the same value is
    /// a no-op (no rebuild, no retired buffer). Always rescales from the
    /// master, so calling `set_volume(0.5)` ten times equals calling it once.
    pub fn set_volume(&self, volume: f32) {
        let v = volume.clamp(0.0, 1.0);
        let prev = f32::from_bits(self.inner.volume_bits.swap(v.to_bits(), Ordering::SeqCst));
        if (prev - v).abs() < 1e-4 {
            return;
        }
        self.inner.start.rescale(v);
        self.inner.end.rescale(v);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.inner.volume_bits.load(Ordering::SeqCst))
    }

    pub fn play_start(&self) {
        self.inner.start.play();
    }

    pub fn play_end(&self) {
        self.inner.end.play();
    }

    #[cfg(test)]
    pub(crate) fn start_bytes(&self) -> &'static [u8] {
        self.inner.start.current_bytes()
    }
    #[cfg(test)]
    pub(crate) fn end_bytes(&self) -> &'static [u8] {
        self.inner.end.current_bytes()
    }
    #[cfg(test)]
    pub(crate) fn start_master(&self) -> &'static [u8] {
        self.inner.start.master
    }
}

// ─────────────────────────── WAV parsing / scaling ───────────────────────────

/// Locate the `data` chunk and read the sample format from `fmt `.
/// Returns None for anything that isn't a well-formed RIFF/WAVE — such files
/// simply never get scaled (played verbatim), which is always safe.
pub(crate) fn parse_wav(d: &[u8]) -> Option<PcmLayout> {
    if d.len() < 12 || &d[0..4] != b"RIFF" || &d[8..12] != b"WAVE" {
        return None;
    }
    let mut i = 12usize;
    let mut fmt: Option<(u16, u16)> = None;
    while i + 8 <= d.len() {
        let id = &d[i..i + 4];
        let sz = u32::from_le_bytes(d[i + 4..i + 8].try_into().ok()?) as usize;
        let body = i + 8;
        if body > d.len() {
            break;
        }
        if id == b"fmt " && sz >= 16 && body + 16 <= d.len() {
            let format = u16::from_le_bytes(d[body..body + 2].try_into().ok()?);
            let bits = u16::from_le_bytes(d[body + 14..body + 16].try_into().ok()?);
            fmt = Some((format, bits));
        } else if id == b"data" {
            let (format, bits) = fmt?;
            let len = sz.min(d.len() - body);
            return Some(PcmLayout {
                off: body,
                len,
                format,
                bits,
            });
        }
        i = body + sz + (sz & 1); // chunks are word-aligned
    }
    None
}

/// Produce `src` with only the sample data multiplied by `volume` (headers
/// untouched, so the result stays a valid WAV of identical size). Unsupported
/// formats are returned as a verbatim copy. Pure function of `(src, volume)`
/// — this is what guarantees no compounding across repeated adjustments.
pub(crate) fn scale_wav(src: &[u8], pcm: Option<PcmLayout>, volume: f32) -> Vec<u8> {
    let mut out = src.to_vec();
    let Some(p) = pcm else { return out };
    let end = (p.off + p.len).min(out.len());
    let data = &mut out[p.off..end];
    let v = volume.clamp(0.0, 1.0);
    match (p.format, p.bits) {
        // 8-bit PCM is unsigned with a 128 bias (the shipped assets' format).
        (1, 8) => {
            for b in data.iter_mut() {
                let s = (*b as i32 - 128) as f32 * v;
                *b = (s.round() as i32).clamp(-128, 127) as u8 ^ 0x80;
            }
        }
        (1, 16) => {
            for c in data.chunks_exact_mut(2) {
                let s = i16::from_le_bytes([c[0], c[1]]) as f32 * v;
                let o = (s.round() as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                c.copy_from_slice(&o.to_le_bytes());
            }
        }
        (3, 32) => {
            for c in data.chunks_exact_mut(4) {
                let s = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let o = (s * v).clamp(-1.0, 1.0);
                c.copy_from_slice(&o.to_le_bytes());
            }
        }
        _ => {} // ADPCM / 24-bit / unknown: play verbatim rather than corrupt
    }
    out
}

// ─────────────────────────── tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal synthetic WAV builder (fmt + data), little-endian.
    fn wav(bits: u16, format: u16, samples: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"RIFF");
        d.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
        d.extend_from_slice(b"WAVEfmt ");
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&format.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes()); // mono
        d.extend_from_slice(&44100u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // byte rate (unused here)
        d.extend_from_slice(&((bits / 8) as u16).to_le_bytes());
        d.extend_from_slice(&bits.to_le_bytes());
        d.extend_from_slice(b"data");
        d.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        d.extend_from_slice(samples);
        d
    }

    #[test]
    fn parses_layout_of_16bit_pcm() {
        let w = wav(16, 1, &[0x00, 0x10, 0xff, 0x7f]);
        let p = parse_wav(&w).unwrap();
        assert_eq!(p.format, 1);
        assert_eq!(p.bits, 16);
        assert_eq!(p.off, 44);
        assert_eq!(p.len, 4);
    }

    #[test]
    fn rejects_non_wav() {
        assert!(parse_wav(b"not a riff file at all").is_none());
        assert!(parse_wav(&[]).is_none());
    }

    #[test]
    fn scales_16bit_from_master_without_compounding() {
        let w = wav(16, 1, &[0x00, 0x40, 0x00, 0x80]); // +16384, -32768
        let p = parse_wav(&w);
        let half = scale_wav(&w, p, 0.5);
        assert_eq!(&half[..44], &w[..44], "headers must stay untouched");
        assert_eq!(i16::from_le_bytes([half[44], half[45]]), 8192);
        assert_eq!(i16::from_le_bytes([half[46], half[47]]), -16384);
        // Rescaling the *scaled* output would halve again — the engine never
        // does that; assert the pure function is deterministic from master:
        assert_eq!(scale_wav(&w, p, 0.5), half);
    }

    #[test]
    fn scales_8bit_with_bias() {
        // 8-bit unsigned: 255 = +127, 0 = -128, 128 = silence
        let w = wav(8, 1, &[255, 0, 128]);
        let p = parse_wav(&w);
        let half = scale_wav(&w, p, 0.5);
        assert_eq!(half[44], (64i32 + 128) as u8); // +127 × 0.5 = 63.5 → round 64
        assert_eq!(half[45], (-64i32 + 128) as u8); // -128 × 0.5 = -64
        assert_eq!(half[46], 128); // silence stays silence
        let mute = scale_wav(&w, p, 0.0);
        assert!(mute[44..47].iter().all(|&b| b == 128));
    }

    #[test]
    fn engine_set_volume_is_idempotent_and_master_relative() {
        let w16 = wav(16, 1, &[0x00, 0x40, 0x00, 0x80]);
        let eng = AudioEngine::new(w16.clone(), w16.clone());
        eng.set_volume(1.0);
        assert_eq!(eng.start_bytes(), eng.start_master(), "unity ⇒ master pointer");
        eng.set_volume(0.5);
        let once = eng.start_bytes().to_vec();
        eng.set_volume(0.5);
        eng.set_volume(0.5); // repeated identical calls must not compound
        assert_eq!(eng.start_bytes(), &once[..]);
        eng.set_volume(0.25);
        eng.set_volume(0.5); // back up: recomputed from master, not from 0.25
        assert_eq!(eng.start_bytes(), &once[..]);
        assert!((eng.volume() - 0.5).abs() < 1e-6);
    }
}
