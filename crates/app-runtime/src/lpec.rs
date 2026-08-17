//! LPEC-backed [`HifiController`] implementation plus the persistent session
//! used by the firmware event loop.
//!
//! Background on the wire protocol lives in the [`linn_lpec`] crate; the
//! Linn-authored spec is at
//! <https://docs.linn.co.uk/wiki/index.php/Developer:LPEC>. This module is
//! where we turn LPEC into the app's notion of "hi-fi status / artwork /
//! pins / commands".
//!
//! ## Two clients in one file
//!
//! - [`LpecHifi`] is a *connection-per-call* [`HifiController`]: each
//!   `status()` / `artwork()` / `handle_command()` opens a fresh TCP
//!   connection. Used by the simulator's worker thread, where polling is
//!   acceptable.
//! - [`LpecSession`] holds the **persistent** subscription used by the
//!   firmware: subscribe once to `Ds/Time` + `Ds/Info` + `Ds/Volume`, then
//!   stream `EVENT` lines, interleaving the occasional command on the same
//!   socket. This avoids the connection churn and polling latency that the
//!   per-call path incurs. See `docs/lpec subscriptions.md` for background.
//!
//! ## Argument encoding quirks
//!
//! LPEC's framing is text-with-XML-escaping, but the payload format inside
//! each `ACTION` / `RESPONSE` quoted arg is *service-specific*:
//!
//! | Action | Argument format |
//! | --- | --- |
//! | `Ds/Volume:SetVolume`, `Ds/Pins:InvokeId` | plain integer |
//! | `Ds/Info:Metatext` / `Track` events | DIDL-Lite XML (entity-escaped) |
//! | `Ds/Pins:ReadList` request/response | JSON pin metadata, fetched one ID at a time |
//! | `Ds/Pins:GetIdArray` response | JSON array of pin IDs, e.g. `"[1,2,3]"` |
//!
//! `apply_metadata` decodes the DIDL-Lite case, [`decode_pin_ids`] handles
//! the `GetIdArray` JSON array used to enable hardware-mapped pins, and
//! [`parse_pin_list_json`] handles optional titles.

use alloc::boxed::Box;
use alloc::vec::Vec as AllocVec;

// TEMP diagnostic macro. With the `lpec-diag` feature on (firmware), this
// routes formatted lines through esp-println. Without it, it's a no-op so the
// sim and any host build don't pull in esp-println at all.
#[cfg(feature = "lpec-diag")]
macro_rules! diag_log {
    ($($arg:tt)*) => { esp_println::println!($($arg)*) };
}
#[cfg(not(feature = "lpec-diag"))]
macro_rules! diag_log {
    ($($arg:tt)*) => {{}};
}

use app_core::{
    ArtworkPixel, HIFI_ARTWORK_PIXELS, HIFI_ARTWORK_SIZE, HIFI_PIN_COUNT, HIFI_PIN_TITLE_LEN,
    HIFI_VOLUME_MAX, HifiArtwork, HifiCommand, HifiPin, HifiPins, HifiStatus, PlaybackState,
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
/// Upper bound on event lines drained from a single `poll()`. Stops a misbehaving
/// device from holding the worker thread indefinitely.
const SESSION_POLL_LINE_BUDGET: usize = 32;
/// Consecutive `is_read_timeout` errors tolerated immediately after subscribing
/// before giving up on the initial event burst. Each tolerated timeout is one
/// read-timeout window (see `event_read_timeout` on the connector); the device
/// pushes initial-state events for every subscribed service, but the burst may
/// not have started by the time the first read fires.
const SESSION_SUBSCRIBE_PATIENCE: usize = 10;
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

#[derive(Debug)]
pub struct LpecSessionHifi<C>
where
    C: TcpConnector,
{
    connector: C,
    endpoint: Endpoint,
    session: LpecSession,
    pending_status: Option<HifiStatus>,
}

impl<C> LpecSessionHifi<C>
where
    C: TcpConnector,
{
    pub fn new(connector: C, endpoint: Endpoint) -> Self {
        Self {
            connector,
            endpoint,
            session: LpecSession::new(),
            pending_status: None,
        }
    }

    pub fn reset(&mut self) {
        self.session.reset();
        self.connector.reset(self.endpoint);
    }

    pub fn into_connector(self) -> C {
        self.connector
    }
}

impl<C> HifiController for LpecSessionHifi<C>
where
    C: TcpConnector,
{
    type Error = Error<C::Error>;

    fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error> {
        let result = {
            let mut stream = self
                .connector
                .connect(self.endpoint)
                .map_err(Error::Connect)?;
            self.session.handle_command(&mut stream, command)
        };

        match result {
            Ok(status) => {
                self.pending_status = status;
                Ok(())
            }
            Err(error) => {
                self.reset();
                Err(error)
            }
        }
    }

    fn status(&mut self) -> Result<HifiStatus, Self::Error> {
        if let Some(status) = self.pending_status.take() {
            return Ok(status);
        }

        let result = {
            let mut stream = self
                .connector
                .connect_events(self.endpoint)
                .map_err(Error::Connect)?;
            self.session.poll(&mut stream)
        };

        match result {
            Ok(Some(status)) => Ok(status),
            Ok(None) => self.session.live_status().ok_or(Error::StatusUnavailable),
            Err(Error::Connect(error)) => self.session.live_status().ok_or(Error::Connect(error)),
            Err(error) => {
                self.reset();
                Err(error)
            }
        }
    }

    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
        load_artwork(&mut self.connector, uri)
    }

    fn pins(&mut self) -> Result<HifiPins, Self::Error> {
        let mut stream = self
            .connector
            .connect(self.endpoint)
            .map_err(Error::Connect)?;
        self.session.fetch_pins(&mut stream)
    }

    fn mark_track_changed(&mut self) {
        self.pending_status = None;
        self.session.clear_track_metadata();
    }
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
        Ok(LinnClient::new(LpecTransport::new(stream)))
    }

    fn invoke_pin_id(&mut self, id: u32) -> Result<(), Error<C::Error>> {
        let mut id_arg = heapless::String::<11>::new();
        core::fmt::write(&mut id_arg, format_args!("{id}")).map_err(|_| Error::LineTooLong)?;
        action_with_retry(&mut self.client()?, linn_lpec::invoke_pin_arg(&id_arg)).map(|_| ())
    }

    fn set_volume(&mut self, volume: u8) -> Result<(), Error<C::Error>> {
        let clamped = volume.min(HIFI_VOLUME_MAX);
        let mut volume_arg = heapless::String::<3>::new();
        core::fmt::write(&mut volume_arg, format_args!("{clamped}"))
            .map_err(|_| Error::LineTooLong)?;
        self.client()?
            .action(linn_lpec::set_ds_volume_arg(&volume_arg))
            .map(|_| ())
            .map_err(map_client_error)
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

    fn previous_track(&mut self) -> Result<(), Error<C::Error>> {
        self.client()?
            .action(linn_lpec::playlist_previous())
            .map(|_| ())
            .map_err(map_client_error)
    }

    fn next_track(&mut self) -> Result<(), Error<C::Error>> {
        self.client()?
            .action(linn_lpec::playlist_next())
            .map(|_| ())
            .map_err(map_client_error)
    }

    pub fn fetch_pins(&mut self) -> Result<HifiPins, Error<C::Error>> {
        let mut client = self.client()?;
        fetch_pins(&mut client)
    }
}

