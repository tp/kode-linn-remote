use alloc::vec::Vec as AllocVec;

use app_core::{
    ArtworkPixel, HIFI_ARTWORK_PIXELS, HIFI_ARTWORK_SIZE, HifiArtwork, HifiCommand, HifiStatus,
    PlaybackState,
};
use heapless::Vec;
use linn_lpec::{Client as LinnClient, Line, Transport};
use zune_core::bytestream::ZCursor;
use zune_jpeg::JpegDecoder;

use crate::{
    HifiController,
    net::{ByteStream, Endpoint, TcpConnector},
};

const MAX_ARTWORK_BYTES: usize = 128 * 1024;
const MAX_HTTP_HEADER_BYTES: usize = 8 * 1024;
const SESSION_ACTION_LINE_BUDGET: usize = 16;
pub const ARTWORK_HTTP_BUFFER_BYTES: usize = 32 * 1024;
pub const ARTWORK_DECODE_BUFFER_BYTES: usize = 36 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Connect(E),
    LineTooLong,
    UnexpectedEof,
    InvalidArtworkUri,
    InvalidHttpResponse,
    HttpError,
    ArtworkTooLarge {
        reason: ArtworkTooLargeReason,
        limit: usize,
        actual: usize,
    },
    ArtworkBufferTooSmall {
        buffer: ArtworkBuffer,
        required: usize,
        actual: usize,
    },
    ArtworkDecode,
    StatusUnavailable,
    Protocol(linn_lpec::Error<core::convert::Infallible>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkTooLargeReason {
    ContentLength,
    ResponseBuffer,
    Reserve,
    LengthOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtworkBuffer {
    HttpResponse,
    DecodeOutput,
}

impl Error<core::convert::Infallible> {
    fn erase_transport<E>(self) -> Error<E> {
        match self {
            Self::Connect(never) => match never {},
            Self::LineTooLong => Error::LineTooLong,
            Self::UnexpectedEof => Error::UnexpectedEof,
            Self::InvalidArtworkUri => Error::InvalidArtworkUri,
            Self::InvalidHttpResponse => Error::InvalidHttpResponse,
            Self::HttpError => Error::HttpError,
            Self::ArtworkTooLarge {
                reason,
                limit,
                actual,
            } => Error::ArtworkTooLarge {
                reason,
                limit,
                actual,
            },
            Self::ArtworkBufferTooSmall {
                buffer,
                required,
                actual,
            } => Error::ArtworkBufferTooSmall {
                buffer,
                required,
                actual,
            },
            Self::ArtworkDecode => Error::ArtworkDecode,
            Self::StatusUnavailable => Error::StatusUnavailable,
            Self::Protocol(error) => Error::Protocol(error),
        }
    }
}

#[derive(Debug)]
pub struct LpecHifi<C>
where
    C: TcpConnector,
{
    connector: C,
    endpoint: Endpoint,
}

impl<C> LpecHifi<C>
where
    C: TcpConnector,
{
    pub const fn new(connector: C, endpoint: Endpoint) -> Self {
        Self {
            connector,
            endpoint,
        }
    }

    pub fn into_connector(self) -> C {
        self.connector
    }

    fn client(&mut self) -> Result<LinnClient<LpecTransport<C::Stream<'_>>>, Error<C::Error>> {
        let stream = self
            .connector
            .connect(self.endpoint)
            .map_err(Error::Connect)?;
        let transport = LpecTransport::new(stream).sync()?;
        Ok(LinnClient::new(transport))
    }

    fn invoke_pin(&mut self, pin: u8) -> Result<(), Error<C::Error>> {
        self.client()?.invoke_pin(pin).map_err(map_client_error)
    }

    fn load_artwork(&mut self, uri: &str) -> Result<HifiArtwork, Error<C::Error>> {
        load_artwork(&mut self.connector, uri)
    }

    fn toggle_playback(&mut self) -> Result<(), Error<C::Error>> {
        let mut client = self.client()?;
        let playback = read_playback_state(&mut client)?;

        if playback_can_pause(playback) {
            client.playlist_pause().map_err(map_client_error)
        } else {
            client.playlist_play().map_err(map_client_error)
        }
    }
}

impl<C> HifiController for LpecHifi<C>
where
    C: TcpConnector,
{
    type Error = Error<C::Error>;

    fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error> {
        match command {
            HifiCommand::ActivatePreset { preset } => self.invoke_pin(preset),
            HifiCommand::TogglePlayback => self.toggle_playback(),
        }
    }

    fn status(&mut self) -> Result<HifiStatus, Self::Error> {
        read_status(&mut self.client()?)
    }

    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
        self.load_artwork(uri)
    }
}

pub fn handle_command_with_stream<S>(stream: S, command: HifiCommand) -> Result<(), Error<S::Error>>
where
    S: ByteStream,
{
    let mut client = LinnClient::new(LpecTransport::new(stream).sync()?);
    match command {
        HifiCommand::ActivatePreset { preset } => invoke_pin(&mut client, preset),
        HifiCommand::TogglePlayback => {
            let playback = read_playback_state(&mut client)?;
            if playback_can_pause(playback) {
                action_with_retry(&mut client, linn_lpec::playlist_pause()).map(|_| ())
            } else {
                action_with_retry(&mut client, linn_lpec::playlist_play()).map(|_| ())
            }
        }
    }
}

pub fn status_from_stream<S>(stream: S) -> Result<HifiStatus, Error<S::Error>>
where
    S: ByteStream,
{
    read_status(&mut LinnClient::new(LpecTransport::new(stream).sync()?))
}

pub fn quick_status_from_stream<S>(stream: S) -> Result<HifiStatus, Error<S::Error>>
where
    S: ByteStream,
{
    let mut client = LinnClient::new(LpecTransport::new(stream).sync()?);
    let mut status = HifiStatus::empty();

    if let Ok(args) = action_with_retry(&mut client, linn_lpec::info_metatext())
        && let Some(metadata) = args.first()
    {
        apply_metadata(&mut status, metadata);
    }
    if status.title.is_empty()
        && let Ok(args) = action_with_retry(&mut client, linn_lpec::info_track())
        && let Some(metadata) = args.get(1)
    {
        apply_metadata(&mut status, metadata);
    }
    if !status_has_live_content(&status) {
        return Err(Error::StatusUnavailable);
    }

    Ok(status)
}

pub fn load_artwork<C>(connector: &mut C, uri: &str) -> Result<HifiArtwork, Error<C::Error>>
where
    C: TcpConnector,
{
    let request = ArtworkRequest::parse(uri).map_err(Error::erase_transport)?;
    let mut stream = connector
        .connect_host(request.host, request.port)
        .map_err(Error::Connect)?;
    write_http_get(&mut stream, request.host, request.path).map_err(Error::Connect)?;
    let response = read_http_response(&mut stream)?;
    let body = response_body(&response).map_err(Error::erase_transport)?;
    decode_jpeg_artwork(uri, body).map_err(Error::erase_transport)
}

