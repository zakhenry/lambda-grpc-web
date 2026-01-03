use std::any::Any;
use crate::grpc_lambda_layer::GrpcLambdaService;
use http::Request;
use lambda_runtime::Error;
use std::convert::Infallible;
use std::time::Duration;
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic::Status;
use tonic::transport::server::Router;
use tonic_web::GrpcWebLayer;
use tower::layer::util::{Identity, Stack};
use tower::{Service, ServiceBuilder};
use tower_http::catch_panic::CatchPanicLayer;
use crate::deadline_layer::LambdaDeadlineLayer;

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

impl <L>LambdaServer<L> {

    pub fn layer<NewLayer>(self, new_layer: NewLayer) -> LambdaServer<Stack<NewLayer, L>> {
        LambdaServer {
            service_builder: self.service_builder.layer(new_layer),
        }
    }

    pub fn add_service<S>(self, svc: S) -> LambdaServer<Stack<S, L>>
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
        // Router::new(self.clone(), Routes::new(svc))
        todo!()
    }

    pub async fn serve(self) -> Result<(), Error> {

        let service_builder = self.service_builder.layer(GrpcWebLayer::new());

        #[cfg(feature = "catch-panic")]
        let service_builder = service_builder.layer(CatchPanicLayer::custom(
            |err: Box<dyn Any + Send + 'static>| {
                let details = if let Some(s) = err.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = err.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "Unknown panic message".to_string()
                };

                Status::internal(details).into_http::<lambda_runtime::streaming::Body>()
            },
        ));

        #[cfg(feature = "deadline")]
        let service_builder = service_builder.layer(LambdaDeadlineLayer::new(Duration::from_millis(500)));

        todo!();
        // let handler = GrpcLambdaService::new(service_builder.boxed());
        // lambda_http::run_with_streaming_response(handler).await
    }
}
