use app_core::{HifiCommand, HifiStatus, PlaybackState};
use linn_lpec::{Client as LinnClient, Line, Transport};

use crate::{
    HifiController,
    net::{ByteStream, Endpoint, TcpConnector},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Connect(E),
    LineTooLong,
    UnexpectedEof,
    Protocol(linn_lpec::Error<core::convert::Infallible>),
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

fn apply_metadata(status: &mut HifiStatus, metadata: &str) {
    copy_tag(metadata, "dc:title", &mut status.title);
    copy_tag(metadata, "upnp:artist", &mut status.artist);
    copy_tag(metadata, "upnp:album", &mut status.album);

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
            r#"<?xml version="1.0" encoding="UTF-8"?><DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:dc="http://purl.org/dc/elements/1.1/"><item id="55642869" parentID="-1" restricted="1"><upnp:class>object.item.audioItem.musicTrack</upnp:class><dc:title>Caroline</dc:title><upnp:album>Village</upnp:album><upnp:artist>Jacob Banks</upnp:artist></item></DIDL-Lite>"#,
        );

        assert_eq!(status.title.as_str(), "Caroline");
        assert_eq!(status.artist.as_str(), "Jacob Banks");
        assert_eq!(status.album.as_str(), "Village");
    }

    #[test]
    fn parses_transient_transport_states_as_buffering() {
        assert_eq!(parse_playback_state("Buffering"), PlaybackState::Buffering);
        assert_eq!(
            parse_playback_state("TRANSITIONING"),
            PlaybackState::Buffering
        );
    }
}
