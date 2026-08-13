use esp_hal::{Blocking, analog::adc::{Adc, AdcPin}};

pub struct SoilSensor<'a, ADC, PIN> {
    adc: Adc<'a, ADC, Blocking>,
    pin: AdcPin<PIN, ADC>,
    dry_value: u16,
    wet_value: u16,
    state: State,
    consecutive_reads: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Dry,
    Wet,
}

impl <'a, ADC, PIN> SoilSensor<'a, ADC, PIN> 
where 
    ADC: esp_hal::analog::adc::RegisterAccess + 'a,
    PIN: esp_hal::analog::adc::AdcChannel,
{
    pub fn new(adc: Adc<'a, ADC, Blocking>, pin: AdcPin<PIN, ADC>) -> Self {
        Self {
            adc,
            pin,
            dry_value: 3000,
            wet_value: 2000,
            state: State::Wet,
            consecutive_reads: 0,
        }
    }

    pub fn read_raw(&mut self) -> u16 {
        nb::block!(self.adc.read_oneshot(&mut self.pin)).unwrap_or(0)
    }

    pub fn read_raw_filtered(&mut self) -> u16 {
        let mut samples = [0u16; 10];

        for sample in samples.iter_mut() {
            *sample = nb::block!(self.adc.read_oneshot(&mut self.pin)).unwrap_or(0);
        }

        samples.sort_unstable();

        let sum: u32 = samples[2..8].iter().map(|&x| x as u32).sum();
        (sum / 6) as u16
    }

    pub fn read_percentage(&mut self) -> u8 {
        let raw = self.read_raw_filtered();

        if raw >= self.dry_value {
            return 0;
        }

        if raw <= self.wet_value {
            return 100;
        }

        let range = (self.dry_value - self.wet_value) as u32;
        let value = (self.dry_value - raw) as u32;

        ((value * 100) / range) as u8
    }

    pub fn calibrate(&mut self, dry: u16, wet: u16) {
        self.dry_value = dry;
        self.wet_value = wet;
    }


    /// This function take a mesure and update internal state (dry or wet)
    pub fn check_state(&mut self) -> State {
        let humidity = self.read_percentage();
        let target_state =  if humidity <= 30 {
            State::Dry
        } else {
            State::Wet
        };

        if target_state != self.state {
            self.consecutive_reads += 1;

            if self.consecutive_reads >= 3 {
                self.state = target_state;
                self.consecutive_reads = 0;
            }
        } else {
            self.consecutive_reads = 0;
        }

        self.state
    }
}

