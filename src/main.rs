#![no_std]
#![no_main]

extern crate alloc;

use core::net::Ipv4Addr;

use embassy_executor::Spawner;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::{
    analog::adc::{Adc, AdcConfig, Attenuation},
    gpio::{DriveMode, Input, InputConfig, Pull},
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
    mqtt,
    servo::{Servo, Servo180, ServoState},
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

const POLL_TIMEOUT_MS: u64 = 200;
const RECONNECT_DELAY_MS: u64 = 5000;
const TCP_CONNECT_TIMEOUT_MS: u64 = 5000;
/// Number of consecutive MQTT polls that yielded no event before we consider
/// the link silently dead and force a reconnect. The loop iterates ~1/s, so
/// this is roughly the number of seconds of silence tolerated.
const MQTT_STALL_THRESHOLD: u32 = 45;

static mut STACK_RESOURCES: embassy_net::StackResources<5> = embassy_net::StackResources::new();
static mut TCP_RX_BUF: [u8; 1024] = [0u8; 1024];
static mut TCP_TX_BUF: [u8; 1024] = [0u8; 1024];
static mut MQTT_RX_BUF: [u8; 512] = [0u8; 512];

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default());

    esp_alloc::heap_allocator!(size: 128 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    println!("Planty starting...");
    println!("W_SSID {}", WIFI_SSID);
    println!("W_PWS: {}", WIFI_PASSWORD);

    // --- Peripherals ---
    let mut adc1_config = AdcConfig::new();
    let pin_g34 = adc1_config.enable_pin(peripherals.GPIO34, Attenuation::_11dB);
    let adc1 = Adc::new(peripherals.ADC1, adc1_config);
    let mut sensor = SoilSensor::new(adc1, pin_g34);
    sensor.calibrate(3263, 1610);
    println!("1. Soil Sensor OK (GPIO34 / ADC1)");

    let mut ledc = Ledc::new(peripherals.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);

    let mut t = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    t.configure(timer::config::Config {
            duty: Duty::Duty14Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_hz(50),
        })
        .unwrap();

    let mut channel = ledc.channel(channel::Number::Channel0, peripherals.GPIO18);
    channel
        .configure(channel::config::Config {
            timer: &t,
            duty_pct: 0,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    let mut valve = Servo180::new(channel, 250, peripherals.FLASH);
    let mut last_state = State::Wet;
    let mut delay = esp_hal::delay::Delay::new();
    let mut mqtt_rx = mqtt::MqttRx::new(unsafe {
        &mut *core::ptr::addr_of_mut!(MQTT_RX_BUF)
    });

    // --- Button (GPIO19, pull-up, connects to GND when pressed) ---
    let button = Input::new(
        peripherals.GPIO19,
        InputConfig::default().with_pull(Pull::Up),
    );
    // Edge detection: only toggle once per press, not continuously while held.
    let mut button_prev_pressed = false;
    println!("2. Button OK (GPIO19)");

    // --- WiFi ---
    println!("4. Initializing WiFi...");
    let radio_controller = esp_radio::init().expect("Failed to init radio");
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

    let runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>> =
        unsafe { core::mem::transmute(runner) };

    _spawner
        .spawn(planty::wifi::net_task(runner))
        .expect("Failed to spawn net task");

    stack.wait_config_up().await;
    println!("6. Network ready (DHCP OK)");

    // --- MQTT connect with retry ---
    let mut socket = new_socket(stack);

    println!("7. Connecting MQTT...");

    let mut mqtt_connected;
    loop {
        mqtt_connected = try_mqtt_connect(&mut socket).await;
        if mqtt_connected {
            break;
        }
        println!("MQTT failed, retrying in {}s...", RECONNECT_DELAY_MS / 1000);
        Timer::after_millis(RECONNECT_DELAY_MS).await;
    }

    println!("8. MQTT ready");

    let mut publish_counter: u32 = 0;
    let mut silent_polls: u32 = 0;

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
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"open").await;
                }
                State::Wet => {
                    println!("Wet: Closing Valve...");
                    valve.close(&mut delay);
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"close").await;
                }
            }
            last_state = current_state;
        }

        // 2.5 Button toggle (falling edge: pull-up -> GND when pressed)
        let button_pressed = button.is_low();
        if button_pressed && !button_prev_pressed {
            match valve.state() {
                ServoState::Close => {
                    println!("BTN: Opening valve...");
                    valve.open(&mut delay);
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"open").await;
                }
                ServoState::Open => {
                    println!("BTN: Closing valve...");
                    valve.close(&mut delay);
                    let _ = mqtt::publish(&mut socket, TOPIC_MOTOR, b"close").await;
                }
            }
        }
        button_prev_pressed = button_pressed;

        // 3. Poll MQTT for commands (with timeout — non-blocking)
        let poll_fut = mqtt::poll(&mut socket, &mut mqtt_rx);
        let timeout_fut = Timer::after_millis(POLL_TIMEOUT_MS);

        match select(poll_fut, timeout_fut).await {
            Either::First(mqtt::MqttStatus::Event(event)) => {
                silent_polls = 0;
                match event {
                    mqtt::MqttEvent::Publish { topic, payload } => {
                        let topic_str = topic.as_str();
                        if topic_str == Some(TOPIC_MOTOR) {
                            match payload.as_str() {
                                Some("open") => {
                                    println!("MQTT: Opening valve...");
                                    valve.open(&mut delay);
                                }
                                Some("close") => {
                                    println!("MQTT: Closing valve...");
                                    valve.close(&mut delay);
                                }
                                Some(other) => println!("MQTT: unknown motor cmd: {}", other),
                                None => println!("MQTT: motor cmd not valid utf8"),
                            }
                        } else if topic_str == Some(TOPIC_CALIBRATE) {
                            match payload.as_str() {
                                Some(json) => {
                                    if let Some((dry, wet)) = parse_calibration(json) {
                                        println!("MQTT: Calibrating dry={} wet={}", dry, wet);
                                        sensor.calibrate(dry, wet);
                                    } else {
                                        println!("MQTT: failed to parse calibration json");
                                    }
                                }
                                None => println!("MQTT: calibrate not valid utf8"),
                            }
                        }
                    }
                    mqtt::MqttEvent::PingResp => {}
                    mqtt::MqttEvent::SubAck => {}
                    mqtt::MqttEvent::ConnAck => {}
                }
            }
            Either::First(mqtt::MqttStatus::Disconnected) => {
                silent_polls = 0;
                // Socket read error or EOF (remote closed) — connection lost.
                // Previously a closed socket surfaced as a benign "no data" and
                // the loop kept running forever with no clear message; now we
                // detect it and reconnect promptly.
                println!("MQTT: connection lost");
                let _ = reconnect_loop(&mut wifi, &mut socket, stack, &mut mqtt_rx).await;
            }
            Either::First(mqtt::MqttStatus::NoData) => {
                // No complete packet available, nothing to do.
                silent_polls += 1;
                if silent_polls >= MQTT_STALL_THRESHOLD {
                    println!("MQTT: RX stalled ({} silent polls), forcing reconnect", silent_polls);
                    silent_polls = 0;
                    let _ = reconnect_loop(&mut wifi, &mut socket, stack, &mut mqtt_rx).await;
                }
            }
            Either::Second(_) => {
                // Timeout — no MQTT data, that's fine.
                silent_polls += 1;
                if silent_polls >= MQTT_STALL_THRESHOLD {
                    println!("MQTT: RX stalled ({} silent polls), forcing reconnect", silent_polls);
                    silent_polls = 0;
                    let _ = reconnect_loop(&mut wifi, &mut socket, stack, &mut mqtt_rx).await;
                }
            }
        }

        // 4. Publish sensor data periodically
        if publish_counter >= 5 {
            publish_counter = 0;
            let mut payload = [0u8; 8];
            let len = format_number(humidity, &mut payload);
            if mqtt::publish(&mut socket, TOPIC_SOIL, &payload[..len])
                .await
                .is_err()
            {
                println!("MQTT: publish failed, reconnecting...");
                let _ = reconnect_loop(&mut wifi, &mut socket, stack, &mut mqtt_rx).await;
            }
        }
        publish_counter += 1;

        // 5. Log sensor metrics
        println!("Raw: {} | Humidity: {}%", raw, humidity);

        // 6. Wait
        Timer::after_millis(1000).await;
    }
}