pub fn load_artwork_with_buffers<C>(
    connector: &mut C,
    uri: &str,
    http_buffer: &mut [u8],
    decode_buffer: &mut [u8],
) -> Result<HifiArtwork, Error<C::Error>>
where
    C: TcpConnector,
{
    let request = ArtworkRequest::parse(uri).map_err(Error::erase_transport)?;
    let mut stream = connector
        .connect_host(request.host, request.port)
        .map_err(Error::Connect)?;
    write_http_get(&mut stream, request.host, request.path).map_err(Error::Connect)?;
    let response_len = read_http_response_into(&mut stream, http_buffer)?;
    let body = response_body(&http_buffer[..response_len]).map_err(Error::erase_transport)?;
    decode_jpeg_artwork_into(uri, body, decode_buffer).map_err(Error::erase_transport)
}

pub fn load_artwork_with_buffers_into<C>(
    connector: &mut C,
    uri: &str,
    http_buffer: &mut [u8],
    decode_buffer: &mut [u8],
    artwork_pixels: &'static mut [ArtworkPixel; HIFI_ARTWORK_PIXELS],
) -> Result<HifiArtwork, Error<C::Error>>
where
    C: TcpConnector,
{
    let request = ArtworkRequest::parse(uri).map_err(Error::erase_transport)?;
    let mut stream = connector
        .connect_host(request.host, request.port)
        .map_err(Error::Connect)?;
    write_http_get(&mut stream, request.host, request.path).map_err(Error::Connect)?;
    let response_len = read_http_response_into(&mut stream, http_buffer)?;
    let body = response_body(&http_buffer[..response_len]).map_err(Error::erase_transport)?;
    decode_jpeg_artwork_into_pixels(body, decode_buffer, artwork_pixels)
        .map_err(Error::erase_transport)?;
    HifiArtwork::from_static_pixels(uri, artwork_pixels).ok_or(Error::InvalidArtworkUri)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LpecSession {
    subscribed: bool,
    status: HifiStatus,
}

impl LpecSession {
    pub fn new() -> Self {
        Self {
            subscribed: false,
            status: HifiStatus::empty(),
        }
    }

    pub fn reset(&mut self) {
        self.subscribed = false;
    }

    pub fn poll<S>(&mut self, stream: &mut S) -> Result<Option<HifiStatus>, Error<S::Error>>
    where
        S: ByteStream,
    {
        self.ensure_subscribed(stream)?;

        let line = read_lpec_line(stream)?;
        let message = linn_lpec::parse_message(line.as_str()).map_err(Error::Protocol)?;
        let changed = self.handle_message(message);
        Ok(self.changed_status(changed))
    }

    pub fn handle_command<S>(
        &mut self,
        stream: &mut S,
        command: HifiCommand,
    ) -> Result<Option<HifiStatus>, Error<S::Error>>
    where
        S: ByteStream,
    {
        self.ensure_subscribed(stream)?;
        let mut changed = false;

        match command {
            HifiCommand::ActivatePreset { preset } => {
                let mut pin_arg = heapless::String::<3>::new();
                core::fmt::write(&mut pin_arg, format_args!("{preset}"))
                    .map_err(|_| Error::LineTooLong)?;
                changed |= self
                    .action(stream, linn_lpec::invoke_pin_arg(&pin_arg))?
                    .changed;
            }
            HifiCommand::TogglePlayback => {
                let playback = if self.status.playback == PlaybackState::Unknown {
                    let response = self.action(stream, linn_lpec::playlist_transport_state())?;
                    changed |= response.changed;
                    let playback = response
                        .args
                        .first()
                        .map(|value| parse_playback_state(value))
                        .unwrap_or(PlaybackState::Unknown);
                    if self.status.playback != playback {
                        self.status.playback = playback;
                        changed = true;
                    }
                    playback
                } else {
                    self.status.playback
                };

                let (action, next_playback) = if playback_can_pause(playback) {
                    (linn_lpec::playlist_pause(), PlaybackState::Paused)
                } else {
                    (linn_lpec::playlist_play(), PlaybackState::Playing)
                };
                changed |= self.action(stream, action)?.changed;
                if self.status.playback != next_playback {
                    self.status.playback = next_playback;
                    changed = true;
                }
            }
        }

        Ok(self.changed_status(changed))
    }

    fn ensure_subscribed<S>(&mut self, stream: &mut S) -> Result<(), Error<S::Error>>
    where
        S: ByteStream,
    {
        if self.subscribed {
            return Ok(());
        }

        write_lpec_line(stream, "")?;
        for service in [
            linn_lpec::Service::Time,
            linn_lpec::Service::Info,
            linn_lpec::Service::Volume,
        ] {
            let line = linn_lpec::format_subscribe(service).map_err(Error::Protocol)?;
            write_lpec_line(stream, line.as_str())?;
        }
        self.subscribed = true;
        Ok(())
    }

    fn action<S>(
        &mut self,
        stream: &mut S,
        action: linn_lpec::Action<'_>,
    ) -> Result<SessionActionResponse, Error<S::Error>>
    where
        S: ByteStream,
    {
        let line = linn_lpec::format_action(action).map_err(Error::Protocol)?;
        write_lpec_line(stream, line.as_str())?;

        let mut changed = false;
        for _ in 0..SESSION_ACTION_LINE_BUDGET {
            let line = read_lpec_line(stream)?;
            match linn_lpec::parse_message(line.as_str()).map_err(Error::Protocol)? {
                linn_lpec::Message::Response { args } => {
                    let args = copy_session_response_args(&args).map_err(Error::erase_transport)?;
                    return Ok(SessionActionResponse { args, changed });
                }
                linn_lpec::Message::Error { code, description } => {
                    let description =
                        copy_remote_description(description).map_err(Error::erase_transport)?;
                    return Err(Error::Protocol(linn_lpec::Error::Remote {
                        code,
                        description,
                    }));
                }
                message => changed |= self.handle_message(message),
            }
        }

        Err(Error::Protocol(linn_lpec::Error::UnexpectedMessage))
    }

    fn handle_message(&mut self, message: linn_lpec::Message<'_>) -> bool {
        match message {
            linn_lpec::Message::Event { variables, .. } => {
                let mut changed = false;
                for variable in variables {
                    changed |=
                        apply_event_variable(&mut self.status, variable.name, variable.value);
                }
                changed
            }
            linn_lpec::Message::ByeBye { .. } | linn_lpec::Message::Unsubscribe { .. } => {
                self.reset();
                false
            }
            linn_lpec::Message::Subscribe { .. }
            | linn_lpec::Message::Alive { .. }
            | linn_lpec::Message::Response { .. }
            | linn_lpec::Message::Error { .. } => false,
        }
    }

    fn changed_status(&self, changed: bool) -> Option<HifiStatus> {
        if changed && status_has_live_content(&self.status) {
            Some(self.status.clone())
        } else {
            None
        }
    }
}

impl Default for LpecSession {
    fn default() -> Self {
        Self::new()
    }
}

struct SessionActionResponse {
    args: heapless::Vec<linn_lpec::ResponseArg, { linn_lpec::MAX_ARGS }>,
    changed: bool,
}

pub fn apply_event_variable(status: &mut HifiStatus, name: &str, value: &str) -> bool {
    match name {
        "TransportState" => {
            status.playback = parse_playback_state(value);
            true
        }
        "Duration" | "TrackDuration" => {
            if let Some(duration) = parse_u32(value) {
                status.duration_seconds = duration;
                status.elapsed_seconds = status.elapsed_seconds.min(duration);
                true
            } else {
                false
            }
        }
        "Seconds" | "TrackSeconds" | "Elapsed" | "ElapsedSeconds" => {
            if let Some(elapsed) = parse_u32(value) {
                let previous = status.elapsed_seconds;
                status.elapsed_seconds = elapsed.min(status.duration_seconds);
                if status.elapsed_seconds != previous
                    && matches!(
                        status.playback,
                        PlaybackState::Unknown | PlaybackState::Paused | PlaybackState::Stopped
                    )
                {
                    status.playback = PlaybackState::Playing;
                }
                true
            } else {
                false
            }
        }
        "Volume" => {
            if let Some(volume) = parse_u32(value) {
                status.volume_percent = volume.min(100) as u8;
                true
            } else {
                false
            }
        }
        "Metatext" | "Track" | "Metadata" => {
            apply_metadata(status, value);
            true
        }
        _ => false,
    }
}

fn read_status<S>(client: &mut LinnClient<LpecTransport<S>>) -> Result<HifiStatus, Error<S::Error>>
where
    S: ByteStream,
{
    let mut status = HifiStatus::empty();

    if let Ok(playback) = read_playback_state(client) {
        status.playback = playback;
    }

    if let Ok(args) = client.action(linn_lpec::time()).map_err(map_client_error) {
        if args.len() >= 3 {
            status.duration_seconds = parse_u32(&args[1]).unwrap_or(0);
            status.elapsed_seconds = parse_u32(&args[2])
                .unwrap_or(0)
                .min(status.duration_seconds);
        }
    }

    if let Ok(volume) = client.volume().map_err(map_client_error) {
        status.volume_percent = volume.min(100);
    } else if let Ok(args) = client
        .action(linn_lpec::get_ds_volume())
        .map_err(map_client_error)
        && let Some(value) = args.first()
        && let Some(volume) = parse_u32(value)
    {
        status.volume_percent = volume.min(100) as u8;
    }

    if let Ok(args) = client
        .action(linn_lpec::info_metatext())
        .map_err(map_client_error)
        && let Some(metadata) = args.first()
    {
        apply_metadata(&mut status, metadata);
    }
    if status.title.is_empty()
        && let Ok(args) = client
            .action(linn_lpec::info_track())
            .map_err(map_client_error)
        && let Some(metadata) = args.get(1)
    {
        apply_metadata(&mut status, metadata);
    }

    if !status_has_live_content(&status) {
        return Err(Error::StatusUnavailable);
    }

    Ok(status)
}

fn action_with_retry<S>(
    client: &mut LinnClient<LpecTransport<S>>,
    action: linn_lpec::Action<'_>,
) -> Result<heapless::Vec<linn_lpec::ResponseArg, { linn_lpec::MAX_ARGS }>, Error<S::Error>>
where
    S: ByteStream,
{
    match client.action(action).map_err(map_client_error) {
        Ok(args) => Ok(args),
        Err(_) => client.action(action).map_err(map_client_error),
    }
}

fn invoke_pin<S>(client: &mut LinnClient<LpecTransport<S>>, pin: u8) -> Result<(), Error<S::Error>>
where
    S: ByteStream,
{
    let mut pin_arg = heapless::String::<3>::new();
    core::fmt::write(&mut pin_arg, format_args!("{pin}")).map_err(|_| Error::LineTooLong)?;
    action_with_retry(client, linn_lpec::invoke_pin_arg(&pin_arg)).map(|_| ())
}

fn write_lpec_line<S>(stream: &mut S, line: &str) -> Result<(), Error<S::Error>>
where
    S: ByteStream,
{
    stream.write_all(line.as_bytes()).map_err(Error::Connect)?;
    stream.write_all(b"\r\n").map_err(Error::Connect)?;
    stream.flush().map_err(Error::Connect)
}

fn read_lpec_line<S>(stream: &mut S) -> Result<Line, Error<S::Error>>
where
    S: ByteStream,
{
    let mut buffer = [0_u8; linn_lpec::MAX_LINE_LEN];
    let mut length = 0;
    let mut byte = [0; 1];

    loop {
        let count = stream.read(&mut byte).map_err(Error::Connect)?;
        if count == 0 {
            return Err(Error::UnexpectedEof);
        }

        match byte[0] {
            b'\n' => {
                let value = core::str::from_utf8(&buffer[..length])
                    .map_err(|_| Error::Protocol(linn_lpec::Error::InvalidUtf8))?;
                let mut line = Line::new();
                line.push_str(value).map_err(|_| Error::LineTooLong)?;
                return Ok(line);
            }
            b'\r' => {}
            value => {
                if length == buffer.len() {
                    return Err(Error::LineTooLong);
                }
                buffer[length] = value;
                length += 1;
            }
        }
    }
}

fn copy_session_response_args(
    args: &Vec<&str, { linn_lpec::MAX_ARGS }>,
) -> Result<Vec<linn_lpec::ResponseArg, { linn_lpec::MAX_ARGS }>, Error<core::convert::Infallible>>
{
    let mut copied = Vec::new();
    for arg in args {
        let mut value = linn_lpec::ResponseArg::new();
        copy_xml_text(arg, &mut value);
        copied
            .push(value)
            .map_err(|_| Error::Protocol(linn_lpec::Error::TooManyArgs))?;
    }
    Ok(copied)
}

fn copy_remote_description(
    description: &str,
) -> Result<linn_lpec::RemoteDescription, Error<core::convert::Infallible>> {
    let mut copied = linn_lpec::RemoteDescription::new();
    copied
        .push_str(description)
        .map_err(|_| Error::LineTooLong)?;
    Ok(copied)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArtworkRequest<'a> {
    host: &'a str,
    port: u16,
    path: &'a str,
}

impl<'a> ArtworkRequest<'a> {
    fn parse(uri: &'a str) -> Result<Self, Error<core::convert::Infallible>> {
        let rest = uri
            .strip_prefix("http://")
            .or_else(|| uri.strip_prefix("https://"))
            .ok_or(Error::InvalidArtworkUri)?;
        let (host_port, path) = rest
            .split_once('/')
            .map(|(host, path)| (host, path))
            .unwrap_or((rest, ""));
        let (host, port) = host_port
            .rsplit_once(':')
            .and_then(|(host, port)| parse_u16(port).map(|port| (host, port)))
            .unwrap_or((host_port, 80));
        Ok(Self { host, port, path })
    }
}

fn map_client_error<E>(error: linn_lpec::Error<Error<E>>) -> Error<E> {
    match error {
        linn_lpec::Error::Fmt => Error::Protocol(linn_lpec::Error::Fmt),
        linn_lpec::Error::LineTooLong => Error::Protocol(linn_lpec::Error::LineTooLong),
        linn_lpec::Error::InvalidUtf8 => Error::Protocol(linn_lpec::Error::InvalidUtf8),
        linn_lpec::Error::InvalidMessage => Error::Protocol(linn_lpec::Error::InvalidMessage),
        linn_lpec::Error::TooManyArgs => Error::Protocol(linn_lpec::Error::TooManyArgs),
        linn_lpec::Error::TooManyEvents => Error::Protocol(linn_lpec::Error::TooManyEvents),
        linn_lpec::Error::InvalidNumber => Error::Protocol(linn_lpec::Error::InvalidNumber),
        linn_lpec::Error::UnexpectedMessage => Error::Protocol(linn_lpec::Error::UnexpectedMessage),
        linn_lpec::Error::Remote { code, description } => {
            Error::Protocol(linn_lpec::Error::Remote { code, description })
        }
        linn_lpec::Error::Transport(error) => error,
    }
}

fn parse_playback_state(value: &str) -> PlaybackState {
    match value {
        "Playing" | "PLAYING" | "playing" => PlaybackState::Playing,
        "Paused" | "PAUSED_PLAYBACK" | "paused" => PlaybackState::Paused,
        "Stopped" | "STOPPED" | "stopped" => PlaybackState::Stopped,
        "Buffering" | "BUFFERING" | "buffering" | "Transitioning" | "TRANSITIONING"
        | "transitioning" => PlaybackState::Buffering,
        _ => PlaybackState::Unknown,
    }
}

fn read_playback_state<S>(
    client: &mut LinnClient<LpecTransport<S>>,
) -> Result<PlaybackState, Error<S::Error>>
where
    S: ByteStream,
{
    let args = client
        .action(linn_lpec::playlist_transport_state())
        .map_err(map_client_error)?;
    Ok(args
        .first()
        .map(|value| parse_playback_state(value))
        .unwrap_or(PlaybackState::Unknown))
}

fn playback_can_pause(playback: PlaybackState) -> bool {
    matches!(playback, PlaybackState::Playing | PlaybackState::Buffering)
}

fn status_has_live_content(status: &HifiStatus) -> bool {
    !status.title.is_empty()
        || !status.artist.is_empty()
        || status.duration_seconds > 0
        || status.elapsed_seconds > 0
        || status.playback != PlaybackState::Unknown
}

fn write_http_get<S>(stream: &mut S, host: &str, path: &str) -> Result<(), S::Error>
where
    S: ByteStream,
{
    stream.write_all(b"GET /")?;
    stream.write_all(path.as_bytes())?;
    stream.write_all(b" HTTP/1.0\r\nHost: ")?;
    stream.write_all(host.as_bytes())?;
    stream.write_all(b"\r\nUser-Agent: esp32-rust\r\nConnection: close\r\n\r\n")?;
    stream.flush()
}

fn read_http_response<S>(stream: &mut S) -> Result<AllocVec<u8>, Error<S::Error>>
where
    S: ByteStream,
{
    let mut response = AllocVec::new();
    let mut chunk = [0_u8; 1024];
    let mut target_len = None;
    loop {
        if target_len.is_some_and(|target| response.len() >= target) {
            return Ok(response);
        }

        let count = stream.read(&mut chunk).map_err(Error::Connect)?;
        if count == 0 {
            return Ok(response);
        }
        response
            .try_reserve(count)
            .map_err(|_| Error::ArtworkTooLarge {
                reason: ArtworkTooLargeReason::Reserve,
                limit: MAX_ARTWORK_BYTES + MAX_HTTP_HEADER_BYTES,
                actual: response.len().saturating_add(count),
            })?;
        let next_len = response.len().saturating_add(count);
        if next_len > MAX_ARTWORK_BYTES + MAX_HTTP_HEADER_BYTES {
            return Err(Error::ArtworkTooLarge {
                reason: ArtworkTooLargeReason::ResponseBuffer,
                limit: MAX_ARTWORK_BYTES + MAX_HTTP_HEADER_BYTES,
                actual: next_len,
            });
        }
        response.extend_from_slice(&chunk[..count]);

        let Some(header_end) = http_header_end(&response) else {
            if response.len() > MAX_HTTP_HEADER_BYTES {
                return Err(Error::InvalidHttpResponse);
            }
            continue;
        };
        if target_len.is_none() {
            target_len = http_content_length(&response[..header_end])
                .map_err(Error::erase_transport)?
                .map(|body_len| {
                    if body_len > MAX_ARTWORK_BYTES {
                        Err(Error::ArtworkTooLarge {
                            reason: ArtworkTooLargeReason::ContentLength,
                            limit: MAX_ARTWORK_BYTES,
                            actual: body_len,
                        })
                    } else {
                        header_end
                            .checked_add(body_len)
                            .ok_or(Error::ArtworkTooLarge {
                                reason: ArtworkTooLargeReason::LengthOverflow,
                                limit: MAX_ARTWORK_BYTES + MAX_HTTP_HEADER_BYTES,
                                actual: usize::MAX,
                            })
                    }
                })
                .transpose()?;
        }
    }
}

fn read_http_response_into<S>(stream: &mut S, response: &mut [u8]) -> Result<usize, Error<S::Error>>
where
    S: ByteStream,
{
    let mut response_len = 0;
    let mut chunk = [0_u8; 1024];
    let mut target_len = None;
    loop {
        if target_len.is_some_and(|target| response_len >= target) {
            return Ok(response_len);
        }

        let count = stream.read(&mut chunk).map_err(Error::Connect)?;
        if count == 0 {
            return Ok(response_len);
        }

        let next_len = response_len.saturating_add(count);
        if next_len > response.len() {
            return Err(Error::ArtworkBufferTooSmall {
                buffer: ArtworkBuffer::HttpResponse,
                required: next_len,
                actual: response.len(),
            });
        }
        response[response_len..next_len].copy_from_slice(&chunk[..count]);
        response_len = next_len;

        let Some(header_end) = http_header_end(&response[..response_len]) else {
            if response_len > MAX_HTTP_HEADER_BYTES {
                return Err(Error::InvalidHttpResponse);
            }
            continue;
        };
        if target_len.is_none() {
            target_len = http_content_length(&response[..header_end])
                .map_err(Error::erase_transport)?
                .map(|body_len| {
                    if body_len > MAX_ARTWORK_BYTES {
                        Err(Error::ArtworkTooLarge {
                            reason: ArtworkTooLargeReason::ContentLength,
                            limit: MAX_ARTWORK_BYTES,
                            actual: body_len,
                        })
                    } else {
                        let required =
                            header_end
                                .checked_add(body_len)
                                .ok_or(Error::ArtworkTooLarge {
                                    reason: ArtworkTooLargeReason::LengthOverflow,
                                    limit: response.len(),
                                    actual: usize::MAX,
                                })?;
                        if required > response.len() {
                            Err(Error::ArtworkBufferTooSmall {
                                buffer: ArtworkBuffer::HttpResponse,
                                required,
                                actual: response.len(),
                            })
                        } else {
                            Ok(required)
                        }
                    }
                })
                .transpose()?;
        }
    }
}

fn response_body(response: &[u8]) -> Result<&[u8], Error<core::convert::Infallible>> {
    let Some(header_end) = http_header_end(response) else {
        return Err(Error::InvalidHttpResponse);
    };
    let status_line_end = response[..header_end]
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or(Error::InvalidHttpResponse)?;
    let status_line = &response[..status_line_end];
    if !status_line.starts_with(b"HTTP/1.") || !status_line.windows(5).any(|part| part == b" 200 ")
    {
        return Err(Error::HttpError);
    }
    Ok(&response[header_end..])
}

fn http_header_end(response: &[u8]) -> Option<usize> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn http_content_length(headers: &[u8]) -> Result<Option<usize>, Error<core::convert::Infallible>> {
    let headers = core::str::from_utf8(headers).map_err(|_| Error::InvalidHttpResponse)?;
    for line in headers.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            let length = parse_usize(value.trim()).ok_or(Error::InvalidHttpResponse)?;
            return Ok(Some(length));
        }
    }
    Ok(None)
}

