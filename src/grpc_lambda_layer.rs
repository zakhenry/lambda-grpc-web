use lambda_http::{Service};
use lambda_http::Request as LambdaRequest;
use lambda_runtime::streaming::Body;
use lambda_runtime::Error;
use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use http::{Request, Response};
use tonic::body::Body as TonicBody;
use tower::util::BoxService;
use tower::{Layer, MakeService, ServiceExt};

/// Adapts a Tower/Axum/Tonic service to the Lambda streaming HTTP runtime.
pub struct GrpcLambdaService {
    inner: BoxService<
        Request<TonicBody>,
        Response<TonicBody>,
        Infallible,
    >,
}

impl GrpcLambdaService {
    pub fn new(
        inner: BoxService<
            Request<TonicBody>,
            Response<TonicBody>,
            Infallible,
        >,
    ) -> Self {
        Self { inner }
    }
}

impl Service<LambdaRequest> for GrpcLambdaService {
    type Response = Response<Body>;
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
        // Convert Lambda request body → AxumBody
        let req = req.map(TonicBody::new);

        let fut = self.inner.call(req);

        Box::pin(async move {
            let res = fut.await.expect("Infallible");

            let (parts, body) = res.into_parts();

            // Convert AxumBody → Lambda streaming body
            Ok(Response::from_parts(parts, Body::new(body)))
        })
    }
}
