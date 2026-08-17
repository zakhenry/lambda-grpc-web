#[cfg(feature = "deadline")]
use crate::deadline_layer::LambdaDeadlineLayer;
#[cfg(feature = "wire-log")]
use crate::wire_log::WireLogLayer;
use http::{Request, Response};
use lambda_runtime::Error;
use std::convert::Infallible;
#[cfg(feature = "deadline")]
use std::time::Duration;
#[cfg(feature = "catch-panic")]
use tonic::Status;
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::service::Routes;
use tonic_web::GrpcWebLayer;
use tower::layer::util::{Identity, Stack};
use tower::{Layer, Service, ServiceBuilder};
#[cfg(feature = "catch-panic")]
use tower_http::catch_panic::CatchPanicLayer;

type GrpcRequest = Request<Body>;
type GrpcResponse = Response<Body>;

/// The shape a transport needs to drive: a cloneable grpc-web service that cannot fail.
///
/// Both an assembled [`LambdaRouter`] stack and the layered service inside it satisfy this, so it
/// spells the bound once rather than repeating it on every `serve_*` method. It is implemented for
/// every service that fits and cannot be implemented by hand - it exists to be named in a
/// `where` clause, not to be written.
pub trait GrpcService:
    Service<GrpcRequest, Response = GrpcResponse, Error = Infallible, Future: Send + 'static>
    + Clone
    + Send
    + 'static
{
}

impl<S> GrpcService for S where
    S: Service<GrpcRequest, Response = GrpcResponse, Error = Infallible, Future: Send + 'static>
        + Clone
        + Send
        + 'static
{
}

#[derive(Clone)]
pub struct LambdaServer<L = Identity> {
    service_builder: ServiceBuilder<L>,
}

impl LambdaServer {
    pub fn builder() -> Self {
        Self {
            service_builder: ServiceBuilder::new(),
        }
    }
}

pub struct LambdaRouter<L> {
    routes: Routes,
    service_builder: ServiceBuilder<L>,
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
            service_builder: self.service_builder,
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

    /// Drive the service with the API Gateway HTTP API (v2) event envelope, which is also what a
    /// Lambda function URL sends.
    ///
    /// The response is streamed, so the function has to be configured with the `RESPONSE_STREAM`
    /// invoke mode.
    #[cfg(feature = "transport-apigw-http")]
    #[cfg_attr(docsrs, doc(cfg(feature = "transport-apigw-http")))]
    pub async fn serve_apigw_http(self) -> Result<(), Error>
    where
        L: Layer<Routes>,
        L::Service: GrpcService,
    {
        crate::transport::apigw_http::run(self.into_service()).await
    }

    /// Drive the service with [Envoy's `aws_lambda` http filter][envoy-lambda-filter] envelope.
    ///
    /// The filter calls the buffered `Invoke` api, so the function has to be configured with the
    /// `BUFFERED` invoke mode and a server streaming response cannot be delivered incrementally.
    ///
    /// [envoy-lambda-filter]: https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/aws_lambda_filter.html
    #[cfg(feature = "transport-envoy")]
    #[cfg_attr(docsrs, doc(cfg(feature = "transport-envoy")))]
    pub async fn serve_envoy(self) -> Result<(), Error>
    where
        L: Layer<Routes>,
        L::Service: GrpcService,
    {
        crate::transport::envoy::run(self.into_service()).await
    }

    /// The grpc-web stack, assembled but not yet driven by a transport. Every `serve_*` method
    /// builds exactly this - the transport is only ever the last step.
    fn into_service(self) -> impl GrpcService
    where
        L: Layer<Routes>,
        L::Service: GrpcService,
    {
        let service_builder = ServiceBuilder::new();

        #[cfg(feature = "wire-log")]
        let service_builder = service_builder.layer(WireLogLayer);

        let service_builder = service_builder.layer(GrpcWebLayer::new());

        #[cfg(feature = "catch-panic")]
        let service_builder = service_builder.layer(CatchPanicLayer::custom(
            |err: Box<dyn std::any::Any + Send + 'static>| {
                let details = if let Some(s) = err.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = err.downcast_ref::<&str>() {
                    s.to_string()
                } else {
                    "Unknown panic message".to_string()
                };

                Status::internal(details).into_http::<Body>()
            },
        ));

        #[cfg(feature = "deadline")]
        let service_builder =
            service_builder.layer(LambdaDeadlineLayer::new(Duration::from_millis(500)));

        service_builder.service(self.service_builder.service(self.routes))
    }
}