fn decode_jpeg_artwork(
    uri: &str,
    bytes: &[u8],
) -> Result<HifiArtwork, Error<core::convert::Infallible>> {
    let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
    let decoded = decoder.decode().map_err(|_| Error::ArtworkDecode)?;
    let info = decoder.info().ok_or(Error::ArtworkDecode)?;
    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 {
        return Err(Error::ArtworkDecode);
    }
    let components = decoded
        .len()
        .checked_div(width.saturating_mul(height))
        .ok_or(Error::ArtworkDecode)?;
    decoded_jpeg_artwork(uri, &decoded, width, height, components)
}

fn decode_jpeg_artwork_into(
    uri: &str,
    bytes: &[u8],
    decode_buffer: &mut [u8],
) -> Result<HifiArtwork, Error<core::convert::Infallible>> {
    let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
    decoder.decode_headers().map_err(|_| Error::ArtworkDecode)?;
    let required = decoder.output_buffer_size().ok_or(Error::ArtworkDecode)?;
    if required > decode_buffer.len() {
        return Err(Error::ArtworkBufferTooSmall {
            buffer: ArtworkBuffer::DecodeOutput,
            required,
            actual: decode_buffer.len(),
        });
    }

    decoder
        .decode_into(&mut decode_buffer[..required])
        .map_err(|_| Error::ArtworkDecode)?;
    let info = decoder.info().ok_or(Error::ArtworkDecode)?;
    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 {
        return Err(Error::ArtworkDecode);
    }
    let decoded = &decode_buffer[..required];
    let components = decoded
        .len()
        .checked_div(width.saturating_mul(height))
        .ok_or(Error::ArtworkDecode)?;
    decoded_jpeg_artwork(uri, decoded, width, height, components)
}

