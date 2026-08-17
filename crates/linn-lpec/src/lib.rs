#![no_std]
//! # `linn-lpec`
//!
//! `no_std` codec and synchronous client for **LPEC** — Linn's Linn Products
//! Event Control protocol. LPEC is a small TCP line protocol that exposes
//! the same UPnP services as the device's standard UPnP/SOAP API, but as
//! plain text framed by `\r\n`. It is not an industry standard; it's
//! specific to Linn DS/DSM hardware.
//!
//! Canonical specification:
//! <https://docs.linn.co.uk/wiki/index.php/Developer:LPEC>
//!
//! ## Wire format at a glance
//!
//! Each line is one message. Arguments are quoted strings with XML entity
//! escaping (`&quot;`, `&amp;`, `&lt;`, `&gt;`, `&apos;`). Examples:
//!
//! ```text
//! ACTION Ds/Volume 1 SetVolume "50"          (client → device)
//! RESPONSE "50"                              (device → client)
//! SUBSCRIBE Ds/Info                          (client → device)
//! EVENT 7 0 TrackDuration "187" Metatext ".." (device → client, async)
//! ERROR 801 "JsonCorrupt"                    (device → client, on action failure)
//! ```
//!
//! - Default TCP port is `23` ([`DEFAULT_PORT`]).
//! - Responses do **not** carry request IDs; pipelining is unsafe — keep one
//!   in-flight action per connection.
//! - Subscriptions deliver unsolicited `EVENT` lines on the same connection
//!   between request/response pairs; clients must tolerate them anywhere.
//!
//! ## What this crate provides
//!
//! - [`Action`] / [`format_action`] — build and format `ACTION` lines.
//! - [`Message`] / [`parse_message`] — parse any incoming line.
//! - [`Transport`] + [`Client`] — synchronous request/response wrapper over
//!   a user-supplied byte transport. Higher-level state (subscriptions,
//!   evented variables, retries) lives in `app-runtime`, not here.
//!
//! Encodings of individual arguments are service-specific: most actions take
//! plain numbers or strings, `Ds/Info:Metatext` returns escaped DIDL-Lite
//! XML, and `Ds/Pins:GetIdArray` / `ReadList` use JSON payloads. This crate
//! only handles the LPEC framing; payload decoding is the caller's job.

use core::{fmt, str};

use heapless::{String, Vec};

pub const DEFAULT_PORT: u16 = 23;
pub const MAX_LINE_LEN: usize = 4096;
pub const MAX_ARG_LEN: usize = 2048;
pub const MAX_ARGS: usize = 8;
pub const MAX_EVENTS: usize = 16;
/// How much of a remote error's description is kept.
///
/// Deliberately far shorter than [`MAX_LINE_LEN`]. This string is stored inline
/// in [`Error`], so it is also stored inline in the `Result` of every fallible
/// function in the crate. Sizing it to a whole protocol line made those results
/// kilobytes wide to carry text that is only ever logged.
pub const MAX_REMOTE_DESCRIPTION_LEN: usize = 96;

pub type Line = String<MAX_LINE_LEN>;
pub type ResponseArg = String<MAX_ARG_LEN>;
pub type RemoteDescription = String<MAX_REMOTE_DESCRIPTION_LEN>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Service {
    AvTransport,
    RenderingControl,
    Playlist,
    Info,
    Time,
    Product,
    Preamp,
    Volume,
    Pins,
}

