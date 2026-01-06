//! This mod is exported only as a utility, do not consider it a part of the true public api of the
//! lambda_grpc_web crate. The purpose is to help diagnose issue with aws lambda interop by logging
//! the low level messages sent on the wire.
//! 
//! Define it as a tower layer either before or after the grpc web layers to make sense of the raw
//! lambda request/response or how the grpc layer was interpreted.

use bytes::Bytes;
use http::{Request, Response};
use http_body::{Body as HttpBody, Frame};
use http_body_util::BodyExt;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Layer;
use tower::Service;

struct LoggingBody<B> {
    inner: B,
    frame_count: usize,
}

impl<B> HttpBody for LoggingBody<B>
where
    B: HttpBody<Data = Bytes>,
{
    type Data = Bytes;
    type Error = B::Error;


    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        // SAFETY: We're not moving out of the pinned fields
        let this = unsafe { self.as_mut().get_unchecked_mut() };
        let inner = unsafe { Pin::new_unchecked(&mut this.inner) };
        let result = inner.poll_frame(cx);

        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                this.frame_count += 1;
                if let Some(data) = frame.data_ref() {
                    eprintln!("Frame {}: DATA {} bytes: {:?}", this.frame_count, data.len(), data);
                } else if let Some(trailers) = frame.trailers_ref() {
                    eprintln!("Frame {}: TRAILERS {:?}", this.frame_count, trailers);
                } else {
                    eprintln!("Frame {}: OTHER", this.frame_count);
                }
            }
            Poll::Ready(None) => {
                eprintln!("Body stream ended after {} frames", this.frame_count);
            }
            Poll::Ready(Some(Err(_))) => {
                eprintln!("Frame error");
            }
            Poll::Pending => {}
        }

        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

#[derive(Clone)]
pub struct WireLogLayer;

impl<S> Layer<S> for WireLogLayer {
    type Service = WireLogService<S>;

    fn layer(&self, service: S) -> Self::Service {
        WireLogService { inner: service }
    }
}

#[derive(Clone)]
pub struct WireLogService<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for WireLogService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Future: Send + 'static,
    ResBody: HttpBody<Data = Bytes> + Send + 'static,
    ResBody::Error: std::error::Error + Send + Sync + 'static,
{
    type Response = Response<http_body_util::combinators::UnsyncBoxBody<Bytes, ResBody::Error>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let fut = self.inner.call(req);
        Box::pin(async move {
            let res = fut.await?;
            let (parts, body) = res.into_parts();

            eprintln!("=== Response ===");
            eprintln!("Status: {:?}", parts.status);
            eprintln!("Headers: {:?}", parts.headers);

            let logged_body = LoggingBody { inner: body, frame_count: 0 };
            let boxed_body = logged_body.boxed_unsync();

            Ok(Response::from_parts(parts, boxed_body))
        })
    }
}
