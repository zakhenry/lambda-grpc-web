//! Envoy `aws_lambda` http filter transport.
//!
//! Envoy's [AWS Lambda http filter] invokes a function directly with its own JSON envelope, which
//! is not the API Gateway shape. Only `payload_passthrough: false` is supported - with passthrough
//! enabled the function receives the raw body with no path and no `content-type`, which leaves
//! nothing to route a gRPC method on.
//!
//! The filter uses the buffered `Invoke` API, so the function must be configured with the
//! `BUFFERED` invoke mode and a server streaming response is collected in full before it is
//! returned.
//!
//! [AWS Lambda http filter]: https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/aws_lambda_filter.html

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use http::header::{CONTENT_TYPE, SET_COOKIE};
use http::uri::PathAndQuery;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri};
use http_body_util::{BodyExt, Full};
use lambda_runtime::tracing::log::{debug, error, warn};
use lambda_runtime::{Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::convert::Infallible;
use std::fmt::{Display, Formatter};
use tonic::body::Body;
use tower::Service;

/// Size of a grpc-web frame header: one flag byte plus a four byte big endian payload length.
const FRAME_HEADER_SIZE: usize = 5;
/// Most significant bit of a grpc-web frame flag byte, set on trailer frames.
const TRAILER_BIT: u8 = 0x80;
/// Default `content-type` for a constructed request envelope.
const GRPC_WEB_PROTO: &str = "application/grpc-web+proto";

/// Request envelope emitted by the filter with `payload_passthrough: false`.
///
/// Constructing one is useful for testing a function without an Envoy in front of it, or for
/// producing an event file for `cargo lambda invoke --data-file`:
///
/// ```
/// # use lambda_grpc_web::EnvoyRequest;
/// // frames are the raw grpc-web body - the base64 Envoy applies is handled on serialisation
/// let frames = [0, 0, 0, 0, 5, 0x0a, 0x03, b'a', b'b', b'c'];
///
/// let event = EnvoyRequest::new("/helloworld.Greeter/SayHello", frames)
///     .with_header("grpc-metadata-tenant", "acme");
///
/// println!("{}", serde_json::to_string_pretty(&event).unwrap());
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct EnvoyRequest {
    /// Request target, *including* the query string - i.e. `/pkg.Service/Method?a=b`.
    raw_path: String,
    method: String,
    /// A flat map: Envoy coalesces repeated headers into a single comma separated value before it
    /// builds the envelope.
    #[serde(default)]
    headers: HashMap<String, String>,
    /// Envoy also breaks the query string out into a map of its own. It is redundant here because
    /// `raw_path` already carries the query string verbatim, and gRPC does not use query strings
    /// in the first place. Declared so the envelope is documented in full, deliberately unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    query_string_parameters: Option<HashMap<String, String>>,
    /// Base64 encoded unless `content-type` is on Envoy's text-ish allowlist (`text/*`,
    /// `application/json`, `application/xml`, `application/javascript`). No grpc-web content type
    /// is on that list, so in practice this is always base64 - but the flag is what decides.
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    is_base64_encoded: bool,
}

impl EnvoyRequest {
    /// Build the envelope Envoy would send for a grpc-web `POST` to `raw_path`.
    ///
    /// `body` is the *unencoded* grpc-web body - the base64 Envoy applies to any content type
    /// outside its text-ish allowlist is done here, so callers pass frames rather than text.
    ///
    /// `raw_path` may carry a query string; Envoy sends it as part of the path.
    pub fn new(raw_path: impl Into<String>, body: impl AsRef<[u8]>) -> Self {
        Self {
            raw_path: raw_path.into(),
            method: Method::POST.to_string(),
            headers: HashMap::from([(CONTENT_TYPE.as_str().to_owned(), GRPC_WEB_PROTO.to_owned())]),
            query_string_parameters: None,
            body: Some(BASE64.encode(body)),
            is_base64_encoded: true,
        }
    }

    /// Override the `POST` set by [`EnvoyRequest::new`].
    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = method.into();
        self
    }

    /// Add a header, replacing any existing entry of the same name.
    ///
    /// Envoy has already coalesced repeated headers into a single comma separated value by the
    /// time it builds the envelope, so there is deliberately no way to add a second value.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// Response envelope the filter expects back.
///
/// Note the field names are snake_case, unlike API Gateway's `statusCode` / `isBase64Encoded`.
/// Envoy fails the response and increments `server_error` if it cannot parse this, so every exit
/// path has to produce one.
#[derive(Debug, Serialize, Deserialize)]
pub struct EnvoyResponse {
    status_code: u16,
    #[serde(default)]
    headers: HashMap<String, String>,
    /// `Set-Cookie` cannot be coalesced into a single header value, so Envoy carries it here
    /// instead, one entry per header. gRPC does not set cookies in practice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cookies: Vec<String>,
    body: String,
    #[serde(default)]
    is_base64_encoded: bool,
}

impl EnvoyResponse {
    /// A minimal, always well formed envelope used for failures that never reach the service.
    fn plain_text(status: StatusCode, message: &str) -> Self {
        Self {
            status_code: status.as_u16(),
            headers: HashMap::from([(CONTENT_TYPE.as_str().to_owned(), "text/plain".to_owned())]),
            cookies: Vec::new(),
            body: BASE64.encode(message),
            is_base64_encoded: true,
        }
    }

    /// The http status. Note a gRPC error is still a `200` - the failure is in the trailer frame
    /// inside [`EnvoyResponse::body`].
    pub fn status_code(&self) -> u16 {
        self.status_code
    }

    pub fn headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// `Set-Cookie` values, which Envoy carries outside the header map.
    pub fn cookies(&self) -> &[String] {
        &self.cookies
    }

    /// The grpc-web body, with Envoy's base64 layer removed if it was applied.
    pub fn body(&self) -> Result<Vec<u8>, base64::DecodeError> {
        if self.is_base64_encoded {
            BASE64.decode(&self.body)
        } else {
            Ok(self.body.clone().into_bytes())
        }
    }
}

/// A request envelope that cannot be turned into an `http::Request`.
#[derive(Debug)]
pub(crate) enum RequestError {
    Method(String),
    RawPath(String),
    Body(base64::DecodeError),
}

impl Display for RequestError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestError::Method(method) => write!(f, "invalid request method `{method}`"),
            RequestError::RawPath(raw_path) => write!(f, "invalid request path `{raw_path}`"),
            RequestError::Body(err) => write!(f, "request body is not valid base64: {err}"),
        }
    }
}