impl Service {
    pub const fn path(self) -> &'static str {
        match self {
            Self::AvTransport => "MediaRenderer/AVTransport",
            Self::RenderingControl => "MediaRenderer/RenderingControl",
            Self::Playlist => "Ds/Playlist",
            Self::Info => "Ds/Info",
            Self::Time => "Ds/Time",
            Self::Product => "Ds/Product",
            Self::Preamp => "Preamp/Preamp",
            Self::Volume => "Ds/Volume",
            Self::Pins => "Ds/Pins",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action<'a> {
    service: Service,
    version: u8,
    name: &'a str,
    args: Arguments<'a>,
}

impl<'a> Action<'a> {
    pub const fn new(service: Service, version: u8, name: &'a str, args: &'a [&'a str]) -> Self {
        Self {
            service,
            version,
            name,
            args: Arguments::Slice(args),
        }
    }

    pub const fn one(service: Service, version: u8, name: &'a str, arg: &'a str) -> Self {
        Self {
            service,
            version,
            name,
            args: Arguments::One(arg),
        }
    }

    pub const fn service(self) -> Service {
        self.service
    }

    pub const fn version(self) -> u8 {
        self.version
    }

    pub const fn name(self) -> &'a str {
        self.name
    }

    pub const fn args(self) -> Arguments<'a> {
        self.args
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arguments<'a> {
    Slice(&'a [&'a str]),
    One(&'a str),
}

// ---------------------------------------------------------------------------
// Action constructors
//
// Each function below corresponds to one openhome.org service action.
// Action names follow the canonical service definitions documented at
// <https://docs.linn.co.uk/wiki/index.php/Developer:LPEC> (and ultimately
// the openhome.org service XML files). Two naming styles coexist:
//
// - Older `Ds/*` and `Preamp/*` services expose property getters under
//   the bare property name (e.g. `Ds/Volume:Volume`, `Preamp/Preamp:Mute`).
// - Newer services like `Ds/Pins` use modern OpenHome conventions with
//   `Get`-prefixed getters (e.g. `Ds/Pins:GetIdArray`).
//
// When adding new actions, cross-check the official service XML rather
// than guessing — the device rejects unknown action names with a
// `Remote { code: 401, .. }` style error.
// ---------------------------------------------------------------------------

pub fn play() -> Action<'static> {
    Action::new(Service::AvTransport, 1, "Play", &["0", "1"])
}

pub fn pause() -> Action<'static> {
    Action::new(Service::AvTransport, 1, "Pause", &["0"])
}

pub fn stop() -> Action<'static> {
    Action::new(Service::AvTransport, 1, "Stop", &["0"])
}

pub fn next() -> Action<'static> {
    Action::new(Service::AvTransport, 1, "Next", &["0"])
}

pub fn previous() -> Action<'static> {
    Action::new(Service::AvTransport, 1, "Previous", &["0"])
}

pub fn playlist_transport_state() -> Action<'static> {
    Action::new(Service::Playlist, 1, "TransportState", &[])
}

pub fn playlist_play() -> Action<'static> {
    Action::new(Service::Playlist, 1, "Play", &[])
}

pub fn playlist_pause() -> Action<'static> {
    Action::new(Service::Playlist, 1, "Pause", &[])
}

pub fn playlist_next() -> Action<'static> {
    Action::new(Service::Playlist, 1, "Next", &[])
}

/// Seeks within the current track, in whole seconds from its start.
///
/// Used to restart a track without changing it, which is what the remote's
/// left button does once a track is properly under way.
pub fn playlist_seek_second_absolute_arg(seconds: &str) -> Action<'_> {
    Action::one(Service::Playlist, 1, "SeekSecondAbsolute", seconds)
}

pub fn playlist_previous() -> Action<'static> {
    Action::new(Service::Playlist, 1, "Previous", &[])
}

pub fn info_metatext() -> Action<'static> {
    Action::new(Service::Info, 1, "Metatext", &[])
}

pub fn info_track() -> Action<'static> {
    Action::new(Service::Info, 1, "Track", &[])
}

pub fn time() -> Action<'static> {
    Action::new(Service::Time, 1, "Time", &[])
}

pub fn get_volume() -> Action<'static> {
    Action::new(Service::Preamp, 1, "Volume", &[])
}

pub fn get_ds_volume() -> Action<'static> {
    Action::new(Service::Volume, 1, "Volume", &[])
}

pub fn set_volume_arg(volume: &str) -> Action<'_> {
    Action::one(Service::Preamp, 1, "SetVolume", volume)
}

