use embedded_hal::delay::DelayNs;
use esp_hal::ledc::{channel::{Channel, ChannelIFace}, timer::TimerSpeed};



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
