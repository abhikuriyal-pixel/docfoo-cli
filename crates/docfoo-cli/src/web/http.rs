//! Tiny HTTP helpers shared by the local browsers.
//!
//! `tiny_http` hands each request to a worker thread; these helpers keep the
//! routing code in each visualizer small and identical: URL splitting, query
//! decoding, bounded body reads and JSON responses.

use std::io::Read;

use serde_json::{json, Value};
use tiny_http::{Header, Request, Response, ResponseBox, StatusCode};

/// Split `/path?a=b` into the path and the raw query string.
pub fn split_url(url: &str) -> (&str, Option<&str>) {
    match url.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (url, None),
    }
}

/// One decoded query parameter (`+` becomes a space, `%XX` decoded).
pub fn query_param(query_string: Option<&str>, key: &str) -> Option<String> {
    let query_string = query_string?;
    for pair in query_string.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return Some(percent_decode(value));
        }
    }
    None
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                if let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                    out.push(high * 16 + low);
                    index += 3;
                    continue;
                }
                out.push(b'%');
                index += 1;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Read a request body, capped at `max` bytes.
pub fn read_body(request: &mut Request, max: u64) -> std::result::Result<String, String> {
    let mut body = String::new();
    request
        .as_reader()
        .take(max)
        .read_to_string(&mut body)
        .map_err(|error| format!("could not read the request body: {error}"))?;
    Ok(body)
}

pub fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

/// A JSON response with `no-store` caching.
pub fn json_response(status: u16, value: &Value) -> ResponseBox {
    Response::from_string(value.to_string())
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
        .boxed()
}

/// `{"error": message}` with the given status.
pub fn error_response(status: u16, message: &str) -> ResponseBox {
    json_response(status, &json!({ "error": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_params_are_decoded() {
        let query = "scope=papers%2Fml&path=Book%2Fassets%2Ff%20x.png&flag";
        assert_eq!(query_param(Some(query), "scope").unwrap(), "papers/ml");
        assert_eq!(
            query_param(Some(query), "path").unwrap(),
            "Book/assets/f x.png"
        );
        assert_eq!(query_param(Some(query), "flag").unwrap(), "");
        assert!(query_param(Some(query), "missing").is_none());
    }

    #[test]
    fn urls_split_cleanly() {
        assert_eq!(split_url("/api/graph?scope="), ("/api/graph", Some("scope=")));
        assert_eq!(split_url("/"), ("/", None));
    }
}