fn new_socket<'a>(stack: embassy_net::Stack<'a>) -> TcpSocket<'a> {
    TcpSocket::new(
        stack,
        unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX_BUF) },
        unsafe { &mut *core::ptr::addr_of_mut!(TCP_TX_BUF) },
    )
}

async fn try_mqtt_connect(socket: &mut TcpSocket<'_>) -> bool {
    socket.close();

    let addr = (
        Ipv4Addr::new(
            MQTT_BROKER_IP[0],
            MQTT_BROKER_IP[1],
            MQTT_BROKER_IP[2],
            MQTT_BROKER_IP[3],
        ),
        MQTT_BROKER_PORT,
    );

    let tcp_result = match select(
        socket.connect(addr),
        Timer::after_millis(TCP_CONNECT_TIMEOUT_MS),
    )
    .await
    {
        Either::First(result) => result,
        Either::Second(_) => {
            println!("TCP connect timed out");
            return false;
        }
    };

    match tcp_result {
        Ok(()) => {
            if mqtt::connect(socket, MQTT_CLIENT_ID, 60).await.is_ok() {
                let _ = mqtt::subscribe(socket, TOPIC_MOTOR, 1).await;
                let _ = mqtt::subscribe(socket, TOPIC_CALIBRATE, 2).await;
                println!("MQTT connected + subscribed");
                return true;
            }
            println!("MQTT protocol handshake failed");
        }
        Err(e) => {
            println!("TCP connect failed: {:?}", e);
        }
    }
    false
}