fn decode_jpeg_artwork_into_pixels(
    bytes: &[u8],
    decode_buffer: &mut [u8],
    artwork_pixels: &mut [ArtworkPixel; HIFI_ARTWORK_PIXELS],
) -> Result<(), Error<core::convert::Infallible>> {
    let mut decoder = JpegDecoder::new(ZCursor::new(bytes));
    decoder.decode_headers().map_err(|_| Error::ArtworkDecode)?;
    let required = decoder.output_buffer_size().ok_or(Error::ArtworkDecode)?;
    if required > decode_buffer.len() {
        return Err(Error::ArtworkBufferTooSmall {
            buffer: ArtworkBuffer::DecodeOutput,
            required,
            actual: decode_buffer.len(),
        });
    }

    decoder
        .decode_into(&mut decode_buffer[..required])
        .map_err(|_| Error::ArtworkDecode)?;
    let info = decoder.info().ok_or(Error::ArtworkDecode)?;
    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 {
        return Err(Error::ArtworkDecode);
    }
    let decoded = &decode_buffer[..required];
    let components = decoded
        .len()
        .checked_div(width.saturating_mul(height))
        .ok_or(Error::ArtworkDecode)?;
    decoded_jpeg_artwork_into_pixels(decoded, width, height, components, artwork_pixels)
}

