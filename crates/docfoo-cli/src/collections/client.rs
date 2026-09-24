//! Collections HTTP client — port of `src-tauri/src/collections/client.rs`
//! with `ureq` instead of `reqwest` and no Tauri progress events.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::CollectionItem;
use crate::error::{CliError, Result};

/// Hard cap for one shared-item download; the extraction quota runs after,
/// this stops a lying server from filling the disk first.
const MAX_COLLECTION_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

fn user_agent() -> String {
    format!("DocFoo-CLI/{}", env!("CARGO_PKG_VERSION"))
}

fn agent(redirects: u32, timeout_secs: u64) -> ureq::Agent {
    ureq::config::Config::builder()
        .max_redirects(redirects)
        .timeout_per_call(Some(Duration::from_secs(timeout_secs)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// Koofr rejects downloads from its public links as "hotlinking" unless the
/// request carries the Referer of the link's own page. The Collections server
/// hands out a 302 to the uploader's Koofr link, so we resolve it manually and
/// derive the link-page Referer from the target URL.
pub fn link_page_referer(location: &str) -> Option<String> {
    let rest = location.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    const MARKER: &str = "content/links/";
    let start = path.find(MARKER)? + MARKER.len();
    let link_id = path[start..].split('/').next()?;
    if link_id.is_empty() {
        return None;
    }
    Some(format!("https://{host}/links/{link_id}"))
}

pub fn list(base: &str) -> Result<Vec<CollectionItem>> {
    let mut response = agent(5, 60)
        .get(format!("{base}/items"))
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|error| CliError::Message(format!("collections server unreachable: {error}")))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| CliError::Message(format!("could not read the collections list: {error}")))?;
    if !(200..300).contains(&status) {
        return Err(CliError::Message(format!(
            "collections server error {status}"
        )));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|error| CliError::Message(format!("could not read the collections list: {error}")))?;
    let items = body
        .get("items")
        .cloned()
        .ok_or_else(|| CliError::Message("could not read the collections list: missing items".to_string()))?;
    serde_json::from_value(items)
        .map_err(|error| CliError::Message(format!("could not read the collections list: {error}")))
}

/// Download one shared item to `dest`, calling `progress(received, total)` at
/// most every 300 ms. Returns the number of bytes written.
pub fn download(
    base: &str,
    id: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<u64> {
    // Resolve the 302 ourselves so the Koofr Referer can be attached.
    let resolver = agent(0, 60);
    let response = resolver
        .get(format!("{base}/items/{id}/download"))
        .header("User-Agent", &user_agent())
        .call()
        .map_err(|error| CliError::Message(format!("collections server unreachable: {error}")))?;
    let status = response.status().as_u16();
    if status == 404 {
        return Err(CliError::NotFound("this item is no longer available".to_string()));
    }
    let response = if (300..400).contains(&status) {
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                CliError::Message("collections server error: missing download target".to_string())
            })?
            .to_string();
        let mut request = agent(5, 900)
            .get(&location)
            .header("User-Agent", &user_agent());
        if let Some(referer) = link_page_referer(&location) {
            request = request.header("Referer", &referer);
        }
        request
            .call()
            .map_err(|error| CliError::Message(format!("download failed: {error}")))?
    } else {
        response
    };
    let status = response.status().as_u16();
    if status == 404 {
        return Err(CliError::NotFound("this item is no longer available".to_string()));
    }
    if status != 200 {
        return Err(CliError::Message(format!(
            "the shared file could not be downloaded (server error {status})"
        )));
    }
    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if total > MAX_COLLECTION_DOWNLOAD_BYTES {
        return Err(CliError::Message(
            "this shared file is too large to download".to_string(),
        ));
    }

    let mut reader = response.into_body().into_reader();
    let mut file = std::fs::File::create(dest).map_err(|error| {
        CliError::Message(format!("could not create the collection download: {error}"))
    })?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    let mut last = Instant::now();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| CliError::Message(format!("could not download the collection: {error}")))?;
        if read == 0 {
            break;
        }
        received += read as u64;
        if received > MAX_COLLECTION_DOWNLOAD_BYTES {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(CliError::Message(
                "this shared file is too large to download".to_string(),
            ));
        }
        file.write_all(&buffer[..read])
            .map_err(|error| CliError::Message(format!("could not download the collection: {error}")))?;
        if last.elapsed() >= Duration::from_millis(300) {
            last = Instant::now();
            progress(received, total);
        }
    }
    progress(received, total);
    Ok(received)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn koofr_content_urls_get_a_link_page_referer() {
        let location = "https://app.koofr.net/content/links/8b958142-72f0-48b3-899e-48a08e07c3c4/files/get/multi_column_1.zip?path=%2F&force&password=254683";
        assert_eq!(
            link_page_referer(location),
            Some("https://app.koofr.net/links/8b958142-72f0-48b3-899e-48a08e07c3c4".to_string())
        );
    }

    #[test]
    fn non_koofr_urls_get_no_referer() {
        assert_eq!(link_page_referer("https://example.com/file.zip"), None);
        assert_eq!(link_page_referer("https://app.koofr.net/content/other/file"), None);
        assert_eq!(link_page_referer("not a url"), None);
    }
}
