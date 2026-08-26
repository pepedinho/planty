use embedded_io_async::Write;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_time::Timer;

const WRITE_TIMEOUT_MS: u64 = 1000;
const READ_TIMEOUT_MS: u64 = 2000;

#[derive(Debug)]
pub enum MqttError {
    ConnectionFailed,
    ProtocolError,
    IoError,
    Timeout,
}

#[derive(Debug, Clone)]
pub enum MqttEvent<'a> {
    Publish {
        topic: &'a str,
        payload: &'a [u8],
    },
    PingResp,
    SubAck,
    ConnAck,
}

fn encode_variable_length(mut len: usize, buf: &mut [u8]) -> usize {
    let mut idx = 0;
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        buf[idx] = byte;
        idx += 1;
        if len == 0 {
            break;
        }
    }
    idx
}

fn decode_variable_length(buf: &[u8]) -> Result<(usize, usize), MqttError> {
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    let mut idx = 0;
    loop {
        if idx >= buf.len() {
            return Err(MqttError::ProtocolError);
        }
        let byte = buf[idx];
        value += (byte & 0x7F) as usize * multiplier;
        idx += 1;
        if byte & 0x80 == 0 {
            break;
        }
        multiplier *= 128;
        if multiplier > 128 * 128 * 128 {
            return Err(MqttError::ProtocolError);
        }
    }
    Ok((value, idx))
}

pub async fn connect(
    socket: &mut TcpSocket<'_>,
    client_id: &str,
    keep_alive_secs: u16,
) -> Result<(), MqttError> {
    let mut buf = [0u8; 256];
    let mut idx = 0;

    // Variable header
    let mut var_header = [0u8; 10];
    var_header[0] = 0x00;
    var_header[1] = 0x04;
    var_header[2] = b'M';
    var_header[3] = b'Q';
    var_header[4] = b'T';
    var_header[5] = b'T';
    var_header[6] = 0x04; // Protocol Level (v3.1.1)
    var_header[7] = 0x02; // Connect Flags: Clean Session
    var_header[8] = (keep_alive_secs >> 8) as u8;
    var_header[9] = (keep_alive_secs & 0xFF) as u8;

    let client_id_bytes = client_id.as_bytes();
    let payload_len = 2 + client_id_bytes.len();
    let total_len = var_header.len() + payload_len;

    // Fixed header
    buf[idx] = 0x10; // CONNECT
    idx += 1;
    idx += encode_variable_length(total_len, &mut buf[idx..]);

    buf[idx..idx + var_header.len()].copy_from_slice(&var_header);
    idx += var_header.len();

    buf[idx] = (client_id_bytes.len() >> 8) as u8;
    buf[idx + 1] = (client_id_bytes.len() & 0xFF) as u8;
    idx += 2;
    buf[idx..idx + client_id_bytes.len()].copy_from_slice(client_id_bytes);
    idx += client_id_bytes.len();

    match select(
        socket.write_all(&buf[..idx]),
        Timer::after_millis(WRITE_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(())) => {}
        Either::First(Err(_)) => return Err(MqttError::IoError),
        Either::Second(_) => return Err(MqttError::Timeout),
    }

    // Wait for CONNACK
    let mut resp = [0u8; 4];
    match select(
        socket.read(&mut resp),
        Timer::after_millis(READ_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(n)) => {
            if n < 4 || resp[0] != 0x20 || resp[3] != 0x00 {
                return Err(MqttError::ConnectionFailed);
            }
        }
        Either::First(Err(_)) => return Err(MqttError::IoError),
        Either::Second(_) => return Err(MqttError::Timeout),
    }

    Ok(())
}

