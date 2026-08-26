#![no_std]
#![no_main]

extern crate alloc;

use core::net::Ipv4Addr;

use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    gpio::DriveMode,
    i2c::master::{Config as I2cConfig, I2c},
    ledc::{
        LSGlobalClkSource, Ledc, LowSpeed,
        channel::{self, ChannelIFace},
        timer::{self, TimerIFace, config::Duty},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_println::println;
use planty::{
    display::OledScreen,
    mqtt,
    servo::{Servo, Servo360},
    soil::{SoilSensor, State},
};

const WIFI_SSID: &str = env!("WIFI_SSID");
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");
const MQTT_BROKER_IP: [u8; 4] = [192, 168, 1, 32];
const MQTT_BROKER_PORT: u16 = 1883;
const MQTT_CLIENT_ID: &str = "planty-esp32";

const TOPIC_MOTOR: &str = "planty/mecha/motor";
const TOPIC_CALIBRATE: &str = "planty/sensor/calibrate";
const TOPIC_SOIL: &str = "planty/sensor/soil";

static mut STACK_RESOURCES: embassy_net::StackResources<5> = embassy_net::StackResources::new();
static mut TCP_RX_BUF: [u8; 1024] = [0u8; 1024];
static mut TCP_TX_BUF: [u8; 1024] = [0u8; 1024];

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_alloc::heap_allocator!(size: 128 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    println!("Planty starting...");
    println!("WIFI_SSID: {}", WIFI_SSID);
    println!("WIFI_PASSWORD: {}", WIFI_PASSWORD);

    // --- Peripherals ---
    let mut adc1_config = AdcConfig::new();
    let pin_g34 = adc1_config.enable_pin(peripherals.GPIO34, Attenuation::_11dB);
    let adc1 = Adc::new(peripherals.ADC1, adc1_config);
    let mut sensor = SoilSensor::new(adc1, pin_g34);
    sensor.calibrate(715, 430);
    println!("1. Soil Sensor OK (GPIO34 / ADC1)");

    let i2c = I2c::new(peripherals.I2C0, I2cConfig::default())
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
    let mut delay = esp_hal::delay::Delay::new();
    let mut cal_buf = [0u8; 256];

    // --- WiFi ---
    println!("4. Initializing WiFi...");
    let radio_controller = esp_radio::init().expect("Failed to init radio");
    // Leak to get 'static lifetime - this is intentional, radio lives for program lifetime
    let radio_static: &'static esp_radio::Controller<'static> =
        alloc::boxed::Box::leak(alloc::boxed::Box::new(radio_controller));
    let (mut wifi, sta_device) = planty::wifi::connect(
        radio_static,
        peripherals.WIFI,
        WIFI_SSID,
        WIFI_PASSWORD,
    )
    .expect("Failed to create WiFi");

    wifi.start_async()
        .await
        .expect("Failed to start WiFi");
    wifi.connect_async()
        .await
        .expect("Failed to connect WiFi");
    println!("5. WiFi connected");

    // --- Embassy-net ---
    let random_seed = esp_hal::rng::Rng::new().random() as u64;
    let (stack, runner) = planty::wifi::setup_stack(
        sta_device,
        unsafe { &mut *core::ptr::addr_of_mut!(STACK_RESOURCES) },
        random_seed,
    );

    // SAFETY: runner is 'static because it references our static STACK_RESOURCES.
    let runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>> =
        unsafe { core::mem::transmute(runner) };

    _spawner
        .spawn(planty::wifi::net_task(runner))
        .expect("Failed to spawn net task");

    stack.wait_config_up().await;
    println!("6. Network ready (DHCP OK)");

    // --- MQTT ---
    println!("7. Connecting MQTT...");
    let mut socket = TcpSocket::new(
        stack,
        unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX_BUF) },
        unsafe { &mut *core::ptr::addr_of_mut!(TCP_TX_BUF) },
    );

    socket
        .connect((
            Ipv4Addr::new(
                MQTT_BROKER_IP[0],
                MQTT_BROKER_IP[1],
                MQTT_BROKER_IP[2],
                MQTT_BROKER_IP[3],
            ),
            MQTT_BROKER_PORT,
        ))
        .await
        .expect("Failed to connect TCP to broker");

    mqtt::connect(&mut socket, MQTT_CLIENT_ID, 60)
        .await
        .expect("Failed to MQTT connect");
    println!("8. MQTT connected");

    mqtt::subscribe(&mut socket, TOPIC_MOTOR, 1)
        .await
        .expect("Failed to subscribe motor topic");
    mqtt::subscribe(&mut socket, TOPIC_CALIBRATE, 2)
        .await
        .expect("Failed to subscribe calibrate topic");
    println!("9. MQTT subscribed");

    let mut publish_counter: u32 = 0;

    // --- Main loop ---
    loop {
        // 1. Read sensor
        let raw = sensor.read_raw();
        let humidity = sensor.read_percentage();
        let current_state = sensor.check_state();

        // 2. Auto-control valve based on sensor
        if current_state != last_state {
            match current_state {
                State::Dry => {
                    println!("DRY: Opening Valve...");
                    valve.open(&mut delay);
                    delay.delay_millis(1500);
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"open").await;
                }
                State::Wet => {
                    println!("Wet: Closing Valve...");
                    valve.close(&mut delay);
                    delay.delay_millis(1500);
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"close").await;
                }
            }
            last_state = current_state;
        }

        // 3. Poll MQTT for commands
        if let Some(event) = mqtt::poll(&mut socket, &mut cal_buf).await {
            match event {
                mqtt::MqttEvent::Publish { topic, payload } => {
                    if topic == TOPIC_MOTOR {
                        match core::str::from_utf8(payload) {
                            Ok("open") => {
                                println!("MQTT: Opening valve...");
                                valve.open(&mut delay);
                                delay.delay_millis(1500);
                            }
                            Ok("close") => {
                                println!("MQTT: Closing valve...");
                                valve.close(&mut delay);
                                delay.delay_millis(1500);
                            }
                            Ok(other) => println!("MQTT: unknown motor cmd: {}", other),
                            Err(_) => println!("MQTT: motor cmd not valid utf8"),
                        }
                    } else if topic == TOPIC_CALIBRATE {
                        match core::str::from_utf8(payload) {
                            Ok(json) => {
                                if let Some((dry, wet)) = parse_calibration(json) {
                                    println!("MQTT: Calibrating dry={} wet={}", dry, wet);
                                    sensor.calibrate(dry, wet);
                                } else {
                                    println!("MQTT: failed to parse calibration json");
                                }
                            }
                            Err(_) => println!("MQTT: calibrate not valid utf8"),
                        }
                    }
                }
                mqtt::MqttEvent::PingResp => {}
                mqtt::MqttEvent::SubAck => {}
                mqtt::MqttEvent::ConnAck => {}
            }
        }

        // 4. Publish sensor data periodically
        publish_counter += 1;
        if publish_counter >= 5 {
            publish_counter = 0;
            let mut payload = [0u8; 8];
            let len = format_number(humidity, &mut payload);
            let _ = mqtt::publish(&mut socket, TOPIC_SOIL, &payload[..len]).await;
        }

        // 5. Update display
        oled.show_metrics(raw, humidity);
        println!("Raw: {} | Humidity: {}%", raw, humidity);

        // 6. Wait
        Timer::after_millis(1000).await;
    }
}

fn format_number(val: u8, buf: &mut [u8]) -> usize {
    if val == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 3];
    let mut i = 0;
    let mut v = val;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut j = 0;
    while i > 0 {
        i -= 1;
        buf[j] = tmp[i];
        j += 1;
    }
    j
}

fn parse_calibration(json: &str) -> Option<(u16, u16)> {
    let json = json.trim();
    if !json.starts_with('{') || !json.ends_with('}') {
        return None;
    }
    let inner = &json[1..json.len() - 1];

    let mut dry: Option<u16> = None;
    let mut wet: Option<u16> = None;

    for part in inner.split(',') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("\"dry\"").or_else(|| part.strip_prefix("dry")) {
            let val = val.trim().trim_start_matches(':').trim();
            dry = val.parse().ok();
        } else if let Some(val) =
            part.strip_prefix("\"wet\"").or_else(|| part.strip_prefix("wet"))
        {
            let val = val.trim().trim_start_matches(':').trim();
            wet = val.parse().ok();
        }
    }

    Some((dry?, wet?))
}
