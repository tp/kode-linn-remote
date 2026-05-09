#![cfg_attr(not(feature = "std"), no_std)]

use app_runtime::net::Endpoint;
use heapless::String;

pub const DEFAULT_LOCAL_CONFIG_PATH: &str = "config/local.env";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    pub linn_lpec_endpoint: Endpoint,
    pub linn_ci_gateway_http_endpoint: Endpoint,
    pub linn_ci_gateway_websocket_endpoint: Endpoint,
    pub wifi: WifiConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WifiConfig {
    pub ssid: Option<String<64>>,
    pub password: Option<String<64>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    InvalidLine,
    InvalidIp,
    InvalidPort,
    ValueTooLong,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            linn_lpec_endpoint: Endpoint::ipv4([127, 0, 0, 1], 23),
            linn_ci_gateway_http_endpoint: Endpoint::ipv4([127, 0, 0, 1], 4100),
            linn_ci_gateway_websocket_endpoint: Endpoint::ipv4([127, 0, 0, 1], 8088),
            wifi: WifiConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn parse_env(input: &str) -> Result<Self, ParseError> {
        let mut config = Self::default();
        let mut linn_host = config.linn_lpec_endpoint.address;
        let mut linn_lpec_port = config.linn_lpec_endpoint.port;
        let mut linn_ci_http_port = config.linn_ci_gateway_http_endpoint.port;
        let mut linn_ci_ws_port = config.linn_ci_gateway_websocket_endpoint.port;

        for raw_line in input.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return Err(ParseError::InvalidLine);
            };
            let key = key.trim();
            let value = unquote(value.trim());

            match key {
                "LINN_HOST" => linn_host = parse_ipv4(value)?,
                "LINN_LPEC_PORT" => linn_lpec_port = parse_port(value)?,
                "LINN_CI_HTTP_PORT" => linn_ci_http_port = parse_port(value)?,
                "LINN_CI_WS_PORT" => linn_ci_ws_port = parse_port(value)?,
                "WIFI_SSID" => config.wifi.ssid = non_empty_string(value)?,
                "WIFI_PASSWORD" => config.wifi.password = non_empty_string(value)?,
                _ => {}
            }
        }

        config.linn_lpec_endpoint = Endpoint::ipv4(linn_host, linn_lpec_port);
        config.linn_ci_gateway_http_endpoint = Endpoint::ipv4(linn_host, linn_ci_http_port);
        config.linn_ci_gateway_websocket_endpoint = Endpoint::ipv4(linn_host, linn_ci_ws_port);
        Ok(config)
    }

    #[cfg(feature = "std")]
    pub fn load_local_or_default() -> Self {
        let path =
            std::env::var("APP_CONFIG").unwrap_or_else(|_| DEFAULT_LOCAL_CONFIG_PATH.to_string());
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::default();
        };

        match Self::parse_env(&contents) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("failed to parse {path}: {error:?}; using defaults");
                Self::default()
            }
        }
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn non_empty_string(value: &str) -> Result<Option<String<64>>, ParseError> {
    if value.is_empty() {
        return Ok(None);
    }

    let mut output = String::new();
    output
        .push_str(value)
        .map_err(|_| ParseError::ValueTooLong)?;
    Ok(Some(output))
}

fn parse_ipv4(value: &str) -> Result<[u8; 4], ParseError> {
    let mut output = [0; 4];
    let mut parts = value.split('.');

    for octet in &mut output {
        let Some(part) = parts.next() else {
            return Err(ParseError::InvalidIp);
        };
        *octet = parse_u8(part).ok_or(ParseError::InvalidIp)?;
    }

    if parts.next().is_some() {
        return Err(ParseError::InvalidIp);
    }

    Ok(output)
}

fn parse_port(value: &str) -> Result<u16, ParseError> {
    parse_u16(value).ok_or(ParseError::InvalidPort)
}

fn parse_u8(value: &str) -> Option<u8> {
    let number = parse_u16(value)?;
    if number > u8::MAX as u16 {
        return None;
    }
    Some(number as u8)
}

fn parse_u16(value: &str) -> Option<u16> {
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
        if number > u16::MAX as u32 {
            return None;
        }
    }
    Some(number as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_config() {
        let config = AppConfig::parse_env(
            r#"
            LINN_HOST=192.168.7.218
            LINN_LPEC_PORT=23
            LINN_CI_HTTP_PORT=4100
            LINN_CI_WS_PORT=8088
            WIFI_SSID="Home WiFi"
            WIFI_PASSWORD="secret"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.linn_lpec_endpoint,
            Endpoint::ipv4([192, 168, 7, 218], 23)
        );
        assert_eq!(
            config.linn_ci_gateway_http_endpoint,
            Endpoint::ipv4([192, 168, 7, 218], 4100)
        );
        assert_eq!(config.wifi.ssid.unwrap().as_str(), "Home WiFi");
        assert_eq!(config.wifi.password.unwrap().as_str(), "secret");
    }

    #[test]
    fn rejects_invalid_ip() {
        assert_eq!(
            AppConfig::parse_env("LINN_HOST=192.168.7.999"),
            Err(ParseError::InvalidIp)
        );
    }
}