fn decoded_jpeg_artwork(
    uri: &str,
    decoded: &[u8],
    width: usize,
    height: usize,
    components: usize,
) -> Result<HifiArtwork, Error<core::convert::Infallible>> {
    if !matches!(components, 1 | 3 | 4) {
        return Err(Error::ArtworkDecode);
    }

    let crop_size = width.min(height);
    let crop_x = (width - crop_size) / 2;
    let crop_y = (height - crop_size) / 2;
    let mut artwork = HifiArtwork::new(uri).ok_or(Error::InvalidArtworkUri)?;
    for y in 0..HIFI_ARTWORK_SIZE as usize {
        let source_y = crop_y + (y * crop_size / HIFI_ARTWORK_SIZE as usize);
        for x in 0..HIFI_ARTWORK_SIZE as usize {
            let source_x = crop_x + (x * crop_size / HIFI_ARTWORK_SIZE as usize);
            let source = (source_y * width + source_x) * components;
            let (r, g, b) = if components == 1 {
                let luma = decoded[source];
                (luma, luma, luma)
            } else {
                (decoded[source], decoded[source + 1], decoded[source + 2])
            };
            if !artwork.push_rgb888(r, g, b) {
                return Err(Error::ArtworkDecode);
            }
        }
    }
    if !artwork.is_complete() {
        return Err(Error::ArtworkDecode);
    }
    Ok(artwork)
}