pub fn set_ds_volume_arg(volume: &str) -> Action<'_> {
    Action::one(Service::Volume, 1, "SetVolume", volume)
}

pub fn get_mute() -> Action<'static> {
    Action::new(Service::Preamp, 1, "Mute", &[])
}

pub fn set_mute_arg(mute: &str) -> Action<'_> {
    Action::one(Service::Preamp, 1, "SetMute", mute)
}

pub fn source_count() -> Action<'static> {
    Action::new(Service::Product, 2, "SourceCount", &[])
}

pub fn source_arg(index: &str) -> Action<'_> {
    Action::one(Service::Product, 2, "Source", index)
}

pub fn set_source_by_system_name_arg(system_name: &str) -> Action<'_> {
    Action::one(Service::Product, 2, "SetSourceBySystemName", system_name)
}

pub fn invoke_pin_arg(pin: &str) -> Action<'_> {
    Action::one(Service::Pins, 1, "InvokeId", pin)
}

pub fn invoke_pin_index_arg(index: &str) -> Action<'_> {
    Action::one(Service::Pins, 1, "InvokeIndex", index)
}

pub fn pins_id_array() -> Action<'static> {
    // OpenHome's Linn-Pins service names this getter `GetIdArray` (modern
    // service naming convention with a `Get` prefix). Older Ds services
    // like `Ds/Volume` use the bare property name (`Volume`), so don't
    // assume the same shape here.
    Action::new(Service::Pins, 1, "GetIdArray", &[])
}

pub fn pins_read_list_arg(ids: &str) -> Action<'_> {
    Action::one(Service::Pins, 1, "ReadList", ids)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventedVariable<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

// Clippy wants the `Event` variant boxed. This crate is `no_std` without
// `alloc`, so there is no box to reach for: the event list is a fixed
// `MAX_EVENTS` buffer by design, and `Message` is a short-lived borrow of one
// parsed line rather than something that is stored or queued.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message<'a> {
    Response {
        args: Vec<&'a str, MAX_ARGS>,
    },
    Subscribe {
        id: u32,
    },
    Unsubscribe {
        id: u32,
    },
    Event {
        id: u32,
        sequence: u32,
        variables: Vec<EventedVariable<'a>, MAX_EVENTS>,
    },
    Alive {
        sub_device: &'a str,
        udn: &'a str,
    },
    ByeBye {
        sub_device: &'a str,
        udn: &'a str,
    },
    Error {
        code: u16,
        description: &'a str,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error<E = core::convert::Infallible> {
    Fmt,
    LineTooLong,
    InvalidUtf8,
    InvalidMessage,
    TooManyArgs,
    TooManyEvents,
    InvalidNumber,
    UnexpectedMessage,
    Remote {
        code: u16,
        description: RemoteDescription,
    },
    Transport(E),
}

impl<E> From<fmt::Error> for Error<E> {
    fn from(_: fmt::Error) -> Self {
        Self::Fmt
    }
}

pub fn format_action(action: Action<'_>) -> Result<Line, Error> {
    let mut line = Line::new();
    fmt::write(
        &mut line,
        format_args!(
            "ACTION {} {} {}",
            action.service.path(),
            action.version,
            action.name
        ),
    )?;

    match action.args {
        Arguments::Slice(args) => {
            for arg in args {
                line.push(' ').map_err(|_| Error::LineTooLong)?;
                push_quoted_xml_arg(&mut line, arg)?;
            }
        }
        Arguments::One(arg) => {
            line.push(' ').map_err(|_| Error::LineTooLong)?;
            push_quoted_xml_arg(&mut line, arg)?;
        }
    }

    Ok(line)
}

pub fn format_subscribe(service: Service) -> Result<Line, Error> {
    let mut line = Line::new();
    fmt::write(&mut line, format_args!("SUBSCRIBE {}", service.path()))?;
    Ok(line)
}

pub fn format_unsubscribe_id(id: u32) -> Result<Line, Error> {
    let mut line = Line::new();
    fmt::write(&mut line, format_args!("UNSUBSCRIBE {id}"))?;
    Ok(line)
}

pub fn format_unsubscribe_all() -> Result<Line, Error> {
    let mut line = Line::new();
    line.push_str("UNSUBSCRIBE")
        .map_err(|_| Error::LineTooLong)?;
    Ok(line)
}

pub fn parse_message(line: &str) -> Result<Message<'_>, Error> {
    let mut parser = Parser::new(line.trim_end_matches(['\r', '\n']));
    let command = parser.next_atom().ok_or(Error::InvalidMessage)?;

    match command {
        "RESPONSE" => {
            let mut args = Vec::new();
            while let Some(arg) = parser.next_quoted()? {
                args.push(arg).map_err(|_| Error::TooManyArgs)?;
            }
            parser.finish()?;
            Ok(Message::Response { args })
        }
        "SUBSCRIBE" => {
            let id = parser.required_u32()?;
            parser.finish()?;
            Ok(Message::Subscribe { id })
        }
        "UNSUBSCRIBE" => {
            let id = parser.required_u32()?;
            parser.finish()?;
            Ok(Message::Unsubscribe { id })
        }
        "EVENT" => {
            let id = parser.required_u32()?;
            let sequence = parser.required_u32()?;
            let mut variables = Vec::new();
            while let Some(name) = parser.next_atom() {
                let value = parser.required_quoted()?;
                variables
                    .push(EventedVariable { name, value })
                    .map_err(|_| Error::TooManyEvents)?;
            }
            parser.finish()?;
            Ok(Message::Event {
                id,
                sequence,
                variables,
            })
        }
        "ALIVE" => {
            let sub_device = parser.required_atom()?;
            let udn = parser.required_atom()?;
            parser.finish()?;
            Ok(Message::Alive { sub_device, udn })
        }
        "BYEBYE" => {
            let sub_device = parser.required_atom()?;
            let udn = parser.required_atom()?;
            parser.finish()?;
            Ok(Message::ByeBye { sub_device, udn })
        }
        "ERROR" => {
            let code = parser.required_u16()?;
            let description = parser.required_quoted()?;
            parser.finish()?;
            Ok(Message::Error { code, description })
        }
        _ => Err(Error::InvalidMessage),
    }
}

pub trait Transport {
    type Error;

    fn write_line(&mut self, line: &str) -> Result<(), Self::Error>;
    fn read_line(&mut self) -> Result<Line, Self::Error>;
}

pub struct Client<T> {
    transport: T,
}

impl<T> Client<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }
}

