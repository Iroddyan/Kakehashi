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
    pub devpath: Option<String>,
    pub product: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
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
            devpath: None,
            product: None,
            model: None,
            device: None,
        }
    }

    /// Add optional metadata shown by `host:devices-l`.
    pub fn with_metadata(
        mut self,
        devpath: Option<&str>,
        product: Option<&str>,
        model: Option<&str>,
        device: Option<&str>,
    ) -> Self {
        self.devpath = devpath.map(str::to_owned);
        self.product = product.map(str::to_owned);
        self.model = model.map(str::to_owned);
        self.device = device.map(str::to_owned);
        self
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
    version: Arc<RwLock<String>>,
    failures: Arc<RwLock<std::collections::HashMap<String, String>>>,
    accept_task: JoinHandle<()>,
}

impl FakeAdbServer {
    /// Start listening on `127.0.0.1` at an ephemeral port.
    pub async fn start() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let devices = Arc::new(RwLock::new(Vec::new()));
        let version = Arc::new(RwLock::new(HOST_VERSION.to_owned()));
        let failures = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let accept_devices = Arc::clone(&devices);
        let accept_version = Arc::clone(&version);
        let accept_failures = Arc::clone(&failures);

        let accept_task = tokio::spawn(async move {
            loop {
                let Ok((stream, _peer)) = listener.accept().await else {
                    break;
                };
                let devices = Arc::clone(&accept_devices);
                let version = Arc::clone(&accept_version);
                let failures = Arc::clone(&accept_failures);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, devices, version, failures).await;
                });
            }
        });

        Ok(Self {
            addr,
            devices,
            version,
            failures,
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

    /// Set the four hexadecimal digits returned by `host:version`.
    pub async fn set_version(&self, version: impl Into<String>) -> io::Result<()> {
        let version = version.into();
        if version.len() != 4 || !version.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "host version must contain exactly four hexadecimal digits",
            ));
        }
        *self.version.write().await = version;
        Ok(())
    }

    /// Configure a request to receive a length-prefixed `FAIL` reply.
    pub async fn set_failure(&self, service: impl Into<String>, reason: impl Into<String>) {
        self.failures
            .write()
            .await
            .insert(service.into(), reason.into());
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
    version: Arc<RwLock<String>>,
    failures: Arc<RwLock<std::collections::HashMap<String, String>>>,
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

    let request_text = String::from_utf8_lossy(&request);
    let response = if let Some(reason) = failures.read().await.get(request_text.as_ref()) {
        fail_payload(reason.as_bytes())?
    } else {
        match request.as_slice() {
            b"host:version" => okay_payload(version.read().await.as_bytes())?,
            b"host:devices" => {
                okay_payload(format_devices(&devices.read().await, false).as_bytes())?
            }
            b"host:devices-l" => {
                okay_payload(format_devices(&devices.read().await, true).as_bytes())?
            }
            // UNVERIFIED: the pinned AOSP host dispatcher can report an unhandled service, but its
            // final wire behavior is not established here. This default keeps T5 tests scriptable.
            _ => fail_payload(b"unsupported fake service")?,
        }
    };

    stream.write_all(&response).await?;
    stream.shutdown().await
}

fn fail_payload(reason: &[u8]) -> io::Result<Vec<u8>> {
    if reason.len() > MAX_REQUEST_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "failure reason exceeds smart-socket limit",
        ));
    }
    let prefix = format!("{:04x}", reason.len());
    let mut response = Vec::with_capacity(8 + reason.len());
    response.extend_from_slice(b"FAIL");
    response.extend_from_slice(prefix.as_bytes());
    response.extend_from_slice(reason);
    Ok(response)
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
        use std::fmt::Write as _;
        if long_listing {
            // AOSP `transport.cpp` at commit 1cf2f017d312f73b3dc53bda85ef2610e35a80e9
            // formats long rows with a 22-character serial column, optional devpath/product/model/
            // device fields, and trailing transport_id. UNVERIFIED: metadata sanitization rules.
            let _ = write!(output, "{:<22} {}", device.serial, device.state);
            for (key, value) in [
                ("devpath", device.devpath.as_deref()),
                ("product", device.product.as_deref()),
                ("model", device.model.as_deref()),
                ("device", device.device.as_deref()),
            ] {
                if let Some(value) = value {
                    let _ = write!(output, " {key}:{value}");
                }
            }
            let _ = writeln!(output, " transport_id:{}", device.transport_id);
        } else {
            let _ = writeln!(output, "{}\t{}", device.serial, device.state);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{FakeAdbServer, FakeDevice, FakeTransportType};
    use kakehashi_core::smart_socket::{
        decode_length_prefixed_payload, decode_status, encode_request, SmartSocketStatus,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn query(server: &FakeAdbServer, service: &[u8]) -> (SmartSocketStatus, Vec<u8>) {
        let mut stream = TcpStream::connect(server.local_addr()).await.unwrap();
        stream
            .write_all(&encode_request(service).unwrap())
            .await
            .unwrap();

        let mut status = [0; 4];
        stream.read_exact(&mut status).await.unwrap();
        let status = decode_status(&status).unwrap();

        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&prefix).unwrap(), 16).unwrap();
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).await.unwrap();

        let mut response = Vec::with_capacity(8 + length);
        response.extend_from_slice(match status {
            SmartSocketStatus::Okay => b"OKAY",
            SmartSocketStatus::Fail => b"FAIL",
        });
        response.extend_from_slice(&prefix);
        response.extend_from_slice(&payload);
        let (payload, consumed) = decode_length_prefixed_payload(&response[4..]).unwrap();
        assert_eq!(consumed, 4 + length);
        (status, payload)
    }

    #[tokio::test]
    async fn server_binds_an_ephemeral_loopback_port_and_returns_version() {
        let server = FakeAdbServer::start().await.unwrap();
        assert_eq!(server.local_addr().ip().to_string(), "127.0.0.1");
        assert_ne!(server.local_addr().port(), 5037);
        assert_eq!(
            query(&server, b"host:version").await,
            (SmartSocketStatus::Okay, b"0029".to_vec())
        );
        server.set_version("0041").await.unwrap();
        assert_eq!(
            query(&server, b"host:version").await,
            (SmartSocketStatus::Okay, b"0041".to_vec())
        );
        assert!(server.set_version("xyz").await.is_err());
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
        assert_eq!(short.0, SmartSocketStatus::Okay);
        assert_eq!(short.1, b"serial-a\tdevice\nserial-b\toffline\n");

        server
            .set_devices(vec![FakeDevice::new(
                "phone",
                "device",
                9,
                FakeTransportType::Usb,
            )
            .with_metadata(
                Some("usb:1-2"),
                Some("acme"),
                Some("model_x"),
                Some("device_x"),
            )])
            .await;
        let long = query(&server, b"host:devices-l").await;
        assert_eq!(long.0, SmartSocketStatus::Okay);
        assert_eq!(
            long.1,
            format!(
                "{:<22} device devpath:usb:1-2 product:acme model:model_x device:device_x transport_id:9\n",
                "phone"
            )
            .as_bytes()
        );
    }

    #[tokio::test]
    async fn configured_failure_is_returned_as_a_typed_fail_frame() {
        let server = FakeAdbServer::start().await.unwrap();
        server.set_failure("host:missing", "scripted failure").await;

        assert_eq!(
            query(&server, b"host:missing").await,
            (SmartSocketStatus::Fail, b"scripted failure".to_vec())
        );
        assert_eq!(
            query(&server, b"host:unknown").await,
            (
                SmartSocketStatus::Fail,
                b"unsupported fake service".to_vec()
            )
        );
    }
}