impl<C> HifiController for LpecHifi<C>
where
    C: TcpConnector,
{
    type Error = Error<C::Error>;

    fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error> {
        match command {
            HifiCommand::InvokePinId { id } => self.invoke_pin_id(id),
            HifiCommand::PreviousTrack => self.previous_track(),
            HifiCommand::TogglePlayback => self.toggle_playback(),
            HifiCommand::NextTrack => self.next_track(),
            HifiCommand::SetVolume { volume } => self.set_volume(volume),
        }
    }

    fn status(&mut self) -> Result<HifiStatus, Self::Error> {
        read_status(&mut self.client()?)
    }

    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
        self.load_artwork(uri)
    }

    fn pins(&mut self) -> Result<HifiPins, Self::Error> {
        self.fetch_pins()
    }
}

pub fn handle_command_with_stream<S>(stream: S, command: HifiCommand) -> Result<(), Error<S::Error>>
where
    S: ByteStream,
{
    let mut client = LinnClient::new(LpecTransport::new(stream).sync()?);
    match command {
        HifiCommand::InvokePinId { id } => invoke_pin_id(&mut client, id),
        HifiCommand::PreviousTrack => {
            action_with_retry(&mut client, linn_lpec::playlist_previous()).map(|_| ())
        }
        HifiCommand::TogglePlayback => {
            let playback = read_playback_state(&mut client)?;
            if playback_can_pause(playback) {
                action_with_retry(&mut client, linn_lpec::playlist_pause()).map(|_| ())
            } else {
                action_with_retry(&mut client, linn_lpec::playlist_play()).map(|_| ())
            }
        }
        HifiCommand::NextTrack => {
            action_with_retry(&mut client, linn_lpec::playlist_next()).map(|_| ())
        }
        HifiCommand::SetVolume { volume } => {
            let clamped = volume.min(HIFI_VOLUME_MAX);
            let mut volume_arg = heapless::String::<3>::new();
            core::fmt::write(&mut volume_arg, format_args!("{clamped}"))
                .map_err(|_| Error::LineTooLong)?;
            action_with_retry(&mut client, linn_lpec::set_ds_volume_arg(&volume_arg)).map(|_| ())
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
    // Reusable buffer for the args of the most recent action response. Boxed
    // so the ~16 KB sits on the heap rather than inline on the session — the
    // session is itself embedded in the firmware's hifi embassy state machine
    // and growing it shifted statics in a way that correlated with WiFi-blob
    // crashes. Boxing keeps `LpecSession`'s footprint the same as before the
    // stack-overflow fix while still letting `action()` return a borrowed view
    // instead of a 16 KB by-value `Vec`.
    response_args: Box<heapless::Vec<linn_lpec::ResponseArg, { linn_lpec::MAX_ARGS }>>,
}

impl LpecSession {
    pub fn new() -> Self {
        Self {
            subscribed: false,
            status: HifiStatus::empty(),
            response_args: Box::new(heapless::Vec::new()),
        }
    }

    pub fn reset(&mut self) {
        self.subscribed = false;
    }

    /// Clear track-derived fields (title, artist, album, art, durations) so a
    /// new track loads cleanly. Volume and playback state are preserved because
    /// they come from their own subscriptions (`Ds/Volume`, `Ds/Info`'s
    /// `TransportState`) and the device only re-emits them when they actually
    /// change.
    pub fn clear_track_metadata(&mut self) {
        self.status.title.clear();
        self.status.artist.clear();
        self.status.album.clear();
        self.status.album_art_uri.clear();
        self.status.elapsed_seconds = 0;
        self.status.duration_seconds = 0;
    }

    pub fn live_status(&self) -> Option<HifiStatus> {
        if status_has_live_content(&self.status) {
            Some(self.status.clone())
        } else {
            None
        }
    }

    pub fn poll<S>(&mut self, stream: &mut S) -> Result<Option<HifiStatus>, Error<S::Error>>
    where
        S: ByteStream,
    {
        let just_subscribed = self.ensure_subscribed(stream)?;

        let mut changed = false;
        let mut lines_read = 0_usize;
        let mut consecutive_timeouts = 0_usize;

        while lines_read < SESSION_POLL_LINE_BUDGET {
            match read_lpec_line(stream) {
                Ok(line) => {
                    lines_read += 1;
                    consecutive_timeouts = 0;
                    diag_log!("poll rx: {}", line.as_str());
                    let message = match linn_lpec::parse_message(line.as_str()) {
                        Ok(message) => message,
                        Err(_error) => {
                            // One malformed event must not tear down the
                            // session — skip it and keep reading. (Most
                            // commonly: a metadata blob we can't parse.)
                            diag_log!("poll parse error: {:?}", _error);
                            continue;
                        }
                    };
                    changed |= self.handle_message(message);
                }
                Err(Error::Connect(error)) if S::is_read_timeout(&error) => {
                    consecutive_timeouts += 1;
                    // Right after subscribing the device may not have started
                    // emitting initial-state events yet — wait a few timeout
                    // windows for the burst. Once we've read at least one line,
                    // a single timeout means the buffer is drained.
                    let limit = if just_subscribed && lines_read == 0 {
                        SESSION_SUBSCRIBE_PATIENCE
                    } else {
                        1
                    };
                    if consecutive_timeouts >= limit {
                        break;
                    }
                }
                Err(error) => return Err(error),
            }
        }

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
            HifiCommand::InvokePinId { id } => {
                let mut pin_arg = heapless::String::<11>::new();
                core::fmt::write(&mut pin_arg, format_args!("{id}"))
                    .map_err(|_| Error::LineTooLong)?;
                changed |= self
                    .action(stream, linn_lpec::invoke_pin_arg(&pin_arg))?
                    .changed;
            }
            HifiCommand::PreviousTrack => {
                changed |= self.action(stream, linn_lpec::playlist_previous())?.changed;
            }
            HifiCommand::SetVolume { volume } => {
                let clamped = volume.min(HIFI_VOLUME_MAX);
                let mut volume_arg = heapless::String::<3>::new();
                core::fmt::write(&mut volume_arg, format_args!("{clamped}"))
                    .map_err(|_| Error::LineTooLong)?;
                let response = self.action(stream, linn_lpec::set_ds_volume_arg(&volume_arg))?;
                changed |= response.changed;
                if self.status.volume_percent != clamped {
                    self.status.volume_percent = clamped;
                    changed = true;
                }
            }
            HifiCommand::NextTrack => {
                changed |= self.action(stream, linn_lpec::playlist_next())?.changed;
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

    /// One-shot fetch of the device pin list. Returns up to `HIFI_PIN_COUNT`
    /// pin entries with their IDs from `Ds/Pins:GetIdArray`.
    ///
    /// Observed Linn DSMs return the hardware pin IDs directly as a JSON
    /// array (`"[1,2,3,4,5,6]"`). Those IDs are enough to invoke pins; optional
    /// titles are fetched with one `ReadList "[id]"` request per pin so a
    /// large or malformed metadata response cannot disable the whole pin list.
    pub fn fetch_pins<S>(&mut self, stream: &mut S) -> Result<HifiPins, Error<S::Error>>
    where
        S: ByteStream,
    {
        self.ensure_subscribed(stream)?;

        // Inner block bounds the borrow of `self.response_args` so the helper
        // below can take `&mut self` again.
        let device_ids = {
            let response = self.action(stream, linn_lpec::pins_id_array())?;
            decode_pin_ids(response.args.iter().map(|s| s.as_str()))
                .ok_or(Error::Protocol(linn_lpec::Error::InvalidMessage))?
        };
        if device_ids.is_empty() {
            return Ok(HifiPins::new());
        }

        let mut pins = pins_from_ids(&device_ids);
        fetch_pin_titles_with_session(self, stream, &device_ids, &mut pins);
        Ok(pins)
    }

    /// Returns `true` if this call performed the SUBSCRIBE handshake, `false`
    /// if the session was already subscribed. Used by `poll` to grant extra
    /// patience on the initial event burst.
    fn ensure_subscribed<S>(&mut self, stream: &mut S) -> Result<bool, Error<S::Error>>
    where
        S: ByteStream,
    {
        if self.subscribed {
            return Ok(false);
        }

        write_lpec_line(stream, "")?;
        for service in [
            linn_lpec::Service::Playlist,
            linn_lpec::Service::Time,
            linn_lpec::Service::Info,
            linn_lpec::Service::Volume,
        ] {
            let line = linn_lpec::format_subscribe(service).map_err(Error::Protocol)?;
            write_lpec_line(stream, line.as_str())?;
        }
        self.subscribed = true;
        Ok(true)
    }

    fn action<S>(
        &mut self,
        stream: &mut S,
        action: linn_lpec::Action<'_>,
    ) -> Result<SessionActionResponse<'_>, Error<S::Error>>
    where
        S: ByteStream,
    {
        let line = linn_lpec::format_action(action).map_err(Error::Protocol)?;
        write_lpec_line(stream, line.as_str())?;

        let mut changed = false;
        for _ in 0..SESSION_ACTION_LINE_BUDGET {
            let line = read_lpec_line(stream)?;
            // TEMP diagnostic: trace the raw LPEC line we read, what
            // parse_message thinks of it, and how we filled response_args.
            // Remove once the InvalidMessage on pin fetch is understood.
            diag_log!("lpec rx: {}", line.as_str());
            let parsed = match linn_lpec::parse_message(line.as_str()) {
                Ok(message) => message,
                Err(_error) => {
                    // Skip malformed lines while waiting for our response —
                    // a stray badly-formatted event must not abort the
                    // action. If nothing parses within the budget the loop
                    // exits with UnexpectedMessage below.
                    diag_log!("lpec parse error: {:?}", _error);
                    continue;
                }
            };
            match parsed {
                linn_lpec::Message::Response { args } => {
                    diag_log!("lpec response: argc={}", args.len());
                    if let Some(first) = args.first() {
                        let bytes = first.as_bytes();
                        let n = bytes.len().min(120);
                        if let Ok(_text) = core::str::from_utf8(&bytes[..n]) {
                            diag_log!("lpec response arg0: {}", _text);
                        }
                    }
                    fill_session_response_args(&args, &mut self.response_args)
                        .map_err(Error::erase_transport)?;
                    return Ok(SessionActionResponse {
                        args: &self.response_args,
                        changed,
                    });
                }
                linn_lpec::Message::Error { code, description } => {
                    diag_log!("lpec remote error: code={} desc={}", code, description);
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
        if changed {
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

struct SessionActionResponse<'a> {
    args: &'a [linn_lpec::ResponseArg],
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

    if let Ok(args) = client.action(linn_lpec::time()).map_err(map_client_error)
        && args.len() >= 3
    {
        status.duration_seconds = parse_u32(&args[1]).unwrap_or(0);
        status.elapsed_seconds = parse_u32(&args[2])
            .unwrap_or(0)
            .min(status.duration_seconds);
    }

    if let Ok(args) = client
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

fn invoke_pin_id<S>(
    client: &mut LinnClient<LpecTransport<S>>,
    id: u32,
) -> Result<(), Error<S::Error>>
where
    S: ByteStream,
{
    let mut pin_arg = heapless::String::<11>::new();
    core::fmt::write(&mut pin_arg, format_args!("{id}")).map_err(|_| Error::LineTooLong)?;
    action_with_retry(client, linn_lpec::invoke_pin_arg(&pin_arg)).map(|_| ())
}

fn fetch_pins<S>(client: &mut LinnClient<LpecTransport<S>>) -> Result<HifiPins, Error<S::Error>>
where
    S: ByteStream,
{
    let id_array_response = client
        .action(linn_lpec::pins_id_array())
        .map_err(map_client_error)?;
    let device_ids = decode_pin_ids(id_array_response.iter().map(|s| s.as_str()))
        .ok_or(Error::Protocol(linn_lpec::Error::InvalidMessage))?;
    if device_ids.is_empty() {
        return Ok(HifiPins::new());
    }

    let mut pins = pins_from_ids(&device_ids);
    fetch_pin_titles_with_client(client, &device_ids, &mut pins);
    Ok(pins)
}

fn pins_from_ids(ids: &heapless::Vec<u32, HIFI_PIN_COUNT>) -> HifiPins {
    let mut pins = HifiPins::new();
    for (slot, id) in ids.iter().copied().enumerate() {
        let _ = pins.set(
            slot,
            HifiPin {
                id,
                title: heapless::String::new(),
            },
        );
    }
    pins
}

fn fetch_pin_titles_with_client<S>(
    client: &mut LinnClient<LpecTransport<S>>,
    ids: &heapless::Vec<u32, HIFI_PIN_COUNT>,
    pins: &mut HifiPins,
) where
    S: ByteStream,
{
    for (slot, id) in ids.iter().copied().enumerate() {
        let Some(ids_arg) = format_single_id_json(id) else {
            continue;
        };
        let Ok(response) = client
            .action(linn_lpec::pins_read_list_arg(&ids_arg))
            .map_err(map_client_error)
        else {
            continue;
        };
        apply_pin_title(slot, id, response.iter().map(|s| s.as_str()), pins);
    }
}

fn fetch_pin_titles_with_session<S>(
    session: &mut LpecSession,
    stream: &mut S,
    ids: &heapless::Vec<u32, HIFI_PIN_COUNT>,
    pins: &mut HifiPins,
) where
    S: ByteStream,
{
    for (slot, id) in ids.iter().copied().enumerate() {
        let Some(ids_arg) = format_single_id_json(id) else {
            continue;
        };
        let Ok(response) = session.action(stream, linn_lpec::pins_read_list_arg(&ids_arg)) else {
            continue;
        };
        apply_pin_title(slot, id, response.args.iter().map(|s| s.as_str()), pins);
    }
}

fn format_single_id_json(id: u32) -> Option<heapless::String<13>> {
    let mut ids_arg = heapless::String::<13>::new();
    ids_arg.push('[').ok()?;
    core::fmt::write(&mut ids_arg, format_args!("{id}")).ok()?;
    ids_arg.push(']').ok()?;
    Some(ids_arg)
}

fn apply_pin_title<'a, I>(slot: usize, id: u32, args: I, pins: &mut HifiPins)
where
    I: Iterator<Item = &'a str>,
{
    let titles = parse_pin_list_json(args);
    let Some(pin) = titles.get(0) else {
        return;
    };
    if pin.id != id {
        return;
    }
    let _ = pins.set(slot, pin.clone());
}

/// Decode the response of `Ds/Pins:GetIdArray`.
///
/// Returns the non-zero IDs trimmed to at most `HIFI_PIN_COUNT`; `None` means
/// the response shape is not recognized.
fn decode_pin_ids<'a>(
    mut args: impl Iterator<Item = &'a str>,
) -> Option<heapless::Vec<u32, HIFI_PIN_COUNT>> {
    let input = args.next()?;
    if args.next().is_some() {
        return None;
    }
    let bytes = input.as_bytes();
    let mut pos = 0;
    skip_json_ws(bytes, &mut pos);
    if bytes.get(pos) != Some(&b'[') {
        return None;
    }
    pos += 1;

    let mut ids = heapless::Vec::<u32, HIFI_PIN_COUNT>::new();
    loop {
        skip_json_ws(bytes, &mut pos);
        match bytes.get(pos).copied()? {
            b']' => return Some(ids),
            b',' => {
                pos += 1;
                continue;
            }
            b'0'..=b'9' => {}
            _ => return None,
        }

        let start = pos;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            pos += 1;
        }
        let id = parse_u32(&input[start..pos])?;
        if id != 0 && ids.len() < HIFI_PIN_COUNT {
            let _ = ids.push(id);
        }

        skip_json_ws(bytes, &mut pos);
        match bytes.get(pos).copied()? {
            b',' => pos += 1,
            b']' => return Some(ids),
            _ => return None,
        }
    }
}

fn skip_json_ws(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && (bytes[*pos] as char).is_ascii_whitespace() {
        *pos += 1;
    }
}

/// Parse the JSON `List` returned by `Ds/Pins:ReadList`. Linn returns a
/// flat array of pin objects each with at least `"Id"` (number) and
/// `"Title"` (string) fields. This is a deliberately small scanner — we
/// only need those two fields per entry, not full JSON support.
fn parse_pin_list_json<'a, I>(args: I) -> HifiPins
where
    I: Iterator<Item = &'a str>,
{
    let mut pins = HifiPins::new();
    let mut slot = 0;
    for arg in args {
        if slot >= HIFI_PIN_COUNT {
            break;
        }
        for entry in JsonPinEntries::new(arg) {
            if slot >= HIFI_PIN_COUNT {
                break;
            }
            let Some(id) = entry.id else {
                continue;
            };
            if id == 0 {
                continue;
            }
            let mut title_buf = heapless::String::<HIFI_PIN_TITLE_LEN>::new();
            if let Some(title) = entry.title {
                for ch in title.chars() {
                    if title_buf.push(ch).is_err() {
                        break;
                    }
                }
            }
            pins.set(
                slot,
                HifiPin {
                    id,
                    title: title_buf,
                },
            );
            slot += 1;
        }
    }
    pins
}

#[derive(Default)]
struct JsonPinEntry<'a> {
    id: Option<u32>,
    title: Option<&'a str>,
}

/// Walk a JSON-ish payload one `{...}` object at a time, pulling out `Id`
/// and `Title` per object. Tolerates whitespace, balanced braces, and any
/// other fields we don't care about.
struct JsonPinEntries<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> JsonPinEntries<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }
}

impl<'a> Iterator for JsonPinEntries<'a> {
    type Item = JsonPinEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Find the start of the next object.
        let bytes = self.src.as_bytes();
        while self.pos < bytes.len() && bytes[self.pos] != b'{' {
            self.pos += 1;
        }
        if self.pos >= bytes.len() {
            return None;
        }

        // Consume balanced braces, respecting strings.
        let start = self.pos;
        let mut depth = 0_i32;
        let mut in_string = false;
        let mut escape = false;
        while self.pos < bytes.len() {
            let b = bytes[self.pos];
            self.pos += 1;
            if in_string {
                if escape {
                    escape = false;
                } else if b == b'\\' {
                    escape = true;
                } else if b == b'"' {
                    in_string = false;
                }
                continue;
            }
            match b {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let object = &self.src[start..self.pos];
                        return Some(parse_json_pin_object(object));
                    }
                }
                _ => {}
            }
        }
        None
    }
}

fn parse_json_pin_object(object: &str) -> JsonPinEntry<'_> {
    JsonPinEntry {
        id: find_json_string_field(object, "Id")
            .and_then(parse_u32)
            .or_else(|| find_json_number_field(object, "Id")),
        title: find_json_string_field(object, "Title"),
    }
}

/// Find `"<key>"` followed by `:` and a quoted string value; return the
/// (un-escaped enough for our needs) inner content. Doesn't handle every
/// JSON escape — just the common ones we'd get back from Linn.
fn find_json_string_field<'a>(object: &'a str, key: &str) -> Option<&'a str> {
    let bytes = object.as_bytes();
    let mut pos = 0;
    while let Some(rel) = object[pos..].find('"') {
        let key_start = pos + rel + 1;
        // Read the key up to the next unescaped quote.
        let mut k = key_start;
        let mut esc = false;
        while k < bytes.len() {
            if esc {
                esc = false;
                k += 1;
                continue;
            }
            if bytes[k] == b'\\' {
                esc = true;
                k += 1;
                continue;
            }
            if bytes[k] == b'"' {
                break;
            }
            k += 1;
        }
        if k >= bytes.len() {
            return None;
        }
        let this_key = &object[key_start..k];
        // Skip whitespace + colon.
        let mut after = k + 1;
        while after < bytes.len() && (bytes[after] as char).is_ascii_whitespace() {
            after += 1;
        }
        let is_field = after < bytes.len() && bytes[after] == b':';
        if !is_field {
            // Not a key-value position; advance past this string and continue.
            pos = k + 1;
            continue;
        }
        if this_key != key {
            pos = after + 1;
            continue;
        }
        // Skip whitespace before value.
        let mut v = after + 1;
        while v < bytes.len() && (bytes[v] as char).is_ascii_whitespace() {
            v += 1;
        }
        if v >= bytes.len() || bytes[v] != b'"' {
            return None;
        }
        let value_start = v + 1;
        let mut q = value_start;
        let mut esc = false;
        while q < bytes.len() {
            if esc {
                esc = false;
                q += 1;
                continue;
            }
            if bytes[q] == b'\\' {
                esc = true;
                q += 1;
                continue;
            }
            if bytes[q] == b'"' {
                return Some(&object[value_start..q]);
            }
            q += 1;
        }
        return None;
    }
    None
}

