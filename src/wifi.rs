use embassy_net::{Config as NetConfig, Stack, StackResources, Runner};
pub use esp_radio::wifi::WifiDevice;

const SOCKET_COUNT: usize = 5;

pub fn connect<'d>(
    controller: &'d esp_radio::Controller<'d>,
    wifi_device: esp_hal::peripherals::WIFI<'d>,
    ssid: &str,
    password: &str,
) -> Result<(esp_radio::wifi::WifiController<'d>, WifiDevice<'d>), esp_radio::wifi::WifiError> {
    let (mut wifi, interfaces) =
        esp_radio::wifi::new(controller, wifi_device, Default::default())?;

    let client_config = esp_radio::wifi::ClientConfig::default()
        .with_ssid(alloc::string::String::from(ssid))
        .with_password(alloc::string::String::from(password))
        .with_auth_method(esp_radio::wifi::AuthMethod::Wpa2Personal);
    wifi.set_config(&esp_radio::wifi::ModeConfig::Client(client_config))?;

    Ok((wifi, interfaces.sta))
}

pub fn setup_stack<'d>(
    device: WifiDevice<'d>,
    resources: &'d mut StackResources<SOCKET_COUNT>,
    random_seed: u64,
) -> (Stack<'d>, Runner<'d, WifiDevice<'d>>) {
    embassy_net::new(
        device,
        NetConfig::dhcpv4(Default::default()),
        resources,
        random_seed,
    )
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await;
}