impl<T> Client<T>
where
    T: Transport,
{
    pub fn action(
        &mut self,
        action: Action<'_>,
    ) -> Result<Vec<ResponseArg, MAX_ARGS>, Error<T::Error>> {
        let line = format_action(action).map_err(Error::erase)?;
        self.transport.write_line(&line).map_err(Error::Transport)?;

        loop {
            let line = self.transport.read_line().map_err(Error::Transport)?;
            match parse_message(&line).map_err(Error::erase)? {
                Message::Response { args } => {
                    return copy_response_args(&args).map_err(Error::erase);
                }
                Message::Error { code, description } => {
                    let description = copy_remote_description(description);
                    return Err(Error::Remote { code, description });
                }
                Message::Alive { .. } | Message::ByeBye { .. } | Message::Event { .. } => {}
                Message::Subscribe { .. } | Message::Unsubscribe { .. } => {
                    return Err(Error::UnexpectedMessage);
                }
            }
        }
    }

    pub fn play(&mut self) -> Result<(), Error<T::Error>> {
        self.action(play()).map(|_| ())
    }

    pub fn pause(&mut self) -> Result<(), Error<T::Error>> {
        self.action(pause()).map(|_| ())
    }

    pub fn stop(&mut self) -> Result<(), Error<T::Error>> {
        self.action(stop()).map(|_| ())
    }

    /// Skips to the next track. Named for the LPEC action and its `previous`
    /// counterpart, not for `Iterator::next`, which a `Client` is not.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<(), Error<T::Error>> {
        self.action(next()).map(|_| ())
    }

    pub fn previous(&mut self) -> Result<(), Error<T::Error>> {
        self.action(previous()).map(|_| ())
    }

    pub fn playlist_play(&mut self) -> Result<(), Error<T::Error>> {
        self.action(playlist_play()).map(|_| ())
    }

    pub fn playlist_pause(&mut self) -> Result<(), Error<T::Error>> {
        self.action(playlist_pause()).map(|_| ())
    }

    pub fn playlist_next(&mut self) -> Result<(), Error<T::Error>> {
        self.action(playlist_next()).map(|_| ())
    }

    pub fn playlist_previous(&mut self) -> Result<(), Error<T::Error>> {
        self.action(playlist_previous()).map(|_| ())
    }

    pub fn volume(&mut self) -> Result<u8, Error<T::Error>> {
        let args = self.action(get_volume())?;
        let volume = args.first().ok_or(Error::InvalidMessage)?;
        parse_u8(volume).ok_or(Error::InvalidNumber)
    }

    pub fn set_volume(&mut self, volume: u8) -> Result<(), Error<T::Error>> {
        let mut volume_arg = String::<3>::new();
        fmt::write(&mut volume_arg, format_args!("{volume}")).map_err(|_| Error::Fmt)?;
        self.action(set_volume_arg(&volume_arg)).map(|_| ())
    }

    pub fn mute(&mut self) -> Result<bool, Error<T::Error>> {
        let args = self.action(get_mute())?;
        let mute = args.first().ok_or(Error::InvalidMessage)?;
        match mute.as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => Err(Error::InvalidMessage),
        }
    }

    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error<T::Error>> {
        self.action(set_mute_arg(if mute { "true" } else { "false" }))
            .map(|_| ())
    }

    pub fn set_source_by_system_name(&mut self, system_name: &str) -> Result<(), Error<T::Error>> {
        self.action(set_source_by_system_name_arg(system_name))
            .map(|_| ())
    }

    pub fn invoke_pin(&mut self, pin: u8) -> Result<(), Error<T::Error>> {
        let mut pin_arg = String::<3>::new();
        fmt::write(&mut pin_arg, format_args!("{pin}")).map_err(|_| Error::Fmt)?;
        self.action(invoke_pin_arg(&pin_arg)).map(|_| ())
    }
}

impl Error {
    fn erase<F>(self) -> Error<F> {
        match self {
            Self::Fmt => Error::Fmt,
            Self::LineTooLong => Error::LineTooLong,
            Self::InvalidUtf8 => Error::InvalidUtf8,
            Self::InvalidMessage => Error::InvalidMessage,
            Self::TooManyArgs => Error::TooManyArgs,
            Self::TooManyEvents => Error::TooManyEvents,
            Self::InvalidNumber => Error::InvalidNumber,
            Self::UnexpectedMessage => Error::UnexpectedMessage,
            Self::Remote { code, description } => Error::Remote { code, description },
            Self::Transport(never) => match never {},
        }
    }
}

/// Keeps as much of a remote error's description as fits, truncating on a char
/// boundary instead of failing. The code is the part callers act on, so losing
/// it to an over-long message would be the worse trade.
fn copy_remote_description(description: &str) -> RemoteDescription {
    let mut copied = RemoteDescription::new();
    for ch in description.chars() {
        if copied.push(ch).is_err() {
            break;
        }
    }
    copied
}

fn push_quoted_xml_arg(line: &mut Line, arg: &str) -> Result<(), Error> {
    line.push('"').map_err(|_| Error::LineTooLong)?;
    for ch in arg.chars() {
        match ch {
            '"' => line.push_str("&quot;"),
            '&' => line.push_str("&amp;"),
            '\'' => line.push_str("&apos;"),
            '<' => line.push_str("&lt;"),
            '>' => line.push_str("&gt;"),
            _ => line.push(ch),
        }
        .map_err(|_| Error::LineTooLong)?;
    }
    line.push('"').map_err(|_| Error::LineTooLong)?;
    Ok(())
}

