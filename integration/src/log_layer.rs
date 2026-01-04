use std::task::{Context, Poll};

use http::{Request, Response};
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct LogServiceNameLayer;

impl<S> Layer<S> for LogServiceNameLayer {
    type Service = LogServiceNameService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LogServiceNameService { inner }
    }
}

#[derive(Clone)]
pub struct LogServiceNameService<S> {
    inner: S,
}

fn grpc_service_method<B>(req: &Request<B>) -> Option<(&str, &str)> {
    let path = req.uri().path().strip_prefix('/')?;
    let (service, method) = path.split_once('/')?;
    Some((service, method))
}

impl<S, B> Service<Request<B>> for LogServiceNameService<S>
where
    S: Service<Request<B>, Response = Response<B>, Error = std::convert::Infallible>,
{
    type Response = Response<B>;
    type Error = std::convert::Infallible;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<B>) -> Self::Future {
        if let Some((service, method)) = grpc_service_method(&req) {
            println!("gRPC request: service={}, method={}", service, method);
        } else {
            panic!("Missing service method")
        }

        self.inner.call(req)
    }
}
