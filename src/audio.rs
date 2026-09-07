// src/audio.rs
use windows::Win32::Media::Audio::{PlaySoundA, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::core::PCSTR;

#[derive(Debug, Clone)]
pub struct AudioEngine {
    start_wav: &'static [u8],
    end_wav: &'static [u8],
}

impl AudioEngine {
    pub fn new(start_data: Vec<u8>, end_data: Vec<u8>) -> Self {
        Self {
            start_wav: Box::leak(start_data.into_boxed_slice()),
            end_wav: Box::leak(end_data.into_boxed_slice()),
        }
    }

    pub fn play_start(&self) {
        self.play_internal(self.start_wav);
    }

    pub fn play_end(&self) {
        self.play_internal(self.end_wav);
    }

    fn play_internal(&self, data: &[u8]) {
        if data.is_empty() { return; }
        let flags = SND_MEMORY | SND_ASYNC | SND_NODEFAULT;
        unsafe {
            let ptr = PCSTR::from_raw(data.as_ptr());
            PlaySoundA(ptr, None, flags);
        }
    }
}