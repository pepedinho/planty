use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;

use crate::servo::ServoState;

const STORAGE_SECTOR_SIZE: u32 = 4096;
const BITS_PER_SECTOR: u32 = STORAGE_SECTOR_SIZE * 8;

pub struct ValveStorage<'d> {
    flash: FlashStorage<'d>,
    sector_offset: u32,
}

impl<'d> ValveStorage<'d> {
    pub fn new(flash: FLASH<'d>) -> Self {
        let storage = FlashStorage::new(flash);
        let capacity = storage.capacity() as u32;
        let sector_offset = capacity - STORAGE_SECTOR_SIZE;
        Self {
            flash: storage,
            sector_offset,
        }
    }

    pub fn load(&mut self) -> ServoState {
        let count = self.transition_count();
        if count % 2 == 1 {
            ServoState::Open
        } else {
            ServoState::Close
        }
    }

    pub fn save(&mut self, state: ServoState) {
        let count = self.transition_count();
        let target = match state {
            ServoState::Open => 1,
            ServoState::Close => 0,
        };
        if count % 2 == target {
            return;
        }

        if count >= BITS_PER_SECTOR - 1 {
            let _ = self
                .flash
                .erase(self.sector_offset, self.sector_offset + STORAGE_SECTOR_SIZE);
            if let ServoState::Open = state {
                self.clear_bit(0);
            }
            return;
        }

        self.clear_bit(count);
    }

    fn transition_count(&mut self) -> u32 {
        for word_idx in 0..(STORAGE_SECTOR_SIZE / 4) {
            let mut word = [0u8; 4];
            let _ = self
                .flash
                .read(self.sector_offset + word_idx * 4, &mut word);
            let w = u32::from_le_bytes(word);
            if w != 0 {
                return word_idx * 32 + w.trailing_zeros();
            }
        }
        BITS_PER_SECTOR
    }

    fn clear_bit(&mut self, bit_idx: u32) {
        let word_idx = bit_idx / 32;
        let bit_in_word = bit_idx % 32;
        let mut cur = [0u8; 4];
        let addr = self.sector_offset + word_idx * 4;
        let _ = self.flash.read(addr, &mut cur);
        let w = u32::from_le_bytes(cur) & !(1 << bit_in_word);
        let _ = self.flash.write(addr, &w.to_le_bytes());
    }
}
