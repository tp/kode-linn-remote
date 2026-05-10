use core::task::Poll;

use app_runtime::net::{ByteStream, Endpoint, TcpConnector};
use embassy_futures::{
    block_on, poll_once,
    select::{Either, Either3, select, select3},
};
use embassy_net::{
    Config as NetConfig, IpAddress, IpEndpoint, Ipv4Address, Runner, Stack, StackResources,
    dns::DnsQueryType, tcp::TcpSocket,
};
use embassy_time::{Duration, Instant, Timer};
use esp_radio::wifi::Interface;

const TCP_RX_BUFFER_BYTES: usize = 2048;
const TCP_TX_BUFFER_BYTES: usize = 2048;
const ARTWORK_RX_BUFFER_BYTES: usize = 4096;
const ARTWORK_TX_BUFFER_BYTES: usize = 1024;
const DHCP_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_CONNECT_TIMEOUT: Duration = Duration::from_millis(20);
const DNS_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_READ_TIMEOUT: Duration = Duration::from_millis(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const EVENT_WRITE_TIMEOUT: Duration = Duration::from_millis(20);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const ABORT_TIMEOUT: Duration = Duration::from_millis(20);

pub struct FirmwareNetwork {
    stack: Stack<'static>,
    runner: Runner<'static, Interface<'static>>,
    lpec_socket: Option<TcpSocket<'static>>,
    artwork_socket: Option<TcpSocket<'static>>,
    config_ready: bool,
    config_poll_started_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirmwareNetError {
    DhcpTimeout,
    ConnectFailed,
    ConnectTimeout,
    DnsFailed,
    DnsTimeout,
    ReadFailed,
    ReadTimeout,
    WriteFailed,
    WriteTimeout,
    FlushFailed,
    FlushTimeout,
}

impl FirmwareNetwork {
    pub fn new(station: Interface<'static>) -> Self {
        static mut NET_RESOURCES: StackResources<4> = StackResources::new();
        let resources = unsafe { &mut *core::ptr::addr_of_mut!(NET_RESOURCES) };
        let (stack, runner) = embassy_net::new(
            station,
            NetConfig::dhcpv4(Default::default()),
            resources,
            0x6c_69_6e_6e,
        );

        Self {
            stack,
            runner,
            lpec_socket: None,
            artwork_socket: None,
            config_ready: false,
            config_poll_started_at: None,
        }
    }

    pub fn wait_config_up(&mut self) -> Result<(), FirmwareNetError> {
        block_on(self.wait_config_up_async())
    }

    pub async fn wait_config_up_async(&mut self) -> Result<(), FirmwareNetError> {
        if self.config_ready {
            return Ok(());
        }

        match select3(
            self.stack.wait_config_up(),
            self.runner.run(),
            Timer::after(DHCP_TIMEOUT),
        )
        .await
        {
            Either3::First(()) => {
                self.config_ready = true;
                self.config_poll_started_at = None;
                Ok(())
            }
            Either3::Second(_) => unreachable!(),
            Either3::Third(()) => Err(FirmwareNetError::DhcpTimeout),
        }
    }

    pub fn poll_config_up(&mut self) -> Result<bool, FirmwareNetError> {
        if self.config_ready {
            return Ok(true);
        }

        let started_at = *self.config_poll_started_at.get_or_insert_with(Instant::now);
        if started_at.elapsed() >= DHCP_TIMEOUT {
            self.config_poll_started_at = None;
            return Err(FirmwareNetError::DhcpTimeout);
        }

        match poll_once(select(self.stack.wait_config_up(), self.runner.run())) {
            Poll::Ready(Either::First(())) => {
                self.config_ready = true;
                self.config_poll_started_at = None;
                Ok(true)
            }
            Poll::Ready(Either::Second(_)) => unreachable!(),
            Poll::Pending => Ok(false),
        }
    }

    pub fn config_v4(&self) -> Option<embassy_net::StaticConfigV4> {
        self.stack.config_v4()
    }

    pub fn connect(
        &mut self,
        endpoint: Endpoint,
    ) -> Result<FirmwareTcpStream<'_>, FirmwareNetError> {
        self.connect_with_timeouts(endpoint, CONNECT_TIMEOUT, READ_TIMEOUT, WRITE_TIMEOUT)
    }

    pub fn connect_events(
        &mut self,
        endpoint: Endpoint,
    ) -> Result<FirmwareTcpStream<'_>, FirmwareNetError> {
        self.connect_with_timeouts(
            endpoint,
            EVENT_CONNECT_TIMEOUT,
            EVENT_READ_TIMEOUT,
            EVENT_WRITE_TIMEOUT,
        )
    }

    fn connect_with_timeouts(
        &mut self,
        endpoint: Endpoint,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Result<FirmwareTcpStream<'_>, FirmwareNetError> {
        self.wait_config_up()?;
        self.ensure_lpec_socket(endpoint, connect_timeout)?;

        let Self {
            runner,
            lpec_socket,
            ..
        } = self;
        let socket = lpec_socket
            .as_mut()
            .ok_or(FirmwareNetError::ConnectFailed)?;
        Ok(FirmwareTcpStream {
            socket,
            runner,
            read_timeout,
            write_timeout,
        })
    }

    pub fn reset_lpec(&mut self) {
        let Some(mut socket) = self.lpec_socket.take() else {
            return;
        };

        socket.abort();
        let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
    }

    pub fn reset_artwork(&mut self) {
        let Some(mut socket) = self.artwork_socket.take() else {
            return;
        };

        socket.abort();
        let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
    }

    fn ensure_lpec_socket(
        &mut self,
        endpoint: Endpoint,
        connect_timeout: Duration,
    ) -> Result<(), FirmwareNetError> {
        if self
            .lpec_socket
            .as_ref()
            .is_some_and(|socket| socket.may_send() || socket.may_recv())
        {
            return Ok(());
        }

        self.reset_lpec();
        static mut TCP_RX_BUFFER: [u8; TCP_RX_BUFFER_BYTES] = [0; TCP_RX_BUFFER_BYTES];
        static mut TCP_TX_BUFFER: [u8; TCP_TX_BUFFER_BYTES] = [0; TCP_TX_BUFFER_BYTES];

        let rx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX_BUFFER) };
        let tx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(TCP_TX_BUFFER) };
        let mut socket = TcpSocket::new(self.stack, rx_buffer, tx_buffer);
        let remote = IpEndpoint::new(
            IpAddress::Ipv4(Ipv4Address::new(
                endpoint.address[0],
                endpoint.address[1],
                endpoint.address[2],
                endpoint.address[3],
            )),
            endpoint.port,
        );

        match block_on(select3(
            socket.connect(remote),
            self.runner.run(),
            Timer::after(connect_timeout),
        )) {
            Either3::First(Ok(())) => {
                self.lpec_socket = Some(socket);
                Ok(())
            }
            Either3::First(Err(_)) => {
                socket.abort();
                let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
                Err(FirmwareNetError::ConnectFailed)
            }
            Either3::Second(_) => unreachable!(),
            Either3::Third(()) => {
                socket.abort();
                let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
                Err(FirmwareNetError::ConnectTimeout)
            }
        }
    }

    fn resolve_ipv4(&mut self, host: &str) -> Result<Ipv4Address, FirmwareNetError> {
        self.wait_config_up()?;
        match block_on(select3(
            self.stack.dns_query(host, DnsQueryType::A),
            self.runner.run(),
            Timer::after(DNS_TIMEOUT),
        )) {
            Either3::First(Ok(addresses)) => addresses
                .iter()
                .find_map(|address| match address {
                    IpAddress::Ipv4(address) => Some(*address),
                })
                .ok_or(FirmwareNetError::DnsFailed),
            Either3::First(Err(_)) => Err(FirmwareNetError::DnsFailed),
            Either3::Second(_) => unreachable!(),
            Either3::Third(()) => Err(FirmwareNetError::DnsTimeout),
        }
    }

    fn connect_artwork(
        &mut self,
        remote: IpEndpoint,
    ) -> Result<FirmwareTcpStream<'_>, FirmwareNetError> {
        self.wait_config_up()?;
        self.reset_artwork();

        static mut ARTWORK_RX_BUFFER: [u8; ARTWORK_RX_BUFFER_BYTES] = [0; ARTWORK_RX_BUFFER_BYTES];
        static mut ARTWORK_TX_BUFFER: [u8; ARTWORK_TX_BUFFER_BYTES] = [0; ARTWORK_TX_BUFFER_BYTES];

        let rx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(ARTWORK_RX_BUFFER) };
        let tx_buffer = unsafe { &mut *core::ptr::addr_of_mut!(ARTWORK_TX_BUFFER) };
        let mut socket = TcpSocket::new(self.stack, rx_buffer, tx_buffer);

        match block_on(select3(
            socket.connect(remote),
            self.runner.run(),
            Timer::after(CONNECT_TIMEOUT),
        )) {
            Either3::First(Ok(())) => {
                self.artwork_socket = Some(socket);
                let Self {
                    runner,
                    artwork_socket,
                    ..
                } = self;
                let socket = artwork_socket
                    .as_mut()
                    .ok_or(FirmwareNetError::ConnectFailed)?;
                Ok(FirmwareTcpStream {
                    socket,
                    runner,
                    read_timeout: READ_TIMEOUT,
                    write_timeout: WRITE_TIMEOUT,
                })
            }
            Either3::First(Err(_)) => {
                socket.abort();
                let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
                Err(FirmwareNetError::ConnectFailed)
            }
            Either3::Second(_) => unreachable!(),
            Either3::Third(()) => {
                socket.abort();
                let _ = drive_tcp(socket.flush(), &mut self.runner, ABORT_TIMEOUT);
                Err(FirmwareNetError::ConnectTimeout)
            }
        }
    }
}

