//! API Gateway HTTP API (v2) transport.
//!
//! `lambda_http` already knows this envelope, so this module is just the glue between its
//! `Service<lambda_http::Request>` shape and the grpc-web service stack. The response is streamed,
//! which requires the function to be configured with the `RESPONSE_STREAM` invoke mode.

use bytes::Bytes;
use http::{Request, Response};
use lambda_runtime::Error;
use std::convert::Infallible;
use tonic::body::Body;
use tower::Service;

pub(crate) async fn run<S, ResBody>(svc: S) -> Result<(), Error>
where
    S: Service<Request<Body>, Response = Response<ResBody>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    ResBody: http_body::Body<Data = Bytes> + Send + 'static,
    ResBody::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let handler = tower::service_fn(move |req: lambda_http::Request| {
        let mut svc = svc.clone();
        async move {
            let req = req.map(|body| Body::new(tonic::service::AxumBody::new(body)));
            let res = match svc.call(req).await {
                Ok(res) => res,
                Err(infallible) => match infallible {},
            };
            let (parts, body) = res.into_parts();
            let body = lambda_runtime::streaming::Body::new(body);
            Ok::<_, Error>(Response::from_parts(parts, body))
        }
    });

    lambda_http::run_with_streaming_response(handler).await
}
