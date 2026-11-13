//! Serial console.
//!
//! The `serialport` crate only offers blocking I/O, so each connection gets
//! two dedicated threads (one reading, one writing) that talk to an async
//! pump through channels. Ports are enumerated without libudev: from sysfs on
//! Linux, IOKit on macOS and the registry on Windows.

use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serialport::SerialPort;
use tokio::sync::mpsc;

use crate::connection::{Backend, Connection, ConnectionInfo, Input, Output, TransportKind};
use crate::error::TransportError;
use crate::pump::{Outcome, ReadEnd, WriteEnd, drive};
use crate::relay::Relay;

/// How often a blocked read wakes up to see whether it should stop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Length of a line break: long enough for a Cisco ROMMON to notice.
const BREAK_DURATION: Duration = Duration::from_millis(250);
const READ_BUFFER: usize = 4096;
/// Unread output staged before the reading thread is made to wait.
const OUTPUT_HIGH_WATER: usize = 1024 * 1024;
/// How long the port threads get to let go of the port when closing.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// How to open a serial port.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialParams {
    /// `COM3`, `/dev/ttyUSB0`, …
    pub port: String,
    pub baud: u32,
    /// 5 to 8.
    pub data_bits: u8,
    pub parity: Parity,
    /// 1 or 2.
    pub stop_bits: u8,
    pub flow: FlowControl,
}

