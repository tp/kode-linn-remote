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
}

impl HostTcpConnector {
    pub const fn new() -> Self {
        Self {
            connect_timeout: Duration::from_millis(700),
            read_timeout: Duration::from_millis(250),
            write_timeout: Duration::from_millis(500),
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
}

impl Default for HostTcpConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpConnector for HostTcpConnector {
    type Stream = HostTcpStream;
    type Error = io::Error;

    fn connect(&mut self, endpoint: Endpoint) -> Result<Self::Stream, Self::Error> {
        HostTcpStream::connect(
            SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(
                    endpoint.address[0],
                    endpoint.address[1],
                    endpoint.address[2],
                    endpoint.address[3],
                ),
                endpoint.port,
            )),
            self.connect_timeout,
            self.read_timeout,
            self.write_timeout,
        )
    }

    fn connect_host(&mut self, host: &str, port: u16) -> Result<Self::Stream, Self::Error> {
        let mut last_error = None;
        for addr in (host, port).to_socket_addrs()? {
            match HostTcpStream::connect(
                addr,
                self.connect_timeout,
                self.read_timeout,
                self.write_timeout,
            ) {
                Ok(stream) => return Ok(stream),
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
pub struct HostTcpStream {
    stream: TcpStream,
}

impl HostTcpStream {
    fn connect(
        addr: SocketAddr,
        connect_timeout: Duration,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> io::Result<Self> {
        let stream = TcpStream::connect_timeout(&addr, connect_timeout)?;
        stream.set_read_timeout(Some(read_timeout))?;
        stream.set_write_timeout(Some(write_timeout))?;

        Ok(Self { stream })
    }
}

impl ByteStream for HostTcpStream {
    type Error = io::Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        self.stream.read(buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.stream.write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.stream.flush()
    }
}
