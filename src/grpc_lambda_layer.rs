use http::{Request, Response};
use lambda_http::{Request as LambdaRequest};
use lambda_runtime::streaming::Body as LambdaBody;
use lambda_runtime::Error;
use std::{
    convert::Infallible,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tonic::service::AxumBody;
use tower::Service;

pub struct GrpcLambdaService<S> {
    inner: S,
}

impl<S> GrpcLambdaService<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Service<LambdaRequest> for GrpcLambdaService<S>
where
    S: Service<Request<AxumBody>, Response = Response<AxumBody>, Error = Infallible>
    + Send
    + 'static,
    S::Future: Send + 'static,
{
    type Response = Response<LambdaBody>;
    type Error = Error;

    type Future =
    Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Error::from)
    }

    fn call(&mut self, req: LambdaRequest) -> Self::Future {
        let req = req.map(AxumBody::new);
        let fut = self.inner.call(req);

        Box::pin(async move {
            let res = fut.await.expect("infallible");

            let (parts, body) = res.into_parts();
            Ok(Response::from_parts(parts, LambdaBody::new(body)))
        })
    }
}
