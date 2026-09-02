//! Sandboxed file delivery over HTTP.
//!
//! Briefcase relays object bytes itself so a permanent URL never becomes a
//! bearer capability. Every response is therefore hardened for in-place
//! rendering of untrusted content: it is sandboxed by Content-Security-Policy,
//! never sniffed, never cached, and never embeddable by another origin.

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::Response,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use crate::{
    application::{
        content::{ContentDelivery, ContentIntent},
        ports::{ByteRange, RangeRequest},
    },
    error::AppError,
};

/// Value that isolates rendered content from the application and the network.
const SANDBOX_POLICY: HeaderValue =
    HeaderValue::from_static("sandbox; default-src 'none'; frame-ancestors 'none'");
const NO_SNIFF: HeaderValue = HeaderValue::from_static("nosniff");
const NO_REFERRER: HeaderValue = HeaderValue::from_static("no-referrer");
const PRIVATE_NO_STORE: HeaderValue = HeaderValue::from_static("private, no-store");
const SAME_ORIGIN_RESOURCE: HeaderValue = HeaderValue::from_static("same-origin");
const BYTES_UNIT: HeaderValue = HeaderValue::from_static("bytes");
const OCTET_STREAM: HeaderValue = HeaderValue::from_static("application/octet-stream");
const CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");
const MAXIMUM_RANGE_HEADER_BYTES: usize = 128;

/// Parses the single supported byte-range form of the `Range` header.
///
/// A malformed or multi-range header is ignored rather than rejected: the
/// caller then receives the complete file, which the HTTP specification
/// explicitly permits and which keeps a strict media player working.
///
/// # Errors
///
/// Returns an error only when the header itself is duplicated or unreadable.
pub(crate) fn requested_range(
    headers: &axum::http::HeaderMap,
) -> Result<Option<RangeRequest>, AppError> {
    let mut values = headers.get_all(header::RANGE).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(AppError::bad_request("duplicate_range_header"));
    }
    let Ok(value) = value.to_str() else {
        return Ok(None);
    };
    if value.len() > MAXIMUM_RANGE_HEADER_BYTES {
        return Ok(None);
    }
    Ok(parse_single_range(value))
}

fn parse_single_range(value: &str) -> Option<RangeRequest> {
    let specifier = value.trim().strip_prefix("bytes=")?.trim();
    if specifier.contains(',') {
        return None;
    }
    let (start, end) = specifier.split_once('-')?;
    let (start, end) = (start.trim(), end.trim());
    match (start.parse::<u64>().ok(), end.parse::<u64>().ok()) {
        (Some(start), Some(end)) if start <= end => Some(RangeRequest::Between(start, end)),
        (Some(start), None) if end.is_empty() => Some(RangeRequest::From(start)),
        (None, Some(length)) if start.is_empty() => Some(RangeRequest::Last(length)),
        _ => None,
    }
}

/// Builds the hardened streaming response for an authorized read.
///
/// # Errors
///
/// Returns an internal error when a derived header value cannot be encoded.
pub(crate) fn response(
    delivery: ContentDelivery,
    intent: ContentIntent,
) -> Result<Response, AppError> {
    let served_length = delivery
        .range
        .map_or(delivery.total_size, ByteRange::length);
    let content_type = match intent {
        // A download is never interpreted, so it is served as opaque bytes.
        ContentIntent::Download => OCTET_STREAM,
        ContentIntent::Render => header_value(&delivery.content_type)?,
    };
    let status = if delivery.range.is_some() {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };

    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, served_length)
        .header(header::ACCEPT_RANGES, BYTES_UNIT)
        .header(header::CONTENT_DISPOSITION, disposition(&delivery, intent)?)
        .header(header::CONTENT_SECURITY_POLICY, SANDBOX_POLICY)
        .header(header::X_CONTENT_TYPE_OPTIONS, NO_SNIFF)
        .header(header::REFERRER_POLICY, NO_REFERRER)
        .header(header::CACHE_CONTROL, PRIVATE_NO_STORE)
        .header(CROSS_ORIGIN_RESOURCE_POLICY, SAME_ORIGIN_RESOURCE)
        .body(Body::from_stream(delivery.body))
        .map_err(|_| AppError::Internal {
            category: "content_response",
        })?;

    if let Some(range) = delivery.range {
        let value = header_value(&format!(
            "bytes {}-{}/{}",
            range.start, range.end, delivery.total_size
        ))?;
        response.headers_mut().insert(header::CONTENT_RANGE, value);
    }
    if let Some(etag) = delivery.etag.as_deref()
        && let Ok(value) = HeaderValue::from_str(etag)
    {
        response.headers_mut().insert(header::ETAG, value);
    }
    Ok(response)
}

fn disposition(delivery: &ContentDelivery, intent: ContentIntent) -> Result<HeaderValue, AppError> {
    let kind = match intent {
        ContentIntent::Render => "inline",
        ContentIntent::Download => "attachment",
    };
    // The quoted form stays ASCII-safe for legacy clients; `filename*` carries
    // the exact UTF-8 name for everyone else.
    let ascii_name = delivery
        .filename
        .chars()
        .map(|character| {
            if character.is_ascii_graphic() && !matches!(character, '"' | '\\') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let encoded_name = utf8_percent_encode(&delivery.filename, NON_ALPHANUMERIC);
    header_value(&format!(
        "{kind}; filename=\"{ascii_name}\"; filename*=UTF-8''{encoded_name}"
    ))
}

fn header_value(value: &str) -> Result<HeaderValue, AppError> {
    HeaderValue::from_str(value).map_err(|_| AppError::Internal {
        category: "content_response_header",
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use crate::application::ports::RangeRequest;

    use super::{parse_single_range, requested_range};

    #[test]
    fn every_supported_range_form_is_parsed() {
        assert_eq!(
            parse_single_range("bytes=0-1023"),
            Some(RangeRequest::Between(0, 1023))
        );
        assert_eq!(
            parse_single_range("bytes=512-"),
            Some(RangeRequest::From(512))
        );
        assert_eq!(
            parse_single_range("bytes=-500"),
            Some(RangeRequest::Last(500))
        );
    }

    #[test]
    fn unsupported_or_inverted_ranges_serve_the_whole_file() {
        assert_eq!(parse_single_range("bytes=100-20"), None);
        assert_eq!(parse_single_range("bytes=0-10,20-30"), None);
        assert_eq!(parse_single_range("items=0-10"), None);
        assert_eq!(parse_single_range("bytes=-"), None);
    }

    #[test]
    fn a_duplicated_range_header_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.append(header::RANGE, HeaderValue::from_static("bytes=0-1"));
        headers.append(header::RANGE, HeaderValue::from_static("bytes=2-3"));
        assert!(requested_range(&headers).is_err());
    }
}