impl RequestError {
    fn into_envoy_response(self) -> EnvoyResponse {
        EnvoyResponse::plain_text(StatusCode::BAD_REQUEST, &self.to_string())
    }
}

/// Unpack the Envoy envelope into the request the grpc-web stack expects.
///
/// A missing or non grpc-web `content-type` is deliberately *not* rejected here - `tonic-web`
/// already answers those with `400 Bad Request`, which serialises into a perfectly good envelope.
fn to_http_request(envelope: EnvoyRequest) -> Result<Request<Body>, RequestError> {
    let method = Method::from_bytes(envelope.method.as_bytes())
        .map_err(|_| RequestError::Method(envelope.method.clone()))?;

    // `raw_path` is an origin-form target, never an absolute URI. Parsing it as a whole `Uri`
    // would let a value like `pkg.Service` be read as an authority with an empty path, so parse
    // the narrower type and require the leading slash up front.
    if !envelope.raw_path.starts_with('/') {
        return Err(RequestError::RawPath(envelope.raw_path));
    }
    let path_and_query = PathAndQuery::try_from(envelope.raw_path.as_str())
        .map_err(|_| RequestError::RawPath(envelope.raw_path.clone()))?;
    let uri = Uri::builder()
        .path_and_query(path_and_query)
        .build()
        .map_err(|_| RequestError::RawPath(envelope.raw_path))?;

    let body = match envelope.body {
        Some(body) if envelope.is_base64_encoded => {
            Bytes::from(BASE64.decode(body).map_err(RequestError::Body)?)
        }
        // A `application/grpc-web-text` body is base64 twice over: the client encodes the frames
        // and Envoy encodes that text again. Only Envoy's layer is peeled here, `tonic-web` does
        // the inner decode based on `content-type`.
        Some(body) => Bytes::from(body),
        None => Bytes::new(),
    };

    let mut request = Request::new(Body::new(Full::new(body)));
    *request.method_mut() = method;
    *request.uri_mut() = uri;

    let headers = request.headers_mut();
    for (name, value) in envelope.headers {
        // Envoy can emit names `http` refuses, most obviously HTTP/2 pseudo-headers such as
        // `:authority`. An unrepresentable header is not worth failing an invocation over.
        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            debug!("skipping request header with invalid name `{name}`");
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            debug!("skipping request header `{name}` with invalid value");
            continue;
        };
        // Repeated headers arrive already coalesced into one comma separated value. Splitting
        // them back apart would corrupt any value that legitimately contains a comma, so the
        // coalesced value is inserted as-is.
        headers.insert(name, value);
    }

    Ok(request)
}