fn decoded_jpeg_artwork_into_pixels(
    decoded: &[u8],
    width: usize,
    height: usize,
    components: usize,
    artwork_pixels: &mut [ArtworkPixel; HIFI_ARTWORK_PIXELS],
) -> Result<(), Error<core::convert::Infallible>> {
    if !matches!(components, 1 | 3 | 4) {
        return Err(Error::ArtworkDecode);
    }

    let crop_size = width.min(height);
    let crop_x = (width - crop_size) / 2;
    let crop_y = (height - crop_size) / 2;
    for y in 0..HIFI_ARTWORK_SIZE as usize {
        let source_y = crop_y + (y * crop_size / HIFI_ARTWORK_SIZE as usize);
        for x in 0..HIFI_ARTWORK_SIZE as usize {
            let source_x = crop_x + (x * crop_size / HIFI_ARTWORK_SIZE as usize);
            let source = (source_y * width + source_x) * components;
            let (r, g, b) = if components == 1 {
                let luma = decoded[source];
                (luma, luma, luma)
            } else {
                (decoded[source], decoded[source + 1], decoded[source + 2])
            };
            artwork_pixels[(y * HIFI_ARTWORK_SIZE as usize) + x] =
                ArtworkPixel::new(r >> 3, g >> 2, b >> 3);
        }
    }
    Ok(())
}

fn parse_u32(value: &str) -> Option<u32> {
    let mut number = 0_u32;
    if value.is_empty() {
        return None;
    }
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .saturating_mul(10)
            .saturating_add((byte - b'0') as u32);
    }
    Some(number)
}

fn parse_usize(value: &str) -> Option<usize> {
    let mut number = 0_usize;
    if value.is_empty() {
        return None;
    }
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .saturating_mul(10)
            .saturating_add((byte - b'0') as usize);
    }
    Some(number)
}

fn parse_u16(value: &str) -> Option<u16> {
    let number = parse_u32(value)?;
    u16::try_from(number).ok()
}

fn apply_metadata(status: &mut HifiStatus, metadata: &str) {
    copy_tag(metadata, "dc:title", &mut status.title);
    copy_tag(metadata, "upnp:artist", &mut status.artist);
    copy_tag(metadata, "upnp:album", &mut status.album);
    copy_album_art_uri(metadata, &mut status.album_art_uri);

    if status.artist.is_empty() {
        copy_tag(metadata, "artist", &mut status.artist);
    }
    if status.album.is_empty() {
        copy_tag(metadata, "album", &mut status.album);
    }
}

fn copy_tag<const N: usize>(xml: &str, tag: &str, output: &mut heapless::String<N>) {
    let Some((content_start, content_end)) =
        find_tag_content(xml, tag).or_else(|| find_escaped_tag_content(xml, tag))
    else {
        return;
    };

    output.clear();
    copy_xml_text(&xml[content_start..content_end], output);
}

fn copy_album_art_uri<const N: usize>(xml: &str, output: &mut heapless::String<N>) {
    if copy_album_art_uri_for_profile(xml, "JPEG_TN", output) {
        prefer_smaller_qobuz_art(output);
        return;
    }
    if copy_album_art_uri_for_profile(xml, "", output) {
        prefer_smaller_qobuz_art(output);
    }
}

fn copy_album_art_uri_for_profile<const N: usize>(
    xml: &str,
    profile: &str,
    output: &mut heapless::String<N>,
) -> bool {
    let mut offset = 0;
    while offset < xml.len() {
        let escaped = xml[offset..].contains("&lt;upnp:albumArtURI")
            && !xml[offset..].contains("<upnp:albumArtURI");
        let start_needle = if escaped {
            "&lt;upnp:albumArtURI"
        } else {
            "<upnp:albumArtURI"
        };
        let end_needle = if escaped { "&gt;" } else { ">" };
        let close_needle = if escaped {
            "&lt;/upnp:albumArtURI&gt;"
        } else {
            "</upnp:albumArtURI>"
        };

        let Some(relative_start) = xml[offset..].find(start_needle) else {
            return false;
        };
        let tag_start = offset + relative_start;
        let Some(content_start) = xml[tag_start..]
            .find(end_needle)
            .map(|relative| tag_start + relative + end_needle.len())
        else {
            return false;
        };
        let open_tag = &xml[tag_start..content_start];
        if profile.is_empty() || open_tag.contains(profile) {
            let Some(content_end) = xml[content_start..]
                .find(close_needle)
                .map(|relative| content_start + relative)
            else {
                return false;
            };
            output.clear();
            copy_xml_text(&xml[content_start..content_end], output);
            return !output.is_empty();
        }

        offset = content_start;
    }

    false
}

fn prefer_smaller_qobuz_art<const N: usize>(uri: &mut heapless::String<N>) {
    if !uri.starts_with("https://static.qobuz.com/") && !uri.starts_with("http://static.qobuz.com/")
    {
        return;
    }

    let Some(extension_start) = uri.rfind('.') else {
        return;
    };
    let Some(size_start) = uri[..extension_start].rfind('_') else {
        return;
    };
    if uri[size_start + 1..extension_start].is_empty()
        || !uri[size_start + 1..extension_start]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return;
    }

    let mut smaller = heapless::String::<N>::new();
    if smaller.push_str(&uri[..size_start]).is_err()
        || smaller.push_str("_50").is_err()
        || smaller.push_str(&uri[extension_start..]).is_err()
    {
        return;
    }

    *uri = smaller;
}

fn find_tag_content(xml: &str, tag: &str) -> Option<(usize, usize)> {
    let start_tag = find_tag_start(xml, tag)?;
    let content_start = xml[start_tag..]
        .find('>')
        .map(|offset| start_tag + offset + 1)?;
    let close = closing_tag(tag);
    let content_end = xml[content_start..]
        .find(close.as_str())
        .map(|offset| content_start + offset)?;
    Some((content_start, content_end))
}

fn find_escaped_tag_content(xml: &str, tag: &str) -> Option<(usize, usize)> {
    let start_tag = find_escaped_tag_start(xml, tag)?;
    let content_start = xml[start_tag..]
        .find("&gt;")
        .map(|offset| start_tag + offset + 4)?;
    let close = escaped_closing_tag(tag);
    let content_end = xml[content_start..]
        .find(close.as_str())
        .map(|offset| content_start + offset)?;
    Some((content_start, content_end))
}

