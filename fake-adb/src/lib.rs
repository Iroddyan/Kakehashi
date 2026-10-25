//! A small fake ADB smart-socket server for tests.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

// Captured in tests/fixtures/host-version-response.txt from ADB 1.0.41 / server 37.0.0p1.
// This version payload is only verified for that local server build.
const HOST_VERSION: &str = "0029";
const MAX_REQUEST_LEN: usize = 0xffff;

/// A configured device record returned by the fake server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeDevice {
    pub serial: String,
    pub state: String,
    pub transport_id: u32,
    pub transport_type: FakeTransportType,
}

impl FakeDevice {
    /// Create a device row for `host:devices` and `host:devices-l` replies.
    pub fn new(
        serial: impl Into<String>,
        state: impl Into<String>,
        transport_id: u32,
        transport_type: FakeTransportType,
    ) -> Self {
        Self {
            serial: serial.into(),
            state: state.into(),
            transport_id,
            transport_type,
        }
    }
}

/// Transport category used for the AOSP device-list ordering.
///
/// The declaration order mirrors `TransportType` in `platform/packages/modules/adb/adb.h`
/// at commit `1cf2f017d312f73b3dc53bda85ef2610e35a80e9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FakeTransportType {
    Usb,
    Local,
    Any,
}

/// An async fake adb server bound to a loopback port selected by the OS.
pub struct FakeAdbServer {
    addr: SocketAddr,
    devices: Arc<RwLock<Vec<FakeDevice>>>,
    accept_task: JoinHandle<()>,
}

impl FakeAdbServer {
    /// Start listening on `127.0.0.1` at an ephemeral port.
    pub async fn start() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let devices = Arc::new(RwLock::new(Vec::new()));
        let accept_devices = Arc::clone(&devices);

        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    break;
                };
                let devices = Arc::clone(&accept_devices);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, devices).await;
                });
            }
        });

        Ok(Self {
            addr,
            devices,
            accept_task,
        })
    }

    /// The loopback address clients can connect to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Replace the device rows returned by subsequent `host:devices*` requests.
    pub async fn set_devices(&self, devices: Vec<FakeDevice>) {
        *self.devices.write().await = devices;
    }
}

impl Drop for FakeAdbServer {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    devices: Arc<RwLock<Vec<FakeDevice>>>,
) -> io::Result<()> {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).await?;
    let request_len = parse_hex_length(&prefix)?;
    if request_len > MAX_REQUEST_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request length exceeds smart-socket limit",
        ));
    }

    let mut request = vec![0; request_len];
    stream.read_exact(&mut request).await?;

    let response = match request.as_slice() {
        b"host:version" => okay_payload(HOST_VERSION.as_bytes())?,
        b"host:devices" => okay_payload(format_devices(&devices.read().await, false).as_bytes())?,
        b"host:devices-l" => okay_payload(format_devices(&devices.read().await, true).as_bytes())?,
        _ => return Ok(()),
    };

    stream.write_all(&response).await?;
    stream.shutdown().await
}

fn parse_hex_length(prefix: &[u8; 4]) -> io::Result<usize> {
    prefix.iter().try_fold(0usize, |value, digit| {
        let nibble = match digit {
            b'0'..=b'9' => usize::from(digit - b'0'),
            b'a'..=b'f' => usize::from(digit - b'a' + 10),
            b'A'..=b'F' => usize::from(digit - b'A' + 10),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "request length is not hexadecimal",
                ))
            }
        };
        Ok((value << 4) | nibble)
    })
}

fn okay_payload(payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.len() > MAX_REQUEST_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "response payload exceeds smart-socket limit",
        ));
    }
    // AOSP `docs/dev/services.md` at commit 1cf2f017d312f73b3dc53bda85ef2610e35a80e9
    // specifies that host:devices replies follow OKAY with a four-hex-digit length and payload.
    let prefix = format!("{:04x}", payload.len());
    let mut response = Vec::with_capacity(8 + payload.len());
    response.extend_from_slice(b"OKAY");
    response.extend_from_slice(prefix.as_bytes());
    response.extend_from_slice(payload);
    Ok(response)
}

fn format_devices(devices: &[FakeDevice], long_listing: bool) -> String {
    let mut sorted = devices.to_vec();
    // AOSP `transport.cpp` at commit 1cf2f017d312f73b3dc53bda85ef2610e35a80e9 sorts by
    // transport type and then serial before formatting the device-list response.
    sorted.sort_by(|left, right| {
        left.transport_type
            .cmp(&right.transport_type)
            .then_with(|| left.serial.cmp(&right.serial))
    });

    let mut output = String::new();
    for device in sorted {
        if long_listing {
            // AOSP `transport.cpp` at commit 1cf2f017d312f73b3dc53bda85ef2610e35a80e9
            // formats long rows with a 22-character serial column and trailing transport_id.
            use std::fmt::Write as _;
            let _ = writeln!(
                output,
                "{:<22} {} transport_id:{}",
                device.serial, device.state, device.transport_id
            );
        } else {
            use std::fmt::Write as _;
            let _ = writeln!(output, "{}\t{}", device.serial, device.state);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{FakeAdbServer, FakeDevice, FakeTransportType};
    use kakehashi_core::smart_socket::encode_request;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn query(server: &FakeAdbServer, service: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect(server.local_addr()).await.unwrap();
        stream
            .write_all(&encode_request(service).unwrap())
            .await
            .unwrap();

        let mut status = [0; 4];
        stream.read_exact(&mut status).await.unwrap();
        assert_eq!(&status, b"OKAY");

        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&prefix).unwrap(), 16).unwrap();
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).await.unwrap();

        let mut response = Vec::with_capacity(8 + length);
        response.extend_from_slice(&status);
        response.extend_from_slice(&prefix);
        response.extend_from_slice(&payload);
        response
    }

    #[tokio::test]
    async fn server_binds_an_ephemeral_loopback_port_and_returns_version() {
        let server = FakeAdbServer::start().await.unwrap();
        assert_eq!(server.local_addr().ip().to_string(), "127.0.0.1");
        assert_ne!(server.local_addr().port(), 5037);
        assert_eq!(query(&server, b"host:version").await, b"OKAY00040029");
    }

    #[tokio::test]
    async fn devices_services_return_two_configured_devices() {
        let server = FakeAdbServer::start().await.unwrap();
        server
            .set_devices(vec![
                FakeDevice::new("serial-b", "offline", 2, FakeTransportType::Local),
                FakeDevice::new("serial-a", "device", 1, FakeTransportType::Local),
            ])
            .await;

        let short = query(&server, b"host:devices").await;
        assert_eq!(short, b"OKAY0021serial-a\tdevice\nserial-b\toffline\n");

        let long = query(&server, b"host:devices-l").await;
        assert_eq!(
            long,
            b"OKAY005bserial-a               device transport_id:1\nserial-b               offline transport_id:2\n"
        );
    }
}