fn find_json_number_field(object: &str, key: &str) -> Option<u32> {
    // Fallback: locate `"<key>" :` and then parse digits.
    let mut needle = heapless::String::<40>::new();
    needle.push('"').ok()?;
    needle.push_str(key).ok()?;
    needle.push('"').ok()?;
    let mut pos = object.find(needle.as_str())?;
    pos += needle.len();
    let bytes = object.as_bytes();
    while pos < bytes.len() && (bytes[pos] as char).is_ascii_whitespace() {
        pos += 1;
    }
    if pos >= bytes.len() || bytes[pos] != b':' {
        return None;
    }
    pos += 1;
    while pos < bytes.len() && (bytes[pos] as char).is_ascii_whitespace() {
        pos += 1;
    }
    let start = pos;
    while pos < bytes.len() && bytes[pos].is_ascii_digit() {
        pos += 1;
    }
    if pos == start {
        return None;
    }
    parse_u32(&object[start..pos])
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
    // LPEC values are wrapped in `"..."`. Real Linn DSMs sometimes send
    // multi-line content inside a value (DIDL-Lite XML with raw newlines
    // between tags is the practical case), so a naive split on `\n` would
    // cut the value in half and `parse_message` would reject both halves.
    // Track quote depth and absorb embedded line terminators inside quoted
    // strings. `\"` does not toggle the quote state.
    let mut in_quotes = false;
    let mut escape = false;

    loop {
        let count = stream.read(&mut byte).map_err(Error::Connect)?;
        if count == 0 {
            return Err(Error::UnexpectedEof);
        }

        let b = byte[0];
        match b {
            b'\n' if !in_quotes => {
                let value = core::str::from_utf8(&buffer[..length])
                    .map_err(|_| Error::Protocol(linn_lpec::Error::InvalidUtf8))?;
                let mut line = Line::new();
                line.push_str(value).map_err(|_| Error::LineTooLong)?;
                return Ok(line);
            }
            // CR is always dropped (original behaviour). LF outside quotes
            // ended the line above; LF inside quotes falls through and is
            // appended as part of the value so downstream metadata parsers
            // see the bytes the device actually sent.
            b'\r' => {}
            _ => {
                if b == b'"' && !escape {
                    in_quotes = !in_quotes;
                }
                escape = b == b'\\' && !escape;
                if length == buffer.len() {
                    return Err(Error::LineTooLong);
                }
                buffer[length] = b;
                length += 1;
            }
        }
    }
}