fn find_tag_start(xml: &str, tag: &str) -> Option<usize> {
    let mut needle = heapless::String::<32>::new();
    needle.push('<').ok()?;
    needle.push_str(tag).ok()?;
    xml.find(needle.as_str())
}

fn find_escaped_tag_start(xml: &str, tag: &str) -> Option<usize> {
    let mut needle = heapless::String::<40>::new();
    needle.push_str("&lt;").ok()?;
    needle.push_str(tag).ok()?;
    xml.find(needle.as_str())
}

fn closing_tag(tag: &str) -> heapless::String<40> {
    let mut close = heapless::String::new();
    let _ = close.push_str("</");
    let _ = close.push_str(tag);
    let _ = close.push('>');
    close
}

fn escaped_closing_tag(tag: &str) -> heapless::String<48> {
    let mut close = heapless::String::new();
    let _ = close.push_str("&lt;/");
    let _ = close.push_str(tag);
    let _ = close.push_str("&gt;");
    close
}

fn copy_xml_text<const N: usize>(value: &str, output: &mut heapless::String<N>) {
    let mut first_pass = heapless::String::<N>::new();
    copy_xml_text_once(value, &mut first_pass);
    output.clear();
    copy_xml_text_once(first_pass.as_str(), output);
}

fn copy_xml_text_once<const N: usize>(value: &str, output: &mut heapless::String<N>) {
    let mut offset = 0;
    while offset < value.len() {
        if value.as_bytes()[offset] != b'&' {
            let Some(ch) = value[offset..].chars().next() else {
                return;
            };
            let _ = output.push(ch);
            offset += ch.len_utf8();
            continue;
        }

        let entity = if value[offset..].starts_with("&quot;") {
            Some(('"', 6))
        } else if value[offset..].starts_with("&amp;") {
            Some(('&', 5))
        } else if value[offset..].starts_with("&apos;") {
            Some(('\'', 6))
        } else if value[offset..].starts_with("&lt;") {
            Some(('<', 4))
        } else if value[offset..].starts_with("&gt;") {
            Some(('>', 4))
        } else {
            None
        };

        if let Some((ch, len)) = entity {
            let _ = output.push(ch);
            offset += len;
        } else {
            let _ = output.push('&');
            offset += 1;
        }
    }
}

#[derive(Debug)]
pub struct LpecTransport<S> {
    stream: S,
}

impl<S> LpecTransport<S> {
    pub const fn new(stream: S) -> Self {
        Self { stream }
    }

    pub fn into_stream(self) -> S {
        self.stream
    }
}

impl<S> LpecTransport<S>
where
    S: ByteStream,
{
    pub fn sync(mut self) -> Result<Self, Error<S::Error>> {
        while self.read_line().is_ok() {}

        self.stream.write_all(b"\r\n").map_err(Error::Connect)?;
        self.stream.flush().map_err(Error::Connect)?;

        while self.read_line().is_ok() {}
        Ok(self)
    }
}