/// Buffer the grpc-web response and pack it into the envelope.
///
/// `tonic-web` writes gRPC trailers as a trailer frame *inside* the body, so buffering the body is
/// enough to carry `grpc-status` back - there are no http trailers left to lose.
async fn from_http_response<B>(response: Response<B>) -> EnvoyResponse
where
    B: http_body::Body<Data = Bytes>,
    B::Error: Display,
{
    let (parts, body) = response.into_parts();

    let body = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            error!("failed to buffer grpc-web response body: {err}");
            return EnvoyResponse::plain_text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to buffer grpc-web response body",
            );
        }
    };

    warn_if_buffered_stream(&parts.headers, &body);

    let mut headers: HashMap<String, String> = HashMap::with_capacity(parts.headers.len());
    let mut cookies = Vec::new();

    for (name, value) in parts.headers.iter() {
        let Ok(value) = value.to_str() else {
            debug!("skipping response header `{name}` with non-ascii value");
            continue;
        };

        if name == SET_COOKIE {
            cookies.push(value.to_owned());
            continue;
        }

        match headers.entry(name.as_str().to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(value.to_owned());
            }
            // The envelope only has room for one value per name, so repeated headers are
            // coalesced the same way Envoy coalesces them on the way in.
            Entry::Occupied(mut entry) => {
                let coalesced = format!("{}, {}", entry.get(), value);
                entry.insert(coalesced);
            }
        }
    }

    EnvoyResponse {
        status_code: parts.status.as_u16(),
        headers,
        cookies,
        // Always base64 - the body is binary grpc-web frames, and for the `-text` variants this
        // is the outer layer that mirrors the encode Envoy applied to the request.
        body: BASE64.encode(&body),
        is_base64_encoded: true,
    }
}

/// Warn when a buffered response turned out to be a stream of more than one message.
fn warn_if_buffered_stream(headers: &HeaderMap, body: &[u8]) {
    // A `-text` body is base64 text rather than raw frames, so the frame walk below would be
    // reading nonsense. Streaming over grpc-web-text is rare enough not to be worth the decode.
    if is_grpc_web_text(headers) {
        return;
    }

    let messages = count_data_messages(body);
    if messages > 1 {
        warn!(
            "buffered {messages} grpc-web messages into a single response: Envoy's aws_lambda \
             filter invokes the function with the buffered `Invoke` api, so a server streaming \
             response is not delivered incrementally and the whole stream must fit within the \
             lambda response payload limit"
        );
    }
}

fn is_grpc_web_text(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|content_type| content_type.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("application/grpc-web-text"))
}

/// Count the grpc-web *data* messages in a buffered grpc-web body.
///
/// Frames are counted rather than http body frames because `tonic-web` can split a single message
/// across several body frames, which would make a frame based count fire spuriously.
///
/// A truncated trailing frame stops the walk rather than being counted, so a partial body cannot
/// inflate the result.
fn count_data_messages(body: &[u8]) -> usize {
    let mut cursor = 0;
    let mut messages = 0;

    while let Some(header) = body.get(cursor..cursor + FRAME_HEADER_SIZE) {
        let payload_len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        // A 32 bit length field can overflow a 32 bit `usize`, and a wrapped cursor would walk
        // backwards forever.
        let Some(next) = cursor
            .checked_add(FRAME_HEADER_SIZE)
            .and_then(|end| end.checked_add(payload_len))
            .filter(|next| *next <= body.len())
        else {
            break;
        };

        if header[0] & TRAILER_BIT == 0 {
            messages += 1;
        }
        cursor = next;
    }

    messages
}

