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

    /// Whether the given read error means "no data available right now" rather
    /// than a fatal failure. Used by drain loops to stop reading without
    /// resetting the connection. Defaults to `false`.
    fn is_read_timeout(_error: &Self::Error) -> bool {
        false
    }
}

impl<T> ByteStream for &mut T
where
    T: ByteStream + ?Sized,
{
    type Error = T::Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        (**self).read(buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        (**self).write_all(bytes)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        (**self).flush()
    }

    fn is_read_timeout(error: &Self::Error) -> bool {
        T::is_read_timeout(error)
    }
}

pub trait TcpConnector {
    type Stream<'a>: ByteStream<Error = Self::Error>
    where
        Self: 'a;
    type Error;

    fn connect(&mut self, endpoint: Endpoint) -> Result<Self::Stream<'_>, Self::Error>;
    fn connect_events(&mut self, endpoint: Endpoint) -> Result<Self::Stream<'_>, Self::Error> {
        self.connect(endpoint)
    }
    fn reset(&mut self, _endpoint: Endpoint) {}
    fn connect_host(&mut self, host: &str, port: u16) -> Result<Self::Stream<'_>, Self::Error>;
}