impl Default for SerialParams {
    /// 9600 8N1 without flow control: what a network console expects.
    fn default() -> Self {
        Self {
            port: String::new(),
            baud: 9600,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            flow: FlowControl::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowControl {
    #[default]
    None,
    /// XON/XOFF.
    Software,
    /// RTS/CTS.
    Hardware,
}

/// A serial port present on this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerialPortInfo {
    /// What to pass as [`SerialParams::port`].
    pub name: String,
    pub description: String,
    pub manufacturer: Option<String>,
    pub usb_vid: Option<u16>,
    pub usb_pid: Option<u16>,
}

/// The serial ports of this machine, in natural order (`COM2` before `COM10`).
pub fn list_serial_ports() -> Vec<SerialPortInfo> {
    // Without libudev, `serialport` reads sysfs and panics if it is missing
    // (some containers).
    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/sys/class/tty").is_dir() {
        return Vec::new();
    }
    let mut ports: Vec<SerialPortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(describe)
        .collect();
    ports.sort_by(|a, b| natural_key(&a.name).cmp(&natural_key(&b.name)));
    ports
}

fn describe(port: serialport::SerialPortInfo) -> SerialPortInfo {
    let mut info = SerialPortInfo {
        name: port.port_name,
        description: "Serial port".to_owned(),
        manufacturer: None,
        usb_vid: None,
        usb_pid: None,
    };
    match port.port_type {
        serialport::SerialPortType::UsbPort(usb) => {
            info.description = usb
                .product
                .filter(|product| !product.trim().is_empty())
                .unwrap_or_else(|| "USB serial port".to_owned());
            info.manufacturer = usb.manufacturer;
            info.usb_vid = Some(usb.vid);
            info.usb_pid = Some(usb.pid);
        }
        serialport::SerialPortType::PciPort => info.description = "PCI serial port".to_owned(),
        serialport::SerialPortType::BluetoothPort => {
            info.description = "Bluetooth serial port".to_owned();
        }
        serialport::SerialPortType::Unknown => {}
    }
    info
}

/// Sorts `ttyUSB10` after `ttyUSB2`.
fn natural_key(name: &str) -> (&str, u64) {
    let stem = name.trim_end_matches(|c: char| c.is_ascii_digit());
    (stem, name[stem.len()..].parse().unwrap_or(0))
}

/// Opens a serial port.
///
/// `Input::Break` holds the line in the break state for 250 ms (how Cisco
/// ROMMON / password recovery is entered); `Input::Resize` means nothing to
/// a serial line and is ignored.
pub async fn connect_serial(params: &SerialParams) -> Result<Connection, TransportError> {
    let settings = Settings::try_from(params)?;
    let opening = tokio::task::spawn_blocking(move || Port::open(&settings));
    let port = opening
        .await
        .map_err(|error| TransportError::Io(io::Error::other(error)))??;

    let info = ConnectionInfo {
        kind: Some(TransportKind::Serial),
        peer: params.port.clone(),
        ..ConnectionInfo::default()
    };
    let (connection, backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
    tokio::spawn(run(port, backend));
    Ok(connection)
}

/// Validated port settings.
struct Settings {
    path: String,
    baud: u32,
    data_bits: serialport::DataBits,
    parity: serialport::Parity,
    stop_bits: serialport::StopBits,
    flow: serialport::FlowControl,
}

impl TryFrom<&SerialParams> for Settings {
    type Error = TransportError;

    fn try_from(params: &SerialParams) -> Result<Self, Self::Error> {
        let invalid = |what: String| TransportError::Unsupported(what);
        if params.port.trim().is_empty() {
            return Err(invalid("no serial port given".to_owned()));
        }
        if params.baud == 0 {
            return Err(invalid("the baud rate must not be 0".to_owned()));
        }
        Ok(Self {
            path: params.port.clone(),
            baud: params.baud,
            data_bits: serialport::DataBits::try_from(params.data_bits).map_err(|_| {
                invalid(format!(
                    "{} data bits (5 to 8 are possible)",
                    params.data_bits
                ))
            })?,
            parity: match params.parity {
                Parity::None => serialport::Parity::None,
                Parity::Odd => serialport::Parity::Odd,
                Parity::Even => serialport::Parity::Even,
            },
            stop_bits: serialport::StopBits::try_from(params.stop_bits).map_err(|_| {
                invalid(format!(
                    "{} stop bits (1 or 2 are possible)",
                    params.stop_bits
                ))
            })?,
            flow: match params.flow {
                FlowControl::None => serialport::FlowControl::None,
                FlowControl::Software => serialport::FlowControl::Software,
                FlowControl::Hardware => serialport::FlowControl::Hardware,
            },
        })
    }
}

/// An open port: one handle per direction.
struct Port {
    reader: Box<dyn SerialPort>,
    writer: Box<dyn SerialPort>,
}

impl Port {
    fn open(settings: &Settings) -> Result<Self, TransportError> {
        let writer = serialport::new(settings.path.as_str(), settings.baud)
            .data_bits(settings.data_bits)
            .parity(settings.parity)
            .stop_bits(settings.stop_bits)
            .flow_control(settings.flow)
            .timeout(POLL_INTERVAL)
            .open()
            .map_err(port_error)?;
        let reader = writer.try_clone().map_err(port_error)?;
        Ok(Self { reader, writer })
    }
}

fn port_error(error: serialport::Error) -> TransportError {
    let kind = match error.kind {
        serialport::ErrorKind::NoDevice => io::ErrorKind::NotFound,
        serialport::ErrorKind::InvalidInput => io::ErrorKind::InvalidInput,
        serialport::ErrorKind::Io(kind) => kind,
        serialport::ErrorKind::Unknown => io::ErrorKind::Other,
    };
    TransportError::Io(io::Error::new(kind, error.description))
}

/// What the writing thread is asked to do.
enum Command {
    Write(Bytes),
    Break,
}

async fn run(port: Port, backend: Backend) {
    let Backend { mut input, output } = backend;
    let mut relay = Relay::for_output(output.clone(), OUTPUT_HIGH_WATER);
    let stop = Arc::new(AtomicBool::new(false));
    // Data read from the port, or the error that ended a port thread.
    let (received_tx, mut received) =
        mpsc::channel::<io::Result<Bytes>>(Connection::DEFAULT_BUFFER);
    let (commands, commands_rx) = mpsc::channel::<Command>(Connection::DEFAULT_BUFFER);

    let threads = {
        let reader = spawn_thread("nocturne-serial-rx", {
            let (stop, events) = (Arc::clone(&stop), received_tx.clone());
            move || read_port(port.reader, &events, &stop)
        });
        let writer = spawn_thread("nocturne-serial-tx", {
            let stop = Arc::clone(&stop);
            move || write_port(port.writer, commands_rx, &received_tx, &stop)
        });
        [reader, writer]
    };

    let outcome = match threads {
        [Ok(_), Ok(_)] => {
            drive(
                read_events(&mut received, &mut relay, &output),
                write_commands(&mut input, &commands),
                Duration::ZERO,
            )
            .await
        }
        _ => Outcome::Closed(Some("cannot start the serial port threads".to_owned())),
    };

    // Let go of the port before announcing the end, so that it can be
    // reopened as soon as `Closed` is seen.
    stop.store(true, Ordering::Release);
    drop((commands, received, input, output));
    let released = tokio::task::spawn_blocking(move || {
        for thread in threads.into_iter().flatten() {
            let _ = thread.join();
        }
    });
    let _ = tokio::time::timeout(RELEASE_TIMEOUT, released).await;

    if let Outcome::Closed(reason) = outcome {
        relay.finish(reason).await;
    }
}

fn spawn_thread(
    name: &str,
    work: impl FnOnce() + Send + 'static,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(work)
}

/// Reading thread: port → pump.
fn read_port(
    mut port: Box<dyn SerialPort>,
    events: &mpsc::Sender<io::Result<Bytes>>,
    stop: &AtomicBool,
) {
    let mut buffer = vec![0u8; READ_BUFFER];
    while !stop.load(Ordering::Acquire) {
        let event = match port.read(&mut buffer) {
            Ok(0) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the device was disconnected",
            )),
            Ok(count) => Ok(Bytes::copy_from_slice(&buffer[..count])),
            Err(error) if is_retryable(&error) => continue,
            Err(error) => Err(error),
        };
        let fatal = event.is_err();
        // Fails once the pump is gone; waits while the application lags.
        if events.blocking_send(event).is_err() || fatal {
            return;
        }
    }
}