fn parse_u8(value: &str) -> Option<u8> {
    let mut number: u16 = 0;
    if value.is_empty() {
        return None;
    }
    for byte in value.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number
            .saturating_mul(10)
            .saturating_add((byte - b'0') as u16);
        if number > u8::MAX as u16 {
            return None;
        }
    }
    Some(number as u8)
}

fn copy_response_args(args: &Vec<&str, MAX_ARGS>) -> Result<Vec<ResponseArg, MAX_ARGS>, Error> {
    let mut copied = Vec::new();
    for arg in args {
        let mut value = ResponseArg::new();
        push_xml_unescaped(&mut value, arg)?;
        copied.push(value).map_err(|_| Error::TooManyArgs)?;
    }
    Ok(copied)
}

fn push_xml_unescaped(output: &mut ResponseArg, input: &str) -> Result<(), Error> {
    let bytes = input.as_bytes();
    let mut offset = 0;

    while offset < bytes.len() {
        if bytes[offset] == b'\\' {
            let Some(ch) = input[offset + 1..].chars().next() else {
                return Err(Error::InvalidMessage);
            };
            match ch {
                '"' | '\\' => {
                    output.push(ch).map_err(|_| Error::LineTooLong)?;
                    offset += 1 + ch.len_utf8();
                    continue;
                }
                _ => {}
            }
        }

        if bytes[offset] != b'&' {
            let ch = input[offset..]
                .chars()
                .next()
                .ok_or(Error::InvalidMessage)?;
            output.push(ch).map_err(|_| Error::LineTooLong)?;
            offset += ch.len_utf8();
            continue;
        }

        let entity = if input[offset..].starts_with("&quot;") {
            Some(('"', 6))
        } else if input[offset..].starts_with("&amp;") {
            Some(('&', 5))
        } else if input[offset..].starts_with("&apos;") {
            Some(('\'', 6))
        } else if input[offset..].starts_with("&lt;") {
            Some(('<', 4))
        } else if input[offset..].starts_with("&gt;") {
            Some(('>', 4))
        } else {
            None
        }
        .ok_or(Error::InvalidMessage)?;

        output.push(entity.0).map_err(|_| Error::LineTooLong)?;
        offset += entity.1;
    }

    Ok(())
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    const fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn next_atom(&mut self) -> Option<&'a str> {
        self.skip_spaces();
        if self.done() || self.peek() == Some(b'"') {
            return None;
        }
        let start = self.offset;
        while !self.done() {
            match self.peek() {
                Some(b' ' | b'\t') => break,
                Some(_) => self.offset += 1,
                None => break,
            }
        }
        Some(&self.input[start..self.offset])
    }

    fn required_atom(&mut self) -> Result<&'a str, Error> {
        self.next_atom().ok_or(Error::InvalidMessage)
    }

    fn next_quoted(&mut self) -> Result<Option<&'a str>, Error> {
        self.skip_spaces();
        if self.done() {
            return Ok(None);
        }
        if self.peek() != Some(b'"') {
            return Err(Error::InvalidMessage);
        }
        self.offset += 1;
        let start = self.offset;
        while !self.done() {
            if self.peek() == Some(b'\\') {
                self.offset += 1;
                if self.done() {
                    return Err(Error::InvalidMessage);
                }
                self.offset += 1;
                continue;
            }
            if self.peek() == Some(b'"') && self.quoted_arg_ends_here() {
                let value = &self.input[start..self.offset];
                self.offset += 1;
                return Ok(Some(value));
            }
            self.offset += 1;
        }
        Err(Error::InvalidMessage)
    }

    fn required_quoted(&mut self) -> Result<&'a str, Error> {
        self.next_quoted()?.ok_or(Error::InvalidMessage)
    }

    fn required_u16(&mut self) -> Result<u16, Error> {
        self.required_atom()?
            .parse()
            .map_err(|_| Error::InvalidNumber)
    }

    fn required_u32(&mut self) -> Result<u32, Error> {
        self.required_atom()?
            .parse()
            .map_err(|_| Error::InvalidNumber)
    }

    fn finish(&mut self) -> Result<(), Error> {
        self.skip_spaces();
        if self.done() {
            Ok(())
        } else {
            Err(Error::InvalidMessage)
        }
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.offset += 1;
        }
    }

    fn done(&self) -> bool {
        self.offset >= self.input.len()
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.offset).copied()
    }

    fn quoted_arg_ends_here(&self) -> bool {
        matches!(
            self.input.as_bytes().get(self.offset + 1).copied(),
            None | Some(b' ' | b'\t')
        )
    }
}

