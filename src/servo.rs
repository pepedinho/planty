use embedded_hal::delay::DelayNs;
use esp_hal::ledc::{channel::{self, Channel, ChannelIFace}, timer::TimerSpeed};



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

pub struct Servo180<'a, S: TimerSpeed> {
    channel: Channel<'a, S>,
    travel_time_ms: u32,
    duty_0_deg: u8,
    duty_180_deg: u8,
}

impl<'a, S: TimerSpeed> Servo180<'a, S> {
    pub fn new(channel: Channel<'a, S>, travel_time_ms: u32) -> Self {
        let mut servo = Self {
            channel,
            travel_time_ms,
            duty_0_deg: 5,
            duty_180_deg: 10,
        };
        servo.set_angle(0);
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
        self.set_angle(180);
        delay.delay_ms(self.travel_time_ms);
    }

    fn close<D: DelayNs>(&mut self, delay: &mut D) {
        self.set_angle(0);
        delay.delay_ms(self.travel_time_ms);
    }
}