impl TcpConnector for FirmwareNetwork {
    type Stream<'a> = FirmwareTcpStream<'a>;
    type Error = FirmwareNetError;

    fn connect(&mut self, endpoint: Endpoint) -> Result<Self::Stream<'_>, Self::Error> {
        FirmwareNetwork::connect(self, endpoint)
    }

    fn connect_host(&mut self, host: &str, port: u16) -> Result<Self::Stream<'_>, Self::Error> {
        let address = self.resolve_ipv4(host)?;
        let remote = IpEndpoint::new(IpAddress::Ipv4(address), port);
        self.connect_artwork(remote)
    }
}

pub struct FirmwareTcpStream<'a> {
    socket: &'a mut TcpSocket<'static>,
    runner: &'a mut Runner<'static, Interface<'static>>,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl ByteStream for FirmwareTcpStream<'_> {
    type Error = FirmwareNetError;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        match drive_tcp(self.socket.read(buffer), self.runner, self.read_timeout) {
            Ok(Ok(count)) => Ok(count),
            Ok(Err(_)) => Err(FirmwareNetError::ReadFailed),
            Err(Timeout) => Err(FirmwareNetError::ReadTimeout),
        }
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> Result<(), Self::Error> {
        while !bytes.is_empty() {
            match drive_tcp(self.socket.write(bytes), self.runner, self.write_timeout) {
                Ok(Ok(0)) => return Err(FirmwareNetError::WriteFailed),
                Ok(Ok(count)) => bytes = &bytes[count..],
                Ok(Err(_)) => return Err(FirmwareNetError::WriteFailed),
                Err(Timeout) => return Err(FirmwareNetError::WriteTimeout),
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        match drive_tcp(self.socket.flush(), self.runner, FLUSH_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(FirmwareNetError::FlushFailed),
            Err(Timeout) => Err(FirmwareNetError::FlushTimeout),
        }
    }
}

struct Timeout;

fn drive_tcp<F, T>(
    operation: F,
    runner: &mut Runner<'static, Interface<'static>>,
    timeout: Duration,
) -> Result<T, Timeout>
where
    F: core::future::Future<Output = T>,
{
    match block_on(select3(operation, runner.run(), Timer::after(timeout))) {
        Either3::First(result) => Ok(result),
        Either3::Second(_) => unreachable!(),
        Either3::Third(()) => Err(Timeout),
    }
}
