#![no_std]

use core::fmt;

use heapless::String;

pub const DEFAULT_HTTP_PORT: u16 = 4100;
pub const DEFAULT_WEBSOCKET_PORT: u16 = 8088;
pub const WEBSOCKET_PATH: &str = "/ws";
pub const API_DOC_PATH: &str = "/res/api.html";
pub const SWAGGER_PATH: &str = "/api/swagger.yaml";

pub const MAX_MESSAGE_LEN: usize = 384;
pub const MAX_TAG_LEN: usize = 32;

pub type Message = String<MAX_MESSAGE_LEN>;
pub type Tag = String<MAX_TAG_LEN>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Method {
    Get,
    Post,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestPath {
    SessionCreate,
    SessionDestroy,
    V2ApiVersion,
    V2TopologyStatus,
    V2PinsStatus,
    V2TransportStatus,
    V2TransportPlay,
    V2TransportPause,
    V2MetadataStatus,
}

impl RequestPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionCreate => "/session/create",
            Self::SessionDestroy => "/session/destroy",
            Self::V2ApiVersion => "/V2/api/version",
            Self::V2TopologyStatus => "/V2/topology/status",
            Self::V2PinsStatus => "/V2/pins/status",
            Self::V2TransportStatus => "/V2/transport/status",
            Self::V2TransportPlay => "/V2/transport/play",
            Self::V2TransportPause => "/V2/transport/pause",
            Self::V2MetadataStatus => "/V2/metadata/status",
        }
    }

    pub const fn method(self) -> Method {
        match self {
            Self::SessionCreate
            | Self::SessionDestroy
            | Self::V2TransportPlay
            | Self::V2TransportPause => Method::Post,
            Self::V2ApiVersion
            | Self::V2TopologyStatus
            | Self::V2PinsStatus
            | Self::V2TransportStatus
            | Self::V2MetadataStatus => Method::Get,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomRequest<'a> {
    pub path: RequestPath,
    pub session: &'a str,
    pub room: &'a str,
    pub tag: Option<&'a str>,
    pub update: Option<u32>,
}

impl<'a> RoomRequest<'a> {
    pub const fn new(path: RequestPath, session: &'a str, room: &'a str) -> Self {
        Self {
            path,
            session,
            room,
            tag: None,
            update: None,
        }
    }

    pub const fn with_tag(mut self, tag: &'a str) -> Self {
        self.tag = Some(tag);
        self
    }

    pub const fn with_update(mut self, update: u32) -> Self {
        self.update = Some(update);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Fmt,
    MessageTooLong,
}

impl From<fmt::Error> for Error {
    fn from(_: fmt::Error) -> Self {
        Self::Fmt
    }
}

pub fn create_session(timeout_seconds: u32, user_agent: Option<&str>) -> Result<Message, Error> {
    let mut message = Message::new();
    message.push('{').map_err(|_| Error::MessageTooLong)?;
    write_json_field(
        &mut message,
        "requestPath",
        RequestPath::SessionCreate.as_str(),
        false,
    )?;
    fmt::write(&mut message, format_args!(",\"timeout\":{timeout_seconds}"))?;
    if let Some(user_agent) = user_agent {
        write_json_field(&mut message, "userAgent", user_agent, true)?;
    }
    message.push('}').map_err(|_| Error::MessageTooLong)?;
    Ok(message)
}

pub fn destroy_session(session: &str, tag: Option<&str>) -> Result<Message, Error> {
    let request = RoomlessRequest {
        path: RequestPath::SessionDestroy,
        session: Some(session),
        tag,
    };
    roomless_request(request)
}

pub fn api_version(tag: Option<&str>) -> Result<Message, Error> {
    roomless_request(RoomlessRequest {
        path: RequestPath::V2ApiVersion,
        session: None,
        tag,
    })
}

pub fn topology_status(
    session: &str,
    tag: Option<&str>,
    update: Option<u32>,
) -> Result<Message, Error> {
    let mut message = Message::new();
    message.push('{').map_err(|_| Error::MessageTooLong)?;
    write_json_field(
        &mut message,
        "requestPath",
        RequestPath::V2TopologyStatus.as_str(),
        false,
    )?;
    write_json_field(&mut message, "session", session, true)?;
    if let Some(tag) = tag {
        write_json_field(&mut message, "tag", tag, true)?;
    }
    push_update_before_close(&mut message, update)?;
    message.push('}').map_err(|_| Error::MessageTooLong)?;
    Ok(message)
}

pub fn pins_status(request: RoomRequest<'_>) -> Result<Message, Error> {
    debug_assert_eq!(request.path, RequestPath::V2PinsStatus);
    room_request(request)
}

pub fn transport_status(request: RoomRequest<'_>) -> Result<Message, Error> {
    debug_assert_eq!(request.path, RequestPath::V2TransportStatus);
    room_request(request)
}

pub fn transport_play(session: &str, room: &str, tag: Option<&str>) -> Result<Message, Error> {
    room_request(
        RoomRequest::new(RequestPath::V2TransportPlay, session, room).with_optional_tag(tag),
    )
}

pub fn transport_pause(session: &str, room: &str, tag: Option<&str>) -> Result<Message, Error> {
    room_request(
        RoomRequest::new(RequestPath::V2TransportPause, session, room).with_optional_tag(tag),
    )
}

pub fn metadata_status(request: RoomRequest<'_>) -> Result<Message, Error> {
    debug_assert_eq!(request.path, RequestPath::V2MetadataStatus);
    room_request(request)
}

struct RoomlessRequest<'a> {
    path: RequestPath,
    session: Option<&'a str>,
    tag: Option<&'a str>,
}

impl<'a> RoomRequest<'a> {
    const fn with_optional_tag(mut self, tag: Option<&'a str>) -> Self {
        self.tag = tag;
        self
    }
}

fn roomless_request(request: RoomlessRequest<'_>) -> Result<Message, Error> {
    let mut message = Message::new();
    message.push('{').map_err(|_| Error::MessageTooLong)?;
    write_json_field(&mut message, "requestPath", request.path.as_str(), false)?;
    if let Some(session) = request.session {
        write_json_field(&mut message, "session", session, true)?;
    }
    if let Some(tag) = request.tag {
        write_json_field(&mut message, "tag", tag, true)?;
    }
    message.push('}').map_err(|_| Error::MessageTooLong)?;
    Ok(message)
}

fn room_request(request: RoomRequest<'_>) -> Result<Message, Error> {
    let mut message = Message::new();
    message.push('{').map_err(|_| Error::MessageTooLong)?;
    write_json_field(&mut message, "requestPath", request.path.as_str(), false)?;
    write_json_field(&mut message, "session", request.session, true)?;
    write_json_field(&mut message, "room", request.room, true)?;
    if let Some(tag) = request.tag {
        write_json_field(&mut message, "tag", tag, true)?;
    }
    push_update_before_close(&mut message, request.update)?;
    message.push('}').map_err(|_| Error::MessageTooLong)?;
    Ok(message)
}

fn push_update_before_close(message: &mut Message, update: Option<u32>) -> Result<(), Error> {
    if let Some(update) = update {
        fmt::write(message, format_args!(",\"update\":{update}"))?;
    }
    Ok(())
}

fn write_json_field(
    message: &mut Message,
    key: &str,
    value: &str,
    prepend_comma: bool,
) -> Result<(), Error> {
    if prepend_comma {
        message.push(',').map_err(|_| Error::MessageTooLong)?;
    }
    message.push('"').map_err(|_| Error::MessageTooLong)?;
    push_json_string_content(message, key)?;
    message
        .push_str("\":\"")
        .map_err(|_| Error::MessageTooLong)?;
    push_json_string_content(message, value)?;
    message.push('"').map_err(|_| Error::MessageTooLong)?;
    Ok(())
}

fn push_json_string_content(message: &mut Message, value: &str) -> Result<(), Error> {
    for ch in value.chars() {
        match ch {
            '"' => message.push_str("\\\""),
            '\\' => message.push_str("\\\\"),
            '\n' => message.push_str("\\n"),
            '\r' => message.push_str("\\r"),
            '\t' => message.push_str("\\t"),
            ch if ch.is_control() => return Err(Error::MessageTooLong),
            ch => message.push(ch),
        }
        .map_err(|_| Error::MessageTooLong)?;
    }
    Ok(())
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_session_create_message() {
        assert_eq!(
            create_session(10_000, Some("esp32-rust/0.1"))
                .unwrap()
                .as_str(),
            "{\"requestPath\":\"/session/create\",\"timeout\":10000,\"userAgent\":\"esp32-rust/0.1\"}"
        );
    }

    #[test]
    fn formats_v2_transport_play_message() {
        assert_eq!(
            transport_play("s.1", "Living Room", Some("pin1"))
                .unwrap()
                .as_str(),
            "{\"requestPath\":\"/V2/transport/play\",\"session\":\"s.1\",\"room\":\"Living Room\",\"tag\":\"pin1\"}"
        );
    }

    #[test]
    fn formats_v2_metadata_subscription_message() {
        let request =
            RoomRequest::new(RequestPath::V2MetadataStatus, "s.1", "Living Room").with_update(1);

        assert_eq!(
            metadata_status(request).unwrap().as_str(),
            "{\"requestPath\":\"/V2/metadata/status\",\"session\":\"s.1\",\"room\":\"Living Room\",\"update\":1}"
        );
    }

    #[test]
    fn escapes_json_string_fields() {
        assert_eq!(
            transport_play("s.1", "Living \"Room\"", None)
                .unwrap()
                .as_str(),
            "{\"requestPath\":\"/V2/transport/play\",\"session\":\"s.1\",\"room\":\"Living \\\"Room\\\"\"}"
        );
    }
}
