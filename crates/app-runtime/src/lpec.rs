use app_core::{HIFI_ARTWORK_SIZE, HifiArtwork, HifiCommand, HifiStatus, PlaybackState};
use heapless::Vec;
use linn_lpec::{Client as LinnClient, Line, Transport};
use zune_core::bytestream::ZCursor;
use zune_jpeg::JpegDecoder;

use crate::{
    HifiController,
    net::{ByteStream, Endpoint, TcpConnector},
};

const MAX_ARTWORK_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Connect(E),
    LineTooLong,
    UnexpectedEof,
    InvalidArtworkUri,
    InvalidHttpResponse,
    HttpError,
    ArtworkTooLarge,
    ArtworkDecode,
    Protocol(linn_lpec::Error<core::convert::Infallible>),
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
            Self::ArtworkTooLarge => Error::ArtworkTooLarge,
            Self::ArtworkDecode => Error::ArtworkDecode,
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

    fn client(&mut self) -> Result<LinnClient<LpecTransport<C::Stream>>, Error<C::Error>> {
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
        let request = ArtworkRequest::parse(uri).map_err(Error::erase_transport)?;
        let mut stream = self
            .connector
            .connect_host(request.host, request.port)
            .map_err(Error::Connect)?;
        write_http_get(&mut stream, request.host, request.path).map_err(Error::Connect)?;
        let response = read_http_response(&mut stream)?;
        let body = response_body(&response).map_err(Error::erase_transport)?;
        decode_jpeg_artwork(uri, body).map_err(Error::erase_transport)
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
        let mut client = self.client()?;
        let mut status = HifiStatus::empty();

        if let Ok(playback) = read_playback_state(&mut client) {
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
        {
            if let Some(metadata) = args.first() {
                apply_metadata(&mut status, metadata);
            }
        }
        if status.title.is_empty()
            && let Ok(args) = client
                .action(linn_lpec::info_track())
                .map_err(map_client_error)
            && let Some(metadata) = args.get(1)
        {
            apply_metadata(&mut status, metadata);
        }

        Ok(status)
    }

    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
        self.load_artwork(uri)
    }
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

fn read_http_response<S>(stream: &mut S) -> Result<Vec<u8, MAX_ARTWORK_BYTES>, Error<S::Error>>
where
    S: ByteStream,
{
    let mut response = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream.read(&mut chunk).map_err(Error::Connect)?;
        if count == 0 {
            return Ok(response);
        }
        response
            .extend_from_slice(&chunk[..count])
            .map_err(|_| Error::ArtworkTooLarge)?;
    }
}

fn response_body(response: &[u8]) -> Result<&[u8], Error<core::convert::Infallible>> {
    let Some(header_end) = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
    else {
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
    let Some(start_tag) = find_tag_start(xml, tag) else {
        return;
    };
    let Some(content_start) = xml[start_tag..]
        .find('>')
        .map(|offset| start_tag + offset + 1)
    else {
        return;
    };
    let close = closing_tag(tag);
    let Some(content_end) = xml[content_start..]
        .find(close.as_str())
        .map(|offset| content_start + offset)
    else {
        return;
    };

    output.clear();
    copy_xml_text(&xml[content_start..content_end], output);
}

fn copy_album_art_uri<const N: usize>(xml: &str, output: &mut heapless::String<N>) {
    if copy_album_art_uri_for_profile(xml, "JPEG_TN", output) {
        return;
    }
    let _ = copy_album_art_uri_for_profile(xml, "", output);
}

fn copy_album_art_uri_for_profile<const N: usize>(
    xml: &str,
    profile: &str,
    output: &mut heapless::String<N>,
) -> bool {
    let mut offset = 0;
    while offset < xml.len() {
        let Some(relative_start) = xml[offset..].find("<upnp:albumArtURI") else {
            return false;
        };
        let tag_start = offset + relative_start;
        let Some(content_start) = xml[tag_start..]
            .find('>')
            .map(|relative| tag_start + relative + 1)
        else {
            return false;
        };
        let open_tag = &xml[tag_start..content_start];
        if profile.is_empty() || open_tag.contains(profile) {
            let Some(content_end) = xml[content_start..]
                .find("</upnp:albumArtURI>")
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

fn find_tag_start(xml: &str, tag: &str) -> Option<usize> {
    let mut needle = heapless::String::<32>::new();
    needle.push('<').ok()?;
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

fn copy_xml_text<const N: usize>(value: &str, output: &mut heapless::String<N>) {
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
}
