use embedded_hal::delay::DelayNs;
use esp_hal::{
    ledc::{
        channel::{Channel, ChannelHW, ChannelIFace},
        timer::TimerSpeed,
    },
    peripherals::FLASH,
};

use crate::storage::ValveStorage;



pub trait Servo {
    fn open<D: DelayNs>(&mut self, delay: &mut D);
    fn close<D: DelayNs>(&mut self, delay: &mut D);
}

pub struct Servo360<'a, S: TimerSpeed> {
    channel: Channel<'a, S>,
    puls_ms: u32,
}

const PCT_CW: u8 = 5;
const PCT_CCW: u8 = 10;

impl<'a, S: TimerSpeed> Servo360<'a, S> {
    pub fn new(channel: Channel<'a, S>, rotation_time_ms: u32) -> Self {
        let mut servo = Self { 
            channel,
            puls_ms: rotation_time_ms,
        };
        servo.stop();
        servo
    }

    pub fn stop(&mut self) {
        let _ = self.channel.set_duty(0);
    }

    pub fn spin_cw(&mut self) {
        let _ = self.channel.set_duty(PCT_CW);
    }

    pub fn spin_ccw(&mut self) {
        let _ = self.channel.set_duty(PCT_CCW);
    }
}

impl<'a, S: TimerSpeed> Servo for Servo360<'a, S> {
    fn open<D: DelayNs>(&mut self, delay: &mut D) {
        self.spin_ccw();
        delay.delay_ms(self.puls_ms);
        self.stop();
    }

    fn close<D: DelayNs>(&mut self, delay: &mut D) {
        self.spin_cw();
        delay.delay_ms(self.puls_ms);
        self.stop();
    }
}

const ANGLE_CLOSE: u8 = 90;
const ANGLE_OPEN: u8 = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServoState {
    Open,
    Close,
}

pub struct Servo180<'a, S: TimerSpeed> {
    channel: Channel<'a, S>,
    travel_time_ms: u32,
    duty_0_deg: u32,
    duty_180_deg: u32,
    state: ServoState,
    storage: ValveStorage<'a>,
}

impl<'a, S: TimerSpeed> Servo180<'a, S> {
    pub fn new(channel: Channel<'a, S>, travel_time_ms: u32, flash: FLASH<'a>) -> Self {
        let mut storage = ValveStorage::new(flash);
        let state = storage.load();
        let servo = Self {
            channel,
            travel_time_ms,
            duty_0_deg: 819,
            duty_180_deg: 1638,
            state,
            storage,
        };
        
        servo
    }

    pub fn set_angle(&mut self, angle_deg: u8) {
        let angle = angle_deg.min(180) as u32;
        let range = self.duty_180_deg.saturating_sub(self.duty_0_deg);
        
        let duty = self.duty_0_deg + (range * angle) / 180;

        let _ = self.channel.set_duty_hw(duty);
    }

    pub fn disable(&mut self) {
        let _ = self.channel.set_duty_hw(0);
    }
}

impl <'a, S: TimerSpeed> Servo for Servo180<'a, S> {
    fn open<D: DelayNs>(&mut self, delay: &mut D) {
        if self.state == ServoState::Close {
            self.set_angle(ANGLE_OPEN);
            self.state = ServoState::Open;
            self.storage.save(ServoState::Open);
            delay.delay_ms(self.travel_time_ms);
        }
    }

    fn close<D: DelayNs>(&mut self, delay: &mut D) {
        if self.state == ServoState::Open {
            self.set_angle(ANGLE_CLOSE);
            self.state = ServoState::Close;
            self.storage.save(ServoState::Close);
            delay.delay_ms(self.travel_time_ms);
        }
    }
}