fn fill_session_response_args(
    args: &Vec<&str, { linn_lpec::MAX_ARGS }>,
    out: &mut Vec<linn_lpec::ResponseArg, { linn_lpec::MAX_ARGS }>,
) -> Result<(), Error<core::convert::Infallible>> {
    out.clear();
    for arg in args {
        out.push(linn_lpec::ResponseArg::new())
            .map_err(|_| Error::Protocol(linn_lpec::Error::TooManyArgs))?;
        // SAFETY-ish: we just pushed; `last_mut` is Some. Writing through the
        // slot avoids a 2 KB `ResponseArg` temporary on the stack per iteration.
        let slot = out.last_mut().expect("just pushed");
        copy_xml_text(arg, slot);
    }
    Ok(())
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
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
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
        || status.volume_percent > 0
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
        if value.as_bytes()[offset] == b'\\' {
            let Some(ch) = value[offset + 1..].chars().next() else {
                return;
            };
            match ch {
                '"' | '\\' => {
                    let _ = output.push(ch);
                    offset += 1 + ch.len_utf8();
                    continue;
                }
                _ => {}
            }
        }

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
    fn read_lpec_line_keeps_newlines_inside_quoted_value() {
        // Linn DSMs ship DIDL-Lite Metadata as a single LPEC argument that
        // happens to contain raw `\n` bytes between XML tags. The line reader
        // must absorb those into the value rather than treating each as
        // end-of-line.
        let mut stream = ScriptedByteStream::from_bytes(b"KEY \"value\nwith\nlines\"\r\nNEXT\r\n");
        let line = read_lpec_line(&mut stream).unwrap();
        assert_eq!(line.as_str(), "KEY \"value\nwith\nlines\"");
        let next = read_lpec_line(&mut stream).unwrap();
        assert_eq!(next.as_str(), "NEXT");
    }

    #[test]
    fn read_lpec_line_treats_escaped_quote_as_value_content() {
        // `\"` inside `"..."` must not toggle quote state, otherwise the
        // outer value closes prematurely and the rest of the line spills
        // into what the parser thinks is the next message.
        let mut stream =
            ScriptedByteStream::from_bytes(b"KEY \"contains \\\"escaped\\\" quotes\"\r\nNEXT\r\n");
        let line = read_lpec_line(&mut stream).unwrap();
        assert_eq!(line.as_str(), "KEY \"contains \\\"escaped\\\" quotes\"");
        let next = read_lpec_line(&mut stream).unwrap();
        assert_eq!(next.as_str(), "NEXT");
    }

    #[test]
    fn session_applies_multi_line_didl_metadata_event() {
        // Regression: real Linn DSMs send the Metatext / Metadata value as
        // multi-line DIDL-Lite XML with raw `\n` characters between tags.
        // The session must reassemble the full quoted value and still
        // extract title/artist.
        let mut session = LpecSession::new();
        let bytes = b"EVENT 5 0 Metatext \"&lt;DIDL-Lite&gt;\n  \
            &lt;item&gt;\n    &lt;dc:title&gt;Chips n Queso&lt;/dc:title&gt;\n    \
            &lt;upnp:artist&gt;Lorde&lt;/upnp:artist&gt;\n  &lt;/item&gt;\n\
            &lt;/DIDL-Lite&gt;\"\r\n";
        let mut stream = ScriptedByteStream::from_bytes(bytes);

        let status = session.poll(&mut stream).unwrap().unwrap();
        assert_eq!(status.title.as_str(), "Chips n Queso");
        assert_eq!(status.artist.as_str(), "Lorde");
    }

    #[test]
    fn session_poll_skips_unparseable_lines() {
        // One malformed line from the device must not tear down the
        // session — a subsequent valid event must still apply.
        let mut session = LpecSession::new();
        let mut stream =
            ScriptedByteStream::new(&["GARBAGE NOT A VALID LPEC LINE", r#"EVENT 7 0 Volume "55""#]);

        let status = session.poll(&mut stream).unwrap().unwrap();
        assert_eq!(status.volume_percent, 55);
    }

    #[test]
    fn session_emits_volume_only_event() {
        let mut session = LpecSession::new();
        let mut stream = ScriptedByteStream::new(&[r#"EVENT 3 0 Volume "42""#]);

        let status = session.poll(&mut stream).unwrap().unwrap();

        assert_eq!(status.volume_percent, 42);
        assert!(stream.writes_as_str().contains("SUBSCRIBE Ds/Volume"));
    }

    #[test]
    fn session_inferrs_playing_from_advancing_seconds() {
        let mut session = LpecSession::new();
        let mut stream =
            ScriptedByteStream::new(&[r#"EVENT 1 0 Duration "180""#, r#"EVENT 1 1 Seconds "2""#]);

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

        fn is_read_timeout(_error: &Self::Error) -> bool {
            // Tests use the unit error solely to mean "no more scripted bytes,"
            // which is the same drain-complete signal as a real read timeout.
            true
        }
    }

    #[test]
    fn decode_pin_ids_accepts_json_array_response() {
        let args = ["[1,2,3,4,5,6]"];
        let ids = decode_pin_ids(args.iter().copied()).unwrap();
        assert_eq!(ids.as_slice(), &[1_u32, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn decode_pin_ids_skips_zero_entries() {
        let args = ["[0,1,0,2]"];
        let ids = decode_pin_ids(args.iter().copied()).unwrap();
        assert_eq!(ids.as_slice(), &[1_u32, 2]);
    }

    #[test]
    fn decode_pin_ids_rejects_non_json_array_response() {
        let args = ["1", "AAAABwAAAAs="];
        assert!(decode_pin_ids(args.iter().copied()).is_none());
    }

    #[test]
    fn decode_pin_ids_accepts_empty_json_array_response() {
        let args = ["[]"];
        let ids = decode_pin_ids(args.iter().copied()).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn decode_pin_ids_returns_empty_for_empty_array() {
        let args = ["[0,0]"];
        let ids = decode_pin_ids(args.iter().copied()).unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn parse_pin_list_json_reads_flat_array() {
        let payload = r#"[{"Id":7,"Mode":"radio","Title":"Radio"},{"Id":11,"Title":"Spotify"}]"#;
        let pins = parse_pin_list_json(core::iter::once(payload));
        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some("Radio"));
        assert_eq!(pins.get(1).map(|p| p.id), Some(11));
        assert_eq!(pins.get(1).map(|p| p.title.as_str()), Some("Spotify"));
        assert!(pins.get(2).is_none());
    }

    #[test]
    fn parse_pin_list_json_skips_entries_without_id() {
        // Second object has no Id — skipped; third still populates slot 1.
        let payload = r#"[{"Id":7,"Title":"A"},{"Title":"missing"},{"Id":9,"Title":"C"}]"#;
        let pins = parse_pin_list_json(core::iter::once(payload));
        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(1).map(|p| p.id), Some(9));
    }

    #[test]
    fn parse_pin_list_json_empty_array_yields_no_pins() {
        let pins = parse_pin_list_json(core::iter::once("[]"));
        assert!(pins.slots().iter().all(|s| s.is_none()));
    }

    #[test]
    fn fetch_pins_via_session_uses_idarray_then_readlist() {
        let id_array = r#"RESPONSE "[7,11]""#;
        let pin_7 = r#"RESPONSE "[{&quot;Id&quot;:7,&quot;Title&quot;:&quot;Radio&quot;}]" "#;
        let pin_11 = r#"RESPONSE "[{&quot;Id&quot;:11,&quot;Title&quot;:&quot;Spotify&quot;}]" "#;
        let mut stream = ScriptedByteStream::new(&[id_array, pin_7, pin_11]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();

        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some("Radio"));
        assert_eq!(pins.get(1).map(|p| p.id), Some(11));
        let writes = stream.writes_as_str();
        assert!(
            writes.contains("ACTION Ds/Pins 1 GetIdArray"),
            "writes: {writes}"
        );
        assert!(
            writes.contains("ACTION Ds/Pins 1 ReadList \"[7]\""),
            "writes: {writes}"
        );
        assert!(
            writes.contains("ACTION Ds/Pins 1 ReadList \"[11]\""),
            "writes: {writes}"
        );
    }

    #[test]
    fn fetch_pins_accepts_json_idarray_response() {
        let id_array = r#"RESPONSE "[1,2,3,4,5,6]""#;
        let pin_1 = "RESPONSE \"[{&quot;Id&quot;:1,&quot;Title&quot;:&quot;Radio&quot;}]\"";
        let pin_2 = "RESPONSE \"[{&quot;Id&quot;:2,&quot;Title&quot;:&quot;Spotify&quot;}]\"";
        let mut stream = ScriptedByteStream::new(&[id_array, pin_1, pin_2]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();

        assert_eq!(pins.get(0).map(|p| p.id), Some(1));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some("Radio"));
        assert_eq!(pins.get(1).map(|p| p.id), Some(2));
        assert_eq!(pins.get(1).map(|p| p.title.as_str()), Some("Spotify"));
        assert!(
            stream
                .writes_as_str()
                .contains("ACTION Ds/Pins 1 ReadList \"[1]\"")
        );
    }

    #[test]
    fn fetch_pins_accepts_backslash_escaped_readlist_json() {
        let id_array = r#"RESPONSE "[7,11]""#;
        let pin_7 = r#"RESPONSE "[{\"Id\":7,\"Title\":\"Radio\"}]""#;
        let pin_11 = r#"RESPONSE "[{\"Id\":11,\"Title\":\"Spotify\"}]""#;
        let mut stream = ScriptedByteStream::new(&[id_array, pin_7, pin_11]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();

        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some("Radio"));
        assert_eq!(pins.get(1).map(|p| p.id), Some(11));
        assert_eq!(pins.get(1).map(|p| p.title.as_str()), Some("Spotify"));
    }

    #[test]
    fn fetch_pins_accepts_raw_readlist_json_quotes() {
        let id_array = r#"RESPONSE "[7,11]""#;
        let pin_7 = r#"RESPONSE "[{"Id":7,"Title":"Radio"}]""#;
        let pin_11 = r#"RESPONSE "[{"Id":11,"Title":"Spotify"}]""#;
        let mut stream = ScriptedByteStream::new(&[id_array, pin_7, pin_11]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();

        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some("Radio"));
        assert_eq!(pins.get(1).map(|p| p.id), Some(11));
        assert_eq!(pins.get(1).map(|p| p.title.as_str()), Some("Spotify"));
    }

    #[test]
    fn fetch_pins_keeps_ids_when_title_fetch_fails() {
        let id_array = r#"RESPONSE "[7,11]""#;
        let mut stream = ScriptedByteStream::new(&[id_array, r#"ERROR 801 "JsonCorrupt""#]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();

        assert_eq!(pins.get(0).map(|p| p.id), Some(7));
        assert_eq!(pins.get(0).map(|p| p.title.as_str()), Some(""));
        assert_eq!(pins.get(1).map(|p| p.id), Some(11));
        assert_eq!(pins.get(1).map(|p| p.title.as_str()), Some(""));
    }

    #[test]
    fn fetch_pins_propagates_idarray_remote_error() {
        // Device rejects IdArray — fetch_pins surfaces the Protocol error,
        // it does NOT silently fall back to slot indices.
        let mut stream = ScriptedByteStream::new(&[r#"ERROR 800 "Not supported""#]);
        let mut session = LpecSession::new();

        let err = session.fetch_pins(&mut stream).unwrap_err();
        match err {
            Error::Protocol(linn_lpec::Error::Remote { code, .. }) => assert_eq!(code, 800),
            other => panic!("expected Protocol(Remote{{..}}), got {other:?}"),
        }
        // Crucially, we never sent ReadList — no point guessing IDs.
        assert!(
            !stream.writes_as_str().contains("ReadList"),
            "should not have sent ReadList after IdArray failed"
        );
    }

    #[test]
    fn fetch_pins_returns_empty_when_device_has_no_pins() {
        let mut stream = ScriptedByteStream::new(&[r#"RESPONSE "[]""#]);
        let mut session = LpecSession::new();

        let pins = session.fetch_pins(&mut stream).unwrap();
        assert!(pins.slots().iter().all(|s| s.is_none()));
        assert!(
            !stream.writes_as_str().contains("ReadList"),
            "should skip ReadList when IdArray is empty"
        );
    }
}
