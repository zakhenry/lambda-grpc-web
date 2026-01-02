mod deadline_layer;

use lambda_http::{Request, Response};
use lambda_runtime::{streaming::Body, Error, Service};
use std::any::Any;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tonic::service::{AxumBody, Routes};
use tonic::Status;
use tonic_web::GrpcWebLayer;
use tower::Layer;
use tower::ServiceExt;
use tower::util::BoxService;
use tower_http::catch_panic::CatchPanicLayer;
pub use lambda_runtime;
use crate::deadline_layer::LambdaDeadlineLayer;

#[derive(Clone, Copy, Debug, Default)]
pub struct GrpcLambdaLayer;

impl GrpcLambdaLayer {
    pub fn new() -> Self {
        Self
    }
}

pub struct GrpcLambdaService {
    router: BoxService<Request, Response<AxumBody>, Infallible>,
}

impl Layer<Routes> for GrpcLambdaLayer {
    type Service = GrpcLambdaService;

    fn layer(&self, routes: Routes) -> Self::Service {
        let router =
            routes
                .into_axum_router()
                .layer(GrpcWebLayer::new())
                .layer(CatchPanicLayer::custom(
                    |err: Box<dyn Any + Send + 'static>| {
                        let details = if let Some(s) = err.downcast_ref::<String>() {
                            s.clone()
                        } else if let Some(s) = err.downcast_ref::<&str>() {
                            s.to_string()
                        } else {
                            "Unknown panic message".to_string()
                        };

                        Status::internal(details).into_http::<Body>()
                    },
                ))
                .layer(LambdaDeadlineLayer::new(Duration::from_millis(500)));

        let router = router.boxed();

        GrpcLambdaService { router }
    }
}

pub async fn run(routes: Routes) -> Result<(), Error> {
    let handler = GrpcLambdaLayer::new().layer(routes);
    lambda_http::run_with_streaming_response(handler).await
}

impl Service<Request> for GrpcLambdaService {
    type Response = Response<Body>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.router.poll_ready(cx).map_err(Error::from)
    }

    fn call(&mut self, req: Request) -> Self::Future {

        let fut = self.router.call(req);

        Box::pin(async move {
            let res = fut.await.expect("Infallible error");

            let (parts, body) = res.into_parts();
            Ok(Response::from_parts(parts, Body::new(body)))
        })
    }
}