pub(crate) async fn run<S, ResBody>(svc: S) -> Result<(), Error>
where
    S: Service<Request<Body>, Response = Response<ResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    ResBody: http_body::Body<Data = Bytes> + Send + 'static,
    ResBody::Error: Display,
{
    let handler = lambda_runtime::service_fn(move |event: LambdaEvent<EnvoyRequest>| {
        let mut svc = svc.clone();
        async move {
            let (envelope, context) = event.into_parts();

            let mut request = match to_http_request(envelope) {
                Ok(request) => request,
                Err(err) => {
                    warn!("rejecting Envoy request envelope: {err}");
                    return Ok(err.into_envoy_response());
                }
            };

            // Mirrors what `lambda_http` does for the API Gateway transport so that layers and
            // services can read the invocation deadline out of the request extensions.
            request.extensions_mut().insert(context);

            let response = match svc.call(request).await {
                Ok(response) => response,
                Err(infallible) => match infallible {},
            };

            Ok::<_, Error>(from_http_response(response).await)
        }
    });

    lambda_runtime::run(handler).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use lambda_runtime::tracing::log;
    use serde_json::{Value, json};
    use std::sync::{Mutex, Once};

    /// A grpc-web data frame carrying `HelloRequest { name: "abc" }`.
    const REQUEST_FRAME: &[u8] = &[0, 0, 0, 0, 5, 0x0a, 0x03, b'a', b'b', b'c'];
    /// A grpc-web data frame carrying `HelloReply { message: "hi" }`.
    const RESPONSE_FRAME: &[u8] = &[0, 0, 0, 0, 4, 0x0a, 0x02, b'h', b'i'];

    fn trailer_frame(trailers: &str) -> Vec<u8> {
        let mut frame = vec![TRAILER_BIT];
        frame.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
        frame.extend_from_slice(trailers.as_bytes());
        frame
    }

    /// Deserialise an envelope from raw JSON, for the cases that are *about* the wire format -
    /// absent fields, names `http` rejects - rather than about the conversion.
    fn envelope(json: Value) -> EnvoyRequest {
        serde_json::from_value(json).expect("envelope should deserialise")
    }

    async fn collect(request: Request<Body>) -> Bytes {
        request
            .into_body()
            .collect()
            .await
            .expect("body should collect")
            .to_bytes()
    }

    fn grpc_web_response(content_type: &str, body: Vec<u8>) -> Response<Full<Bytes>> {
        Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, content_type)
            .body(Full::new(Bytes::from(body)))
            .expect("response should build")
    }

    /// Three data frames and a trailer frame - what a server streaming rpc buffers down to.
    fn server_stream_body() -> Vec<u8> {
        let mut body = Vec::new();
        for _ in 0..3 {
            body.extend_from_slice(RESPONSE_FRAME);
        }
        body.extend_from_slice(&trailer_frame("grpc-status:0\r\n"));
        body
    }

    /// Records emitted through the `log` facade the crate warns on, so tests can assert the
    /// warning actually fires rather than just that the condition holds.
    static LOG_RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct CaptureLogger;

    impl log::Log for CaptureLogger {
        fn enabled(&self, _metadata: &log::Metadata) -> bool {
            true
        }

        fn log(&self, record: &log::Record) {
            if let Ok(mut records) = LOG_RECORDS.lock() {
                records.push(format!("[{}] {}", record.level(), record.args()));
            }
        }

        fn flush(&self) {}
    }

    /// Install the capture logger and return a marker for [`logs_since`].
    ///
    /// The buffer is never cleared and the logger is global, so a test running in parallel can
    /// append to it too - assertions match on content rather than on the exact set.
    fn start_capturing_logs() -> usize {
        static INSTALLED: Once = Once::new();
        static CAPTURE_LOGGER: CaptureLogger = CaptureLogger;
        INSTALLED.call_once(|| {
            // another test binary component may have got there first, which is equally fine
            let _ = log::set_logger(&CAPTURE_LOGGER);
            log::set_max_level(log::LevelFilter::Trace);
        });

        LOG_RECORDS.lock().expect("log buffer should lock").len()
    }

    fn logs_since(marker: usize) -> Vec<String> {
        LOG_RECORDS.lock().expect("log buffer should lock")[marker..].to_vec()
    }

    /// Serialise a response and compare as JSON, so key ordering cannot cause flakes.
    async fn envoy_json<B>(response: Response<B>) -> Value
    where
        B: http_body::Body<Data = Bytes>,
        B::Error: Display,
    {
        serde_json::to_value(from_http_response(response).await).expect("envelope should serialise")
    }

    #[test]
    fn constructed_request_serialises_to_envoys_wire_format() {
        let envelope = EnvoyRequest::new("/helloworld.Greeter/SayHello", REQUEST_FRAME)
            .with_header("grpc-metadata-tenant", "acme");

        assert_eq!(
            serde_json::to_value(&envelope).expect("envelope should serialise"),
            json!({
                "raw_path": "/helloworld.Greeter/SayHello",
                "method": "POST",
                "headers": {
                    "content-type": "application/grpc-web+proto",
                    "grpc-metadata-tenant": "acme",
                },
                "body": "AAAAAAUKA2FiYw==",
                "is_base64_encoded": true,
            })
        );
    }

    #[tokio::test]
    async fn unary_request_maps_method_uri_headers_and_body() {
        let request = to_http_request(
            EnvoyRequest::new("/helloworld.Greeter/SayHello", REQUEST_FRAME)
                .with_header("grpc-metadata-tenant", "acme"),
        )
        .expect("envelope should convert");

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri().path(), "/helloworld.Greeter/SayHello");
        assert_eq!(request.headers().get(CONTENT_TYPE).unwrap(), GRPC_WEB_PROTO);
        assert_eq!(
            request.headers().get("grpc-metadata-tenant").unwrap(),
            "acme"
        );
        assert_eq!(collect(request).await, Bytes::from(REQUEST_FRAME));
    }

    #[tokio::test]
    async fn plaintext_request_body_is_passed_through_verbatim() {
        let request = to_http_request(envelope(json!({
            "raw_path": "/helloworld.Greeter/SayHello",
            "method": "POST",
            "headers": { "content-type": "application/grpc-web+proto" },
            "body": "hello",
            "is_base64_encoded": false,
        })))
        .expect("envelope should convert");

        assert_eq!(collect(request).await, Bytes::from_static(b"hello"));
    }

    #[tokio::test]
    async fn absent_optional_fields_default() {
        // no `body`, no `query_string_parameters`, no `is_base64_encoded`
        let request = to_http_request(envelope(json!({
            "raw_path": "/helloworld.Greeter/SayHello",
            "method": "POST",
            "headers": { "content-type": "application/grpc-web+proto" },
        })))
        .expect("envelope should convert");

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri().path(), "/helloworld.Greeter/SayHello");
        assert!(collect(request).await.is_empty());
    }

    #[test]
    fn query_string_parameters_are_accepted_and_ignored() {
        // `raw_path` is the authority on the query string, so the redundant map must not upset
        // deserialisation
        let request = to_http_request(envelope(json!({
            "raw_path": "/helloworld.Greeter/SayHello?trace=1",
            "method": "POST",
            "headers": { "content-type": "application/grpc-web+proto" },
            "query_string_parameters": { "trace": "1" },
        })))
        .expect("envelope should convert");

        assert_eq!(request.uri().query(), Some("trace=1"));
    }

    #[test]
    fn raw_path_query_string_is_preserved() {
        let request = to_http_request(EnvoyRequest::new(
            "/helloworld.Greeter/SayHello?trace=1&debug=true",
            REQUEST_FRAME,
        ))
        .expect("envelope should convert");

        assert_eq!(request.uri().path(), "/helloworld.Greeter/SayHello");
        assert_eq!(request.uri().query(), Some("trace=1&debug=true"));
        assert_eq!(
            request.uri().path_and_query().map(|pq| pq.as_str()),
            Some("/helloworld.Greeter/SayHello?trace=1&debug=true")
        );
    }

    #[test]
    fn pseudo_header_is_skipped_rather_than_fatal() {
        let request = to_http_request(
            // `EnvoyRequest::with_header` would take it too, but going through JSON makes it
            // obvious this is a name arriving off the wire
            envelope(json!({
                "raw_path": "/helloworld.Greeter/SayHello",
                "method": "POST",
                "headers": {
                    ":authority": "grpc.example.com",
                    "content-type": "application/grpc-web+proto",
                },
            })),
        )
        .expect("envelope should convert");

        assert_eq!(request.headers().len(), 1);
        assert_eq!(request.headers().get(CONTENT_TYPE).unwrap(), GRPC_WEB_PROTO);
    }

    #[tokio::test]
    async fn grpc_web_text_request_decodes_one_layer_only() {
        // a browser client base64s the frames itself, then Envoy base64s that text again
        let client_body = BASE64.encode(REQUEST_FRAME);

        let request = to_http_request(
            EnvoyRequest::new("/helloworld.Greeter/SayHello", &client_body)
                .with_header("content-type", "application/grpc-web-text"),
        )
        .expect("envelope should convert");

        // Envoy's layer is peeled, leaving the client's base64 text for `tonic-web` to decode -
        // decoding both layers here would hand `tonic-web` raw frames it would then try to
        // base64 decode
        assert_eq!(collect(request).await, Bytes::from(client_body));
    }

    #[test]
    fn malformed_method_produces_an_error_envelope() {
        let err = to_http_request(
            EnvoyRequest::new("/helloworld.Greeter/SayHello", REQUEST_FRAME).with_method("GET /"),
        )
        .expect_err("invalid method should be rejected");

        let response = err.into_envoy_response();

        assert_eq!(response.status_code(), 400);
        assert_eq!(response.headers()["content-type"], "text/plain");
        assert!(response.cookies().is_empty());
        assert_eq!(
            String::from_utf8(response.body().expect("body should be base64"))
                .expect("message should be utf8"),
            "invalid request method `GET /`"
        );

        let envelope = serde_json::to_value(&response).expect("envelope should serialise");
        assert_eq!(envelope["status_code"], json!(400));
        assert!(envelope.get("cookies").is_none());
    }

    #[test]
    fn malformed_raw_path_produces_an_error_envelope() {
        let err = to_http_request(EnvoyRequest::new(
            "helloworld.Greeter/SayHello",
            REQUEST_FRAME,
        ))
        .expect_err("invalid raw_path should be rejected");

        let response = err.into_envoy_response();

        assert_eq!(response.status_code(), 400);
        assert_eq!(
            String::from_utf8(response.body().expect("body should be base64"))
                .expect("message should be utf8"),
            "invalid request path `helloworld.Greeter/SayHello`"
        );
    }

    #[tokio::test]
    async fn unary_response_serialises_to_the_envelope() {
        let mut body = RESPONSE_FRAME.to_vec();
        body.extend_from_slice(&trailer_frame("grpc-status:0\r\n"));

        let envelope = envoy_json(grpc_web_response(GRPC_WEB_PROTO, body)).await;

        assert_eq!(
            envelope,
            json!({
                "status_code": 200,
                "headers": { "content-type": "application/grpc-web+proto" },
                "body": "AAAAAAQKAmhpgAAAAA9ncnBjLXN0YXR1czowDQo=",
                "is_base64_encoded": true,
            })
        );
        // the two things a mechanical port of the API Gateway envelope gets wrong
        assert!(envelope["status_code"].is_number());
        assert!(envelope.get("statusCode").is_none());
        assert!(envelope.get("isBase64Encoded").is_none());
        // `cookies` is skipped when empty
        assert!(envelope.get("cookies").is_none());
    }

    #[tokio::test]
    async fn set_cookie_headers_move_into_cookies() {
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, GRPC_WEB_PROTO)
            .header(SET_COOKIE, "a=1; HttpOnly")
            .header(SET_COOKIE, "b=2; Secure")
            .body(Full::new(Bytes::new()))
            .expect("response should build");

        let envelope = envoy_json(response).await;

        assert_eq!(
            envelope,
            json!({
                "status_code": 200,
                "headers": { "content-type": "application/grpc-web+proto" },
                "cookies": ["a=1; HttpOnly", "b=2; Secure"],
                "body": "",
                "is_base64_encoded": true,
            })
        );
        assert!(envelope["headers"].get("set-cookie").is_none());
    }

    #[tokio::test]
    async fn grpc_error_response_stays_http_200() {
        let body = trailer_frame("grpc-status:13\r\ngrpc-message:boom\r\n");

        let envelope = envoy_json(grpc_web_response(GRPC_WEB_PROTO, body)).await;

        assert_eq!(
            envelope,
            json!({
                "status_code": 200,
                "headers": { "content-type": "application/grpc-web+proto" },
                "body": "gAAAACNncnBjLXN0YXR1czoxMw0KZ3JwYy1tZXNzYWdlOmJvb20NCg==",
                "is_base64_encoded": true,
            })
        );
    }

    #[tokio::test]
    async fn grpc_web_text_response_re_encodes_symmetrically() {
        // `tonic-web` base64 encodes each frame individually for the `-text` variants, and the
        // envelope encode goes on top of that
        let text_body = format!(
            "{}{}",
            BASE64.encode(RESPONSE_FRAME),
            BASE64.encode(trailer_frame("grpc-status:0\r\n"))
        );

        let response =
            grpc_web_response("application/grpc-web-text", text_body.clone().into_bytes());
        let envelope: EnvoyResponse =
            serde_json::from_value(envoy_json(response).await).expect("envelope should round trip");

        assert_eq!(
            String::from_utf8(envelope.body().expect("body should be base64"))
                .expect("text should be utf8"),
            text_body
        );
    }

    #[tokio::test]
    async fn buffered_server_stream_warns_once() {
        let marker = start_capturing_logs();

        let envelope = envoy_json(grpc_web_response(GRPC_WEB_PROTO, server_stream_body())).await;

        // the response itself is unaffected - the warning is advisory, not an error
        assert_eq!(envelope["status_code"], json!(200));

        let warnings: Vec<_> = logs_since(marker)
            .into_iter()
            .filter(|record| record.contains("buffered 3 grpc-web messages"))
            .collect();

        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one buffered stream warning, got {warnings:?}"
        );
        assert!(
            warnings[0].starts_with("[WARN]"),
            "expected the buffered stream message at WARN, got {:?}",
            warnings[0]
        );
        assert!(
            warnings[0].contains("not delivered incrementally"),
            "expected the warning to explain the consequence, got {:?}",
            warnings[0]
        );
    }

    #[tokio::test]
    async fn unary_response_does_not_warn() {
        let marker = start_capturing_logs();

        let mut body = RESPONSE_FRAME.to_vec();
        body.extend_from_slice(&trailer_frame("grpc-status:0\r\n"));

        envoy_json(grpc_web_response(GRPC_WEB_PROTO, body)).await;

        assert!(
            !logs_since(marker)
                .iter()
                .any(|record| record.contains("grpc-web messages")),
            "a single message response should not warn, got {:?}",
            logs_since(marker)
        );
    }

    #[tokio::test]
    async fn grpc_web_text_response_does_not_warn() {
        let marker = start_capturing_logs();

        // the same three message stream, but base64 text rather than raw frames - walking it as
        // frames would read nonsense, so the count is deliberately skipped
        let text_body = BASE64.encode(server_stream_body()).into_bytes();

        envoy_json(grpc_web_response("application/grpc-web-text", text_body)).await;

        assert!(
            !logs_since(marker)
                .iter()
                .any(|record| record.contains("grpc-web messages")),
            "a grpc-web-text response should not be walked as frames, got {:?}",
            logs_since(marker)
        );
    }

    #[test]
    fn count_data_messages_over_an_empty_body() {
        assert_eq!(count_data_messages(&[]), 0);
    }

    #[test]
    fn count_data_messages_over_a_single_message() {
        assert_eq!(count_data_messages(RESPONSE_FRAME), 1);
    }

    #[test]
    fn count_data_messages_ignores_the_trailer_frame() {
        let mut body = RESPONSE_FRAME.to_vec();
        body.extend_from_slice(&trailer_frame("grpc-status:0\r\n"));

        assert_eq!(count_data_messages(&body), 1);
    }

    #[test]
    fn count_data_messages_over_a_stream() {
        assert_eq!(count_data_messages(&server_stream_body()), 3);
    }

    #[test]
    fn count_data_messages_stops_at_a_truncated_frame() {
        let mut body = RESPONSE_FRAME.to_vec();
        // a header promising more payload than is present
        body.extend_from_slice(&[0, 0, 0, 0, 9, 0x0a]);

        assert_eq!(count_data_messages(&body), 1);
    }
}