impl<S> Transport for LpecTransport<S>
where
    S: ByteStream,
{
    type Error = Error<S::Error>;

    fn write_line(&mut self, line: &str) -> Result<(), Self::Error> {
        self.stream
            .write_all(line.as_bytes())
            .map_err(Error::Connect)?;
        self.stream.write_all(b"\r\n").map_err(Error::Connect)?;
        self.stream.flush().map_err(Error::Connect)
    }

    fn read_line(&mut self) -> Result<Line, Self::Error> {
        let mut buffer = [0_u8; linn_lpec::MAX_LINE_LEN];
        let mut length = 0;
        let mut byte = [0; 1];

        loop {
            let count = self.stream.read(&mut byte).map_err(Error::Connect)?;
            if count == 0 {
                return Err(Error::UnexpectedEof);
            }

            match byte[0] {
                b'\n' => {
                    let value = core::str::from_utf8(&buffer[..length])
                        .map_err(|_| Error::Protocol(linn_lpec::Error::InvalidUtf8))?;
                    let mut line = Line::new();
                    line.push_str(value).map_err(|_| Error::LineTooLong)?;
                    return Ok(line);
                }
                b'\r' => {}
                value => {
                    if length == buffer.len() {
                        return Err(Error::LineTooLong);
                    }
                    buffer[length] = value;
                    length += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::VecDeque, vec::Vec as AllocVec};

    #[test]
    fn parses_didl_track_metadata() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"<?xml version="1.0" encoding="UTF-8"?><DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:dc="http://purl.org/dc/elements/1.1/"><item id="55642869" parentID="-1" restricted="1"><upnp:class>object.item.audioItem.musicTrack</upnp:class><dc:title>Caroline</dc:title><upnp:album>Village</upnp:album><upnp:artist>Jacob Banks</upnp:artist><upnp:albumArtURI dlna:profileID="JPEG_MED">http://192.168.7.218/art/medium.jpg</upnp:albumArtURI><upnp:albumArtURI dlna:profileID="JPEG_TN">http://192.168.7.218/art/thumb.jpg</upnp:albumArtURI></item></DIDL-Lite>"#,
        );

        assert_eq!(status.title.as_str(), "Caroline");
        assert_eq!(status.artist.as_str(), "Jacob Banks");
        assert_eq!(status.album.as_str(), "Village");
        assert_eq!(
            status.album_art_uri.as_str(),
            "http://192.168.7.218/art/thumb.jpg"
        );
    }

    #[test]
    fn parses_lpec_escaped_didl_track_metadata() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"&lt;DIDL-Lite xmlns:upnp=&quot;urn:schemas-upnp-org:metadata-1-0/upnp/&quot; xmlns:dc=&quot;http://purl.org/dc/elements/1.1/&quot;&gt;&lt;item&gt;&lt;dc:title&gt;Chips n Queso&lt;/dc:title&gt;&lt;upnp:album&gt;Five Star Michelin&lt;/upnp:album&gt;&lt;upnp:artist&gt;Lorde&lt;/upnp:artist&gt;&lt;upnp:albumArtURI dlna:profileID=&quot;JPEG_TN&quot;&gt;https://static.qobuz.com/images/covers/thumb.jpg&lt;/upnp:albumArtURI&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;"#,
        );

        assert_eq!(status.title.as_str(), "Chips n Queso");
        assert_eq!(status.artist.as_str(), "Lorde");
        assert_eq!(status.album.as_str(), "Five Star Michelin");
        assert_eq!(
            status.album_art_uri.as_str(),
            "https://static.qobuz.com/images/covers/thumb.jpg"
        );
    }

    #[test]
    fn unescapes_nested_entities_in_metadata_text() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"<DIDL-Lite><item><dc:title>Rock &amp;quot;Roll&amp;quot; &amp;amp; More</dc:title><upnp:album>Left &amp;lt;Right&amp;gt;</upnp:album><upnp:artist>John &amp;apos;Jack&amp;apos;</upnp:artist></item></DIDL-Lite>"#,
        );

        assert_eq!(status.title.as_str(), r#"Rock "Roll" & More"#);
        assert_eq!(status.album.as_str(), "Left <Right>");
        assert_eq!(status.artist.as_str(), "John 'Jack'");
    }

    #[test]
    fn prefers_smaller_qobuz_artwork_uri() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"<DIDL-Lite><item><upnp:albumArtURI>https://static.qobuz.com/images/covers/zl/mi/go7xvf8bnmizl_600.jpg</upnp:albumArtURI></item></DIDL-Lite>"#,
        );

        assert_eq!(
            status.album_art_uri.as_str(),
            "https://static.qobuz.com/images/covers/zl/mi/go7xvf8bnmizl_50.jpg"
        );
    }

    #[test]
    fn prefers_tiny_qobuz_artwork_uri_from_existing_thumbnail() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"<DIDL-Lite><item><upnp:albumArtURI>https://static.qobuz.com/images/covers/52/35/0724357473552_230.jpg</upnp:albumArtURI></item></DIDL-Lite>"#,
        );

        assert_eq!(
            status.album_art_uri.as_str(),
            "https://static.qobuz.com/images/covers/52/35/0724357473552_50.jpg"
        );
    }

    #[test]
    fn leaves_non_qobuz_artwork_uri_unchanged() {
        let mut status = HifiStatus::empty();

        apply_metadata(
            &mut status,
            r#"<DIDL-Lite><item><upnp:albumArtURI>http://192.168.7.218/art/cover_600.jpg</upnp:albumArtURI></item></DIDL-Lite>"#,
        );

        assert_eq!(
            status.album_art_uri.as_str(),
            "http://192.168.7.218/art/cover_600.jpg"
        );
    }

    #[test]
    fn session_subscribes_and_applies_info_event_metadata() {
        let mut session = LpecSession::new();
        let mut stream = ScriptedByteStream::new(&[
            r#"EVENT 2 0 Metatext "&lt;DIDL-Lite&gt;&lt;item&gt;&lt;dc:title&gt;Chips n Queso&lt;/dc:title&gt;&lt;upnp:artist&gt;Lorde&lt;/upnp:artist&gt;&lt;/item&gt;&lt;/DIDL-Lite&gt;""#,
        ]);

        let status = session.poll(&mut stream).unwrap().unwrap();

        assert_eq!(status.title.as_str(), "Chips n Queso");
        assert_eq!(status.artist.as_str(), "Lorde");
        assert!(stream.writes_as_str().contains("SUBSCRIBE Ds/Info"));
    }

    #[test]
    fn session_inferrs_playing_from_advancing_seconds() {
        let mut session = LpecSession::new();
        let mut stream =
            ScriptedByteStream::new(&[r#"EVENT 1 0 Duration "180""#, r#"EVENT 1 1 Seconds "2""#]);

        assert!(session.poll(&mut stream).unwrap().is_some());
        let status = session.poll(&mut stream).unwrap().unwrap();

        assert_eq!(status.elapsed_seconds, 2);
        assert_eq!(status.playback, PlaybackState::Playing);
    }

    #[test]
    fn session_updates_playback_after_toggle_command() {
        let mut session = LpecSession::new();
        let mut stream = ScriptedByteStream::new(&[r#"RESPONSE "Playing""#, "RESPONSE"]);

        let status = session
            .handle_command(&mut stream, HifiCommand::TogglePlayback)
            .unwrap()
            .unwrap();

        assert_eq!(status.playback, PlaybackState::Paused);
        assert!(
            stream
                .writes_as_str()
                .contains("ACTION Ds/Playlist 1 Pause")
        );
    }

    #[test]
    fn parses_transient_transport_states_as_buffering() {
        assert_eq!(parse_playback_state("Buffering"), PlaybackState::Buffering);
        assert_eq!(
            parse_playback_state("TRANSITIONING"),
            PlaybackState::Buffering
        );
    }

    #[test]
    fn artwork_request_uses_plain_http_for_https_hosts() {
        let request =
            ArtworkRequest::parse("https://static.qobuz.com/images/covers/mb/x1/cover_230.jpg")
                .unwrap();

        assert_eq!(request.host, "static.qobuz.com");
        assert_eq!(request.port, 80);
        assert_eq!(request.path, "images/covers/mb/x1/cover_230.jpg");
    }

    #[test]
    fn artwork_response_stops_after_content_length_body() {
        let mut stream = ScriptedByteStream::from_bytes(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nbody",
        );

        let response = read_http_response(&mut stream).unwrap();

        assert_eq!(response_body(&response).unwrap(), b"body");
    }

    #[test]
    fn fixed_artwork_response_buffer_stops_after_content_length_body() {
        let mut stream = ScriptedByteStream::from_bytes(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nbody",
        );
        let mut response = [0_u8; 96];

        let response_len = read_http_response_into(&mut stream, &mut response).unwrap();

        assert_eq!(response_body(&response[..response_len]).unwrap(), b"body");
    }

    #[test]
    fn fixed_artwork_response_buffer_reports_required_size() {
        let mut stream = ScriptedByteStream::from_bytes(
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: keep-alive\r\n\r\nbody",
        );
        let mut response = [0_u8; 64];

        let error = read_http_response_into(&mut stream, &mut response).unwrap_err();

        assert_eq!(
            error,
            Error::ArtworkBufferTooSmall {
                buffer: ArtworkBuffer::HttpResponse,
                required: 66,
                actual: 64
            }
        );
    }

    struct ScriptedByteStream {
        reads: VecDeque<u8>,
        writes: AllocVec<u8>,
    }

    impl ScriptedByteStream {
        fn new(lines: &[&str]) -> Self {
            let mut reads = VecDeque::new();
            for line in lines {
                reads.extend(line.as_bytes());
                reads.extend(b"\r\n");
            }
            Self {
                reads,
                writes: AllocVec::new(),
            }
        }

        fn from_bytes(bytes: &[u8]) -> Self {
            Self {
                reads: bytes.iter().copied().collect(),
                writes: AllocVec::new(),
            }
        }

        fn writes_as_str(&self) -> &str {
            core::str::from_utf8(&self.writes).unwrap()
        }
    }

    impl ByteStream for ScriptedByteStream {
        type Error = ();

        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
            let Some(byte) = self.reads.pop_front() else {
                return Err(());
            };
            buffer[0] = byte;
            Ok(1)
        }

        fn write_all(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
            self.writes.extend_from_slice(bytes);
            Ok(())
        }

        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
}