async fn reconnect_loop<'a>(
    wifi: &mut esp_radio::wifi::WifiController<'a>,
    socket: &mut TcpSocket<'a>,
    stack: embassy_net::Stack<'a>,
    mqtt_rx: &mut mqtt::MqttRx<'_>,
) -> bool {
    println!("Attempting MQTT reconnect...");

    loop {
        // If the network link or IP config is down, re-establish WiFi+DHCP.
        if !stack.is_link_up() || !stack.is_config_up() {
            println!("Network down (link={}, config={}), reconnecting WiFi...",
                stack.is_link_up(), stack.is_config_up());

            let _ = wifi.disconnect_async().await;
            Timer::after_millis(500).await;
            match wifi.connect_async().await {
                Ok(()) => println!("WiFi reconnected"),
                Err(e) => println!("WiFi reconnect failed: {:?}", e),
            }

            // Wait for DHCP to assign an IP again.
            let link_future = stack.wait_link_up();
            let timeout = Timer::after_millis(15000);
            match select(link_future, timeout).await {
                Either::First(_) => {}
                Either::Second(_) => {
                    println!("Timed out waiting for link up");
                    Timer::after_millis(RECONNECT_DELAY_MS).await;
                    continue;
                }
            }

            let cfg_future = stack.wait_config_up();
            let timeout = Timer::after_millis(15000);
            match select(cfg_future, timeout).await {
                Either::First(_) => println!("Network ready again (DHCP OK)"),
                Either::Second(_) => {
                    println!("Timed out waiting for DHCP");
                    Timer::after_millis(RECONNECT_DELAY_MS).await;
                    continue;
                }
            }
        }

        // Recreate socket (old one may be in bad state)
        *socket = new_socket(stack);
        // Drop any stale partial MQTT bytes buffered from the dead connection
        // so they can't desync the freshly (re)subscribed connection.
        mqtt_rx.clear();

        if try_mqtt_connect(socket).await {
            println!("MQTT reconnected");
            return true;
        }

        println!("Reconnect failed, retrying in {}s...", RECONNECT_DELAY_MS / 1000);
        Timer::after_millis(RECONNECT_DELAY_MS).await;
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
