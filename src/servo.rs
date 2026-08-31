use embedded_hal::delay::DelayNs;
use esp_hal::{
    ledc::{
        channel::{Channel, ChannelIFace},
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServoState {
    Open,
    Close,
}

pub struct Servo180<'a, S: TimerSpeed> {
    channel: Channel<'a, S>,
    travel_time_ms: u32,
    duty_0_deg: u8,
    duty_180_deg: u8,
    state: ServoState,
    storage: ValveStorage<'a>,
}

impl<'a, S: TimerSpeed> Servo180<'a, S> {
    pub fn new(channel: Channel<'a, S>, travel_time_ms: u32, flash: FLASH<'a>) -> Self {
        let mut storage = ValveStorage::new(flash);
        let state = storage.load();
        let mut servo = Self {
            channel,
            travel_time_ms,
            duty_0_deg: 5,
            duty_180_deg: 10,
            state,
            storage,
        };
        servo.set_angle(match state {
            ServoState::Open => 180,
            ServoState::Close => 0,
        });
        servo
    }

    pub fn set_angle(&mut self, angle_deg: u8) {
        let angle = angle_deg.min(180);
        let range = self.duty_180_deg.saturating_sub(self.duty_0_deg) as u32;
        let duty = self.duty_0_deg as u32 + (range * angle as u32) / 180;

        let _ = self.channel.set_duty(duty as u8);
    }

    pub fn disable(&mut self) {
        let _ = self.channel.set_duty(0);
    }
}

impl <'a, S: TimerSpeed> Servo for Servo180<'a, S> {
    fn open<D: DelayNs>(&mut self, delay: &mut D) {
        if self.state == ServoState::Close {
            self.set_angle(90);
            self.state = ServoState::Open;
            self.storage.save(ServoState::Open);
            delay.delay_ms(self.travel_time_ms);
        }
    }

    fn close<D: DelayNs>(&mut self, delay: &mut D) {
        if self.state == ServoState::Open {
            self.set_angle(0);
            self.state = ServoState::Close;
            self.storage.save(ServoState::Close);
            delay.delay_ms(self.travel_time_ms);
        }
    }
}
