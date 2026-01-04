//! This is an example of a generic tower middleware that copies http header value for 
//! key `echo-meta` into `meta-value` if present. It is used by the integration test to verify
//! standard tower middleware functions as expected

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::{Request, Response};
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct MetaEchoLayer;

impl<S> Layer<S> for MetaEchoLayer {
    type Service = MetaEchoService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        MetaEchoService { inner }
    }
}

#[derive(Clone)]
pub struct MetaEchoService<S> {
    inner: S,
}

impl<S, B> Service<Request<B>> for MetaEchoService<S>
where
    S: Service<Request<B>, Response = Response<B>, Error = std::convert::Infallible>,
    S::Future: Send + 'static,
    B: 'static,
{
    type Response = Response<B>;
    type Error = std::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        let echo_meta = req.headers().get("echo-meta").cloned();

        let fut = self.inner.call(req);

        Box::pin(async move {
            let mut res = fut.await.expect("infallible");
            if let Some(echo_meta) = echo_meta {
                res.headers_mut().insert("meta-value", echo_meta);
            }
            Ok(res)
        })
    }
}