pub fn line_from_bytes(bytes: &[u8]) -> Result<&str, Error> {
    str::from_utf8(bytes).map_err(|_| Error::InvalidUtf8)
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn formats_transport_actions() {
        assert_eq!(
            format_action(play()).unwrap().as_str(),
            "ACTION MediaRenderer/AVTransport 1 Play \"0\" \"1\""
        );
        assert_eq!(
            format_action(next()).unwrap().as_str(),
            "ACTION MediaRenderer/AVTransport 1 Next \"0\""
        );
        assert_eq!(
            format_action(playlist_play()).unwrap().as_str(),
            "ACTION Ds/Playlist 1 Play"
        );
        assert_eq!(
            format_action(playlist_pause()).unwrap().as_str(),
            "ACTION Ds/Playlist 1 Pause"
        );
        assert_eq!(
            format_action(playlist_next()).unwrap().as_str(),
            "ACTION Ds/Playlist 1 Next"
        );
        assert_eq!(
            format_action(playlist_previous()).unwrap().as_str(),
            "ACTION Ds/Playlist 1 Previous"
        );
    }

    #[test]
    fn escapes_xml_arguments() {
        let action = set_source_by_system_name_arg("TV & \"Arc\"");

        assert_eq!(
            format_action(action).unwrap().as_str(),
            "ACTION Ds/Product 2 SetSourceBySystemName \"TV &amp; &quot;Arc&quot;\""
        );
    }

    #[test]
    fn parses_empty_response() {
        assert_eq!(
            parse_message("RESPONSE\r\n").unwrap(),
            Message::Response { args: Vec::new() }
        );
    }

    #[test]
    fn parses_response_arguments() {
        assert_eq!(
            parse_message("RESPONSE \"50\" \"Room\"").unwrap(),
            Message::Response {
                args: Vec::from_slice(&["50", "Room"]).unwrap()
            }
        );
    }

    #[test]
    fn parses_response_arguments_with_backslash_escaped_quotes() {
        assert_eq!(
            parse_message(r#"RESPONSE "[{\"Id\":7,\"Title\":\"Radio\"}]""#).unwrap(),
            Message::Response {
                args: Vec::from_slice(&[r#"[{\"Id\":7,\"Title\":\"Radio\"}]"#]).unwrap()
            }
        );
    }

    #[test]
    fn parses_response_arguments_with_raw_json_quotes() {
        assert_eq!(
            parse_message(r#"RESPONSE "[{"Id":7,"Title":"Radio"}]""#).unwrap(),
            Message::Response {
                args: Vec::from_slice(&[r#"[{"Id":7,"Title":"Radio"}]"#]).unwrap()
            }
        );
    }

    #[test]
    fn parses_events() {
        assert_eq!(
            parse_message("EVENT 49 0 ProductName \"Selekt DSM\" ProductStandby \"false\"")
                .unwrap(),
            Message::Event {
                id: 49,
                sequence: 0,
                variables: Vec::from_slice(&[
                    EventedVariable {
                        name: "ProductName",
                        value: "Selekt DSM"
                    },
                    EventedVariable {
                        name: "ProductStandby",
                        value: "false"
                    }
                ])
                .unwrap()
            }
        );
    }

    #[test]
    fn parses_remote_errors() {
        assert_eq!(
            parse_message("ERROR 103 \"Service not found\"").unwrap(),
            Message::Error {
                code: 103,
                description: "Service not found"
            }
        );
    }

    #[test]
    fn client_sends_action_and_reads_response() {
        let transport = ScriptedTransport::new(&["RESPONSE \"42\""]);
        let mut client = Client::new(transport);

        let volume = client.volume().unwrap();
        let transport = client.into_inner();

        assert_eq!(volume, 42);
        assert_eq!(transport.writes[0], "ACTION Preamp/Preamp 1 Volume");
    }

    #[test]
    fn client_unescapes_response_arguments() {
        let transport = ScriptedTransport::new(&["RESPONSE \"TV &amp; &quot;Arc&quot;\""]);
        let mut client = Client::new(transport);

        let response = client.action(source_arg("3")).unwrap();

        assert_eq!(response[0].as_str(), "TV & \"Arc\"");
    }

    #[test]
    fn client_unescapes_backslash_quoted_response_arguments() {
        let transport = ScriptedTransport::new(&[r#"RESPONSE "[{\"Id\":7,\"Title\":\"Radio\"}]""#]);
        let mut client = Client::new(transport);

        let response = client.action(pins_read_list_arg("[7]")).unwrap();

        assert_eq!(response[0].as_str(), r#"[{"Id":7,"Title":"Radio"}]"#);
    }

    #[test]
    fn client_ignores_unsolicited_alive_before_response() {
        let transport =
            ScriptedTransport::new(&["ALIVE Ds 4c494e4e-0050-c221-71e5-df000003013f", "RESPONSE"]);
        let mut client = Client::new(transport);

        client.pause().unwrap();

        assert_eq!(
            client.into_inner().writes[0],
            "ACTION MediaRenderer/AVTransport 1 Pause \"0\""
        );
    }

    #[test]
    fn client_surfaces_remote_error_code_and_description() {
        let transport = ScriptedTransport::new(&["ERROR 103 \"Service not found\""]);
        let mut client = Client::new(transport);

        assert_eq!(
            client.stop(),
            Err(Error::Remote {
                code: 103,
                description: copy_remote_description("Service not found")
            })
        );
    }

    #[test]
    fn an_over_long_remote_description_is_truncated_not_rejected() {
        // The description no longer gets a whole protocol line's worth of
        // room, so it has to degrade by losing text rather than by losing the
        // error code along with it.
        let long = "x".repeat(MAX_REMOTE_DESCRIPTION_LEN * 2);
        let copied = copy_remote_description(&long);

        assert_eq!(copied.len(), MAX_REMOTE_DESCRIPTION_LEN);
        assert!(long.starts_with(copied.as_str()));
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        // Two bytes per char, so a cut measured in bytes could land mid-char.
        let long = "é".repeat(MAX_REMOTE_DESCRIPTION_LEN);
        let copied = copy_remote_description(&long);

        assert!(copied.len() <= MAX_REMOTE_DESCRIPTION_LEN);
        assert!(copied.len() >= MAX_REMOTE_DESCRIPTION_LEN - 1);
        assert!(copied.chars().all(|ch| ch == 'é'));
    }

    struct ScriptedTransport {
        reads: VecDeque<&'static str>,
        writes: std::vec::Vec<std::string::String>,
    }

    impl ScriptedTransport {
        fn new(reads: &[&'static str]) -> Self {
            Self {
                reads: reads.iter().copied().collect(),
                writes: std::vec::Vec::new(),
            }
        }
    }

    impl Transport for ScriptedTransport {
        type Error = ();

        fn write_line(&mut self, line: &str) -> Result<(), Self::Error> {
            self.writes.push(line.into());
            Ok(())
        }

        fn read_line(&mut self) -> Result<Line, Self::Error> {
            let line = self.reads.pop_front().unwrap();
            let mut output = Line::new();
            output.push_str(line).unwrap();
            Ok(output)
        }
    }
}
