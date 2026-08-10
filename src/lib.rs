#[cfg(feature = "deadline")]
mod deadline_layer;
mod lambda_server_builder;
mod transport;

#[cfg(all(feature = "transport-apigw-http", feature = "transport-envoy"))]
compile_error!(
    "the `transport-apigw-http` and `transport-envoy` features are mutually exclusive - exactly \
     one transport is compiled in. Pick the one matching what invokes the function and remove the \
     other, e.g. \
     `lambda-grpc-web = { version = \"...\", features = [\"transport-envoy\"] }`. Note that \
     features are additive across a dependency graph, so another crate in the workspace may be \
     enabling the one you did not ask for."
);

#[cfg(not(any(feature = "transport-apigw-http", feature = "transport-envoy")))]
compile_error!(
    "no transport feature is enabled - enable exactly one of `transport-apigw-http` (for API \
     Gateway HTTP APIs and Lambda function URLs, invoke mode `RESPONSE_STREAM`) or \
     `transport-envoy` (for Envoy's `aws_lambda` http filter, invoke mode `BUFFERED`). There is \
     deliberately no default: the two receive different event envelopes, so the choice has to \
     match the deployment, e.g. \
     `lambda-grpc-web = { version = \"...\", features = [\"transport-apigw-http\"] }`"
);

pub use lambda_runtime;
pub use lambda_server_builder::LambdaServer;

/// The Envoy event envelopes, so that a function can be exercised in tests without an Envoy in
/// front of it. See [`EnvoyRequest::new`].
// the `not(..)` half matches `transport/mod.rs`, keeping a misconfigured feature set reporting the
// `compile_error!` above rather than an unresolved import
#[cfg(all(feature = "transport-envoy", not(feature = "transport-apigw-http")))]
pub use transport::{EnvoyRequest, EnvoyResponse};

#[cfg(feature = "wire-log")]
mod wire_log;
#[cfg(feature = "wire-log")]
pub use wire_log::{WireLogLayer, WireLogService};