pub async fn subscribe(
    socket: &mut TcpSocket<'_>,
    topic: &str,
    packet_id: u16,
) -> Result<(), MqttError> {
    let mut buf = [0u8; 128];
    let mut idx = 0;

    let topic_bytes = topic.as_bytes();
    let variable_len = 2 + 2 + topic_bytes.len() + 1; // packet_id + topic_len + topic + qos

    buf[idx] = 0x82; // SUBSCRIBE (type 8, flags 2)
    idx += 1;
    idx += encode_variable_length(variable_len, &mut buf[idx..]);

    buf[idx] = (packet_id >> 8) as u8;
    buf[idx + 1] = (packet_id & 0xFF) as u8;
    idx += 2;

    buf[idx] = (topic_bytes.len() >> 8) as u8;
    buf[idx + 1] = (topic_bytes.len() & 0xFF) as u8;
    idx += 2;
    buf[idx..idx + topic_bytes.len()].copy_from_slice(topic_bytes);
    idx += topic_bytes.len();

    buf[idx] = 0x00; // QoS 0
    idx += 1;

    match select(
        socket.write_all(&buf[..idx]),
        Timer::after_millis(WRITE_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(())) => {}
        Either::First(Err(_)) => return Err(MqttError::IoError),
        Either::Second(_) => return Err(MqttError::Timeout),
    }

    // Wait for SUBACK
    let mut resp = [0u8; 5];
    match select(
        socket.read(&mut resp),
        Timer::after_millis(READ_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(n)) => {
            if n < 4 || resp[0] != 0x90 {
                return Err(MqttError::ProtocolError);
            }
        }
        Either::First(Err(_)) => return Err(MqttError::IoError),
        Either::Second(_) => return Err(MqttError::Timeout),
    }

    Ok(())
}

pub async fn publish(
    socket: &mut TcpSocket<'_>,
    topic: &str,
    payload: &[u8],
) -> Result<(), MqttError> {
    let mut buf = [0u8; 256];
    let mut idx = 0;

    let topic_bytes = topic.as_bytes();
    let variable_len = 2 + topic_bytes.len() + payload.len();

    buf[idx] = 0x30; // PUBLISH (QoS 0, no retain)
    idx += 1;
    idx += encode_variable_length(variable_len, &mut buf[idx..]);

    buf[idx] = (topic_bytes.len() >> 8) as u8;
    buf[idx + 1] = (topic_bytes.len() & 0xFF) as u8;
    idx += 2;
    buf[idx..idx + topic_bytes.len()].copy_from_slice(topic_bytes);
    idx += topic_bytes.len();

    let payload_end = idx + payload.len();
    if payload_end > buf.len() {
        return Err(MqttError::ProtocolError);
    }
    buf[idx..payload_end].copy_from_slice(payload);
    idx = payload_end;

    match select(
        socket.write_all(&buf[..idx]),
        Timer::after_millis(WRITE_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(())) => {}
        Either::First(Err(_)) => return Err(MqttError::IoError),
        Either::Second(_) => return Err(MqttError::Timeout),
    }

    Ok(())
}

pub async fn ping(socket: &mut TcpSocket<'_>) -> Result<(), MqttError> {
    match select(
        socket.write_all(&[0xC0, 0x00]),
        Timer::after_millis(WRITE_TIMEOUT_MS),
    )
    .await
    {
        Either::First(Ok(())) => Ok(()),
        Either::First(Err(_)) => Err(MqttError::IoError),
        Either::Second(_) => Err(MqttError::Timeout),
    }
}

pub async fn poll<'a>(
    socket: &mut TcpSocket<'_>,
    rx_buf: &'a mut [u8],
) -> Option<MqttEvent<'a>> {
    let n = match socket.read(rx_buf).await {
        Ok(n) if n > 0 => n,
        _ => return None,
    };

    let data = &rx_buf[..n];
    let packet_type = (data[0] >> 4) & 0x0F;

    match packet_type {
        0x03 => {
            // PUBLISH
            if data.len() < 4 {
                return None;
            }
            let (_remaining_len, var_len_bytes) = decode_variable_length(&data[1..]).ok()?;
            let var_start = 1 + var_len_bytes;
            if data.len() < var_start + 2 {
                return None;
            }
            let topic_len = ((data[var_start] as usize) << 8) | (data[var_start + 1] as usize);
            let topic_start = var_start + 2;
            let topic_end = topic_start + topic_len;
            if topic_end > data.len() {
                return None;
            }
            let topic = core::str::from_utf8(&data[topic_start..topic_end]).ok()?;
            let payload = &data[topic_end..];
            Some(MqttEvent::Publish { topic, payload })
        }
        0x0D => Some(MqttEvent::PingResp),
        0x90 => Some(MqttEvent::SubAck),
        0x20 => Some(MqttEvent::ConnAck),
        _ => None,
    }
}
