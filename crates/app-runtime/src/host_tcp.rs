use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs},
    time::Duration,
};

use crate::net::{ByteStream, Endpoint, TcpConnector};

#[derive(Debug)]
pub struct HostTcpConnector {
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
    event_connect_timeout: Duration,
    event_read_timeout: Duration,
    event_write_timeout: Duration,
    lpec_endpoint: Option<Endpoint>,
    lpec_stream: Option<TcpStream>,
}

impl HostTcpConnector {
    pub const fn new() -> Self {
        Self {
            connect_timeout: Duration::from_millis(2000),
            read_timeout: Duration::from_millis(2500),
            write_timeout: Duration::from_millis(2500),
            event_connect_timeout: Duration::from_millis(20),
            event_read_timeout: Duration::from_millis(5),
            event_write_timeout: Duration::from_millis(20),
            lpec_endpoint: None,
            lpec_stream: None,
        }
    }

    pub const fn with_timeouts(
        mut self,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Self {
        self.connect_timeout = connect_timeout;
        self.read_timeout = read_timeout;
        self.write_timeout = write_timeout;
        self
    }

    pub const fn with_event_timeouts(
        mut self,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> Self {
        self.event_connect_timeout = connect_timeout;
        self.event_read_timeout = read_timeout;
        self.event_write_timeout = write_timeout;
        self
    }

    fn connect_lpec(
        &mut self,
        endpoint: Endpoint,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> io::Result<HostTcpStream<'_>> {
        if self.lpec_endpoint != Some(endpoint) {
            self.lpec_stream = None;
            self.lpec_endpoint = Some(endpoint);
        }

        if self.lpec_stream.is_none() {
            self.lpec_stream = Some(connect_tcp_stream(
                endpoint_addr(endpoint),
                connect_timeout,
                read_timeout,
                write_timeout,
            )?);
        }

        let stream = self
            .lpec_stream
            .as_mut()
            .expect("lpec stream was just connected");
        stream.set_read_timeout(Some(read_timeout))?;
        stream.set_write_timeout(Some(write_timeout))?;
        Ok(HostTcpStream::Borrowed(stream))
    }
}

impl Default for HostTcpConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpConnector for HostTcpConnector {
    type Stream<'a> = HostTcpStream<'a>;
    type Error = io::Error;

    fn connect(&mut self, endpoint: Endpoint) -> Result<Self::Stream<'_>, Self::Error> {
        self.connect_lpec(
            endpoint,
            self.connect_timeout,
            self.read_timeout,
            self.write_timeout,
        )
    }

    fn connect_events(&mut self, endpoint: Endpoint) -> Result<Self::Stream<'_>, Self::Error> {
        self.connect_lpec(
            endpoint,
            self.event_connect_timeout,
            self.event_read_timeout,
            self.event_write_timeout,
        )
    }

    fn reset(&mut self, endpoint: Endpoint) {
        if self.lpec_endpoint == Some(endpoint) {
            self.lpec_stream = None;
        }
    }

    fn connect_host(&mut self, host: &str, port: u16) -> Result<Self::Stream<'_>, Self::Error> {
        let mut last_error = None;
        for addr in (host, port).to_socket_addrs()? {
            match connect_tcp_stream(
                addr,
                self.connect_timeout,
                self.read_timeout,
                self.write_timeout,
            ) {
                Ok(stream) => return Ok(HostTcpStream::Owned(stream)),
                Err(error) => last_error = Some(error),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "host did not resolve to any address",
            )
        }))
    }
}

#[derive(Debug)]
pub enum HostTcpStream<'a> {
    Owned(TcpStream),
    Borrowed(&'a mut TcpStream),
}

impl HostTcpStream<'_> {
    fn stream(&mut self) -> &mut TcpStream {
        match self {
            Self::Owned(stream) => stream,
            Self::Borrowed(stream) => stream,
        }
    }
}

impl ByteStream for HostTcpStream<'_> {
    type Error = io::Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        self.stream().read(buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.stream().write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.stream().flush()
    }

    fn is_read_timeout(error: &Self::Error) -> bool {
        matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        )
    }
}

fn endpoint_addr(endpoint: Endpoint) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(
            endpoint.address[0],
            endpoint.address[1],
            endpoint.address[2],
            endpoint.address[3],
        ),
        endpoint.port,
    ))
}

fn connect_tcp_stream(
    addr: SocketAddr,
    connect_timeout: Duration,
    read_timeout: Duration,
    write_timeout: Duration,
) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, connect_timeout)?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(write_timeout))?;
    Ok(stream)
}
