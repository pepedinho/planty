use embedded_io_async::Write;
use embassy_futures::select::{select, Either};
use embassy_net::tcp::TcpSocket;
use embassy_time::Timer;

const WRITE_TIMEOUT_MS: u64 = 1000;
const READ_TIMEOUT_MS: u64 = 2000;

/// Persistent receive state for MQTT. The buffer accumulates incoming bytes
/// across `poll` calls so that partial or coalesced TCP segments are handled
/// correctly. Only one complete MQTT packet is decoded per `poll` call; the
/// remainder is kept for the next call.
///
/// The unread portion lives in `buf[start..end]`. Consuming a packet advances
/// `start`; when the buffer would otherwise be exhausted we reset both
/// indices. No bytes are physically moved while an event derived from the
/// buffer is still alive, so returned `MqttEvent`s may borrow the buffer.
pub struct MqttRx<'a> {
    buf: &'a mut [u8],
    start: usize,
    end: usize,
}

impl<'a> MqttRx<'a> {
    /// Creates a new receive state backed by the given buffer.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            start: 0,
            end: 0,
        }
    }

    /// Returns how many bytes are currently buffered (unconsumed).
    pub fn pending(&self) -> usize {
        self.end - self.start
    }
}

#[derive(Debug)]
pub enum MqttError {
    ConnectionFailed,
    ProtocolError,
    IoError,
    Timeout,
}

/// A fully-decoded MQTT event, with owned copies of topic and payload so it
/// does not borrow from the receive buffer. Sized for this application's
/// topics and payloads.
pub enum MqttEvent {
    Publish {
        topic: ArrayBuf<64>,
        payload: ArrayBuf<256>,
    },
    PingResp,
    SubAck,
    ConnAck,
}

/// A fixed-capacity owned byte buffer with a length.
pub struct ArrayBuf<const N: usize> {
    data: [u8; N],
    len: usize,
}

impl<const N: usize> ArrayBuf<N> {
    fn new() -> Self {
        Self { data: [0u8; N], len: 0 }
    }

    /// Copies `src` into the buffer (truncating if it doesn't fit).
    fn copy_from(&mut self, src: &[u8]) {
        let n = src.len().min(N);
        self.data[..n].copy_from_slice(&src[..n]);
        self.len = n;
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data[..self.len]
    }

    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_slice()).ok()
    }
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

/// Parses one MQTT packet from the front of `data` (the full buffered bytes).
/// Returns `(event, consumed)` where `consumed` is the number of bytes the
/// packet occupies, or `None` if the data is incomplete (need more bytes) or
/// the packet type is unknown.
fn decode_packet(data: &[u8]) -> Option<(MqttEvent, usize)> {
    if data.is_empty() {
        return None;
    }

    let first = data[0];
    let packet_type = (first >> 4) & 0x0F;

    // Decode the "remaining length" varint to know the full packet size.
    let (remaining_len, var_len_bytes) = match decode_variable_length(&data[1..]) {
        Ok(v) => v,
        Err(_) => return None,
    };

    let total = 1 + var_len_bytes + remaining_len;
    if data.len() < total {
        // Incomplete packet — need more bytes before we can parse it.
        return None;
    }

    // Body starts after fixed header (1 type byte + remaining-length varint).
    let body_start = 1 + var_len_bytes;
    let body = &data[body_start..total];

    match packet_type {
        0x03 => {
            // PUBLISH
            if body.len() < 2 {
                return None;
            }
            let topic_len = ((body[0] as usize) << 8) | (body[1] as usize);
            let topic_start = 2;
            let topic_end = topic_start + topic_len;
            if topic_end > body.len() {
                return None;
            }
            let mut topic = ArrayBuf::new();
            topic.copy_from(&body[topic_start..topic_end]);
            let mut payload = ArrayBuf::new();
            payload.copy_from(&body[topic_end..]);
            Some((MqttEvent::Publish { topic, payload }, total))
        }
        0x0D => Some((MqttEvent::PingResp, total)),
        0x90 => Some((MqttEvent::SubAck, total)),
        0x20 => Some((MqttEvent::ConnAck, total)),
        _ => None,
    }
}

/// Reads any available MQTT data from the socket into the persistent receive
/// state and decodes one complete packet if possible.
///
/// Returns `MqttStatus::Event` when a full packet was decoded (consuming it),
/// `MqttStatus::NoData` when nothing was read or no complete packet is
/// available yet, and `MqttStatus::Disconnected` only when the socket is
/// actually broken (read error) so the caller knows to reconnect.
pub enum MqttStatus {
    Event(MqttEvent),
    NoData,
    Disconnected,
}

pub async fn poll(socket: &mut TcpSocket<'_>, rx: &mut MqttRx<'_>) -> MqttStatus {
    // First, try to decode a packet from any leftover buffered data.
    let decoded = {
        let data = &rx.buf[rx.start..rx.end];
        decode_packet(data)
    };
    if let Some((event, consumed)) = decoded {
        advance(rx, consumed);
        return MqttStatus::Event(event);
    }

    // No complete packet yet — if the buffer is full we're out of sync;
    // drop everything and start fresh.
    if rx.end == rx.buf.len() {
        rx.start = 0;
        rx.end = 0;
    }

    // Read more data into the free space at the end of the buffer.
    // `available` is disjoint from `buf[start..end]` because we only read
    // into `buf[end..]` and nothing above references the consumed region.
    let n = {
        let available = &mut rx.buf[rx.end..];
        if available.is_empty() {
            return MqttStatus::NoData;
        }
        match socket.read(available).await {
            Ok(n) if n > 0 => n,
            Ok(_) => return MqttStatus::NoData,
            Err(_) => return MqttStatus::Disconnected,
        }
    };
    rx.end += n;

    // Try to decode a packet from the data available so far.
    let decoded = {
        let data = &rx.buf[rx.start..rx.end];
        decode_packet(data)
    };
    if let Some((event, consumed)) = decoded {
        advance(rx, consumed);
        MqttStatus::Event(event)
    } else {
        MqttStatus::NoData
    }
}

/// Advances the read pointer past `consumed` bytes. Only adjusts indices,
/// never moves bytes, so buffers referenced by a live `MqttEvent` stay valid.
fn advance(rx: &mut MqttRx<'_>, consumed: usize) {
    rx.start += consumed;
    if rx.start == rx.end {
        rx.start = 0;
        rx.end = 0;
    }
}