/// Writing thread: pump → port.
fn write_port(
    mut port: Box<dyn SerialPort>,
    mut commands: mpsc::Receiver<Command>,
    events: &mpsc::Sender<io::Result<Bytes>>,
    stop: &AtomicBool,
) {
    while let Some(command) = commands.blocking_recv() {
        let done = match command {
            Command::Write(data) => write_all(port.as_mut(), &data, stop),
            Command::Break => {
                // Not every adapter can do it; that is no reason to hang up.
                if let Err(error) = send_break(port.as_ref()) {
                    tracing::warn!(%error, "cannot send a break on the serial line");
                }
                Ok(())
            }
        };
        if let Err(error) = done {
            let _ = events.blocking_send(Err(error));
            return;
        }
    }
}

/// Like `Write::write_all`, but a port that is not ready (flow control) is
/// retried until the connection is being closed.
fn write_all(port: &mut dyn SerialPort, mut data: &[u8], stop: &AtomicBool) -> io::Result<()> {
    while !data.is_empty() {
        match port.write(data) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => data = &data[written..],
            Err(error) if is_retryable(&error) => {
                if stop.load(Ordering::Acquire) {
                    return Ok(());
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn send_break(port: &dyn SerialPort) -> io::Result<()> {
    port.set_break()?;
    thread::sleep(BREAK_DURATION);
    port.clear_break()?;
    Ok(())
}

fn is_retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// Port → application.
async fn read_events(
    received: &mut mpsc::Receiver<io::Result<Bytes>>,
    relay: &mut Relay<Output>,
    output: &mpsc::Sender<Output>,
) -> ReadEnd {
    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return ReadEnd::Abandoned;
                }
            }
            () = output.closed() => return ReadEnd::Abandoned,
            event = received.recv(), if relay.has_room() => match event {
                Some(Ok(data)) => relay.push_data(data),
                Some(Err(error)) => {
                    return ReadEnd::Finished(Some(format!("serial port error: {error}")));
                }
                None => return ReadEnd::Finished(Some("the serial port was closed".to_owned())),
            }
        }
    }
}

/// Application → port.
async fn write_commands(
    input: &mut mpsc::Receiver<Input>,
    commands: &mpsc::Sender<Command>,
) -> WriteEnd {
    while let Some(item) = input.recv().await {
        let command = match item {
            Input::Data(data) => Command::Write(data),
            Input::Break => Command::Break,
            Input::Resize { .. } => continue,
            Input::Close => return WriteEnd::CloseRequested,
        };
        if commands.send(command).await.is_err() {
            // The writing thread reports why it stopped through the reader.
            return WriteEnd::InputClosed;
        }
    }
    WriteEnd::InputClosed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_default_to_9600_8n1() {
        let params: SerialParams = serde_json::from_str(r#"{"port":"COM3"}"#).unwrap();
        assert_eq!(
            params,
            SerialParams {
                port: "COM3".into(),
                ..SerialParams::default()
            }
        );
        assert_eq!(
            (params.baud, params.data_bits, params.stop_bits),
            (9600, 8, 1)
        );
        assert_eq!(params.parity, Parity::None);
        assert_eq!(params.flow, FlowControl::None);

        let params: SerialParams =
            serde_json::from_str(r#"{"port":"x","parity":"even","flow":"hardware","baud":115200}"#)
                .unwrap();
        assert_eq!(params.parity, Parity::Even);
        assert_eq!(params.flow, FlowControl::Hardware);
    }

    #[test]
    fn invalid_settings_are_rejected_before_opening() {
        let reject = |params: SerialParams| Settings::try_from(&params).err().map(|e| e.code());
        let base = SerialParams {
            port: "COM1".into(),
            ..SerialParams::default()
        };
        assert_eq!(reject(base.clone()), None);
        assert_eq!(reject(SerialParams::default()), Some("unsupported"));
        assert_eq!(
            reject(SerialParams {
                data_bits: 9,
                ..base.clone()
            }),
            Some("unsupported")
        );
        assert_eq!(
            reject(SerialParams {
                stop_bits: 3,
                ..base.clone()
            }),
            Some("unsupported")
        );
        assert_eq!(
            reject(SerialParams { baud: 0, ..base }),
            Some("unsupported")
        );
    }

    #[test]
    fn ports_sort_naturally() {
        let mut names = vec![
            "COM10",
            "COM2",
            "/dev/ttyUSB11",
            "/dev/ttyUSB9",
            "/dev/ttyS0",
        ];
        names.sort_by_key(|name| natural_key(name));
        assert_eq!(
            names,
            [
                "/dev/ttyS0",
                "/dev/ttyUSB9",
                "/dev/ttyUSB11",
                "COM2",
                "COM10"
            ]
        );
    }

    #[test]
    fn listing_ports_never_panics() {
        for port in list_serial_ports() {
            assert!(!port.name.is_empty());
            assert!(!port.description.is_empty());
        }
    }

    #[tokio::test]
    async fn a_missing_port_is_an_io_error() {
        let params = SerialParams {
            port: "/dev/nocturne-no-such-port".into(),
            ..SerialParams::default()
        };
        let error = connect_serial(&params).await.unwrap_err();
        assert_eq!(error.code(), "io");
    }
}
