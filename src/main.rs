#![no_std]
#![no_main]


use esp_backtrace as _;
use esp_hal::{analog::adc::{Adc, AdcConfig, Attenuation, Resolution}, gpio::DriveMode, i2c::master::{Config, I2c}, ledc::{LSGlobalClkSource, Ledc, LowSpeed, channel::{self, ChannelIFace}, timer::{self, TimerIFace, config::Duty}}, main, time::Rate};
use esp_println::println;
use planty::{display::OledScreen, servo::{Servo, Servo360}, soil::{SoilSensor, State}};

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);
    let mut delay = esp_hal::delay::Delay::new();

    println!("🌱 Planty ready !");

    let mut adc2_config = AdcConfig::new();
    let pin_g2 = adc2_config.enable_pin(peripherals.GPIO2, Attenuation::_11dB);
    let adc2 = Adc::new(peripherals.ADC2, adc2_config);
    let mut sensor = SoilSensor::new(adc2, pin_g2);
    sensor.calibrate(715, 430);
    println!("1. Soil Sensor OK");

    let i2c = I2c::new(peripherals.I2C0, Config::default())
        .unwrap()
        .with_sda(peripherals.GPIO21)
        .with_scl(peripherals.GPIO22);
    println!("2. I2C OK");
    let mut oled = OledScreen::new(i2c).expect("Failing to init OLED");
    println!("3. OLED screen OK");

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    timer
        .configure(timer::config::Config {
            duty: Duty::Duty14Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_hz(50),
        })
        .unwrap();

    let mut channel = ledc.channel(channel::Number::Channel0, peripherals.GPIO18);
    channel
        .configure(channel::config::Config {
            timer: &timer,
            duty_pct: 0,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    let mut valve = Servo360::new(channel, 250);

    let mut last_state = State::Wet;

    loop {
        let raw = sensor.read_raw();
        let humidity = sensor.read_percentage();
        let current_state = sensor.check_state();

        if current_state != last_state {
            match current_state {
                State::Dry => {
                    println!("DRY: Opening Valve...");
                    valve.open(&mut delay);
                    delay.delay_millis(1500);
                }
                State::Wet => {
                    println!("Wet: Closing Valve...");
                    valve.close(&mut delay);
                    delay.delay_millis(1500);
                }
            }
            last_state = current_state;
        }

        println!("Raw level: {} | Humidity: {}%", raw, humidity);
        oled.show_metrics(raw, humidity);
        delay.delay_millis(1000);
    }
}
