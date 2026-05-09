#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub address: [u8; 4],
    pub port: u16,
}

impl Endpoint {
    pub const fn ipv4(address: [u8; 4], port: u16) -> Self {
        Self { address, port }
    }
}

pub trait ByteStream {
    type Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}

pub trait TcpConnector {
    type Stream: ByteStream<Error = Self::Error>;
    type Error;

    fn connect(&mut self, endpoint: Endpoint) -> Result<Self::Stream, Self::Error>;
    fn connect_host(&mut self, host: &str, port: u16) -> Result<Self::Stream, Self::Error>;
}
