use crate::deadline_layer::LambdaDeadlineLayer;
use crate::grpc_lambda_layer::GrpcLambdaService;
use http::Request;
use lambda_runtime::Error;
use std::any::Any;
use std::convert::Infallible;
use std::time::Duration;
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::Status;
use tonic_web::GrpcWebLayer;
use tower::layer::util::{Identity, Stack};
use tower::{Layer, Service, ServiceBuilder};
use tower_http::catch_panic::CatchPanicLayer;

#[derive(Clone)]
pub struct LambdaServer<L = Identity> {
    service_builder: ServiceBuilder<L>
}

impl LambdaServer {
    pub fn builder() -> Self {
        Self {
            service_builder: ServiceBuilder::new()
        }
    }
}

pub struct LambdaRouter<L> {
    routes: Routes,
    service_builder: ServiceBuilder<L>
}

impl<L> LambdaServer<L> {
    pub fn layer<NewLayer>(self, new_layer: NewLayer) -> LambdaServer<Stack<NewLayer, L>> {
        LambdaServer {
            service_builder: self.service_builder.layer(new_layer),
        }
    }

    pub fn add_service<S>(self, svc: S) -> LambdaRouter<L>
    where
        S: Service<Request<Body>, Error = Infallible>
        + NamedService
        + Clone
        + Send
        + Sync
        + 'static,
        S::Response: axum::response::IntoResponse,
        S::Future: Send + 'static,
        L: Clone,
    {
        LambdaRouter {
            routes: Routes::new(svc),
            service_builder: self.service_builder
        }
    }
}

impl<L> LambdaRouter<L> {
    pub fn add_service<S>(mut self, svc: S) -> Self
    where
        S: Service<Request<Body>, Error = Infallible>
        + NamedService
        + Clone
        + Send
        + Sync
        + 'static,
        S::Response: axum::response::IntoResponse,
        S::Future: Send + 'static,
    {
        self.routes = self.routes.add_service(svc);
        self
    }

    pub async fn serve(self) -> Result<(), Error>
    where
        L: Layer<Routes>,
        L::Service: Service<Request<Body>, Response = http::Response<Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
        <L::Service as Service<Request<Body>>>::Future: Send + 'static,
    {
        let mut svc = self
            .service_builder
            .service(self.routes);

        #[cfg(feature = "catch-panic")]
        let mut svc = ServiceBuilder::new()
            .layer(CatchPanicLayer::custom(|err: Box<dyn Any + Send + 'static>| {
                let details = if let Some(s) = err.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = err.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "Unknown panic message".to_string()
                };

                Status::internal(details)
                    .into_http::<Body>()
            }))
            .service(svc);

        let mut svc = ServiceBuilder::new()
            .layer(GrpcWebLayer::new())
            .service(svc);

        let svc = tower::ServiceExt::map_request(svc, |req: Request<tonic::service::AxumBody>| {
            req.map(Body::new)
        });

        let svc = tower::ServiceExt::map_response(svc, |res: http::Response<Body>| {
            res.map(tonic::service::AxumBody::new)
        });

        #[cfg(feature = "deadline")]
        let svc = ServiceBuilder::new()
            .layer(LambdaDeadlineLayer::new(Duration::from_millis(500)))
            .service(svc);

        let handler = GrpcLambdaService::new(svc);
        lambda_http::run_with_streaming_response(handler).await
    }
}
