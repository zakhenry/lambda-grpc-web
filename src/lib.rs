// `docsrs` is set by the `rustdoc-args` in `[package.metadata.docs.rs]`, so the feature badges are
// built on docs.rs (which runs nightly) and this is inert everywhere else.
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "deadline")]
mod deadline_layer;
mod lambda_server_builder;
mod transport;

// The transport features are additive - enabling both is supported and compiles both, so that a
// workspace can hold one function behind API Gateway and another behind Envoy. Which one a server
// uses is picked by calling `serve_apigw_http` or `serve_envoy`, not by the feature set. At least
// one has to be on, though: with none there is no way to run anything.
#[cfg(not(any(feature = "transport-apigw-http", feature = "transport-envoy")))]
compile_error!(
    "no transport feature is enabled - enable at least one of `transport-apigw-http` (for API \
     Gateway HTTP APIs and Lambda function URLs, invoke mode `RESPONSE_STREAM`) or \
     `transport-envoy` (for Envoy's `aws_lambda` http filter, invoke mode `BUFFERED`). There is \
     deliberately no default: the two receive different event envelopes, so the choice has to \
     match the deployment, e.g. \
     `lambda-grpc-web = { version = \"...\", features = [\"transport-apigw-http\"] }`"
);

pub use lambda_runtime;
pub use lambda_server_builder::{GrpcService, LambdaRouter, LambdaServer};

/// The Envoy event envelopes, so that a function can be exercised in tests without an Envoy in
/// front of it. See [`EnvoyRequest::new`].
#[cfg(feature = "transport-envoy")]
#[cfg_attr(docsrs, doc(cfg(feature = "transport-envoy")))]
pub use transport::{EnvoyRequest, EnvoyResponse};

#[cfg(feature = "wire-log")]
mod wire_log;
#[cfg(feature = "wire-log")]
#[cfg_attr(docsrs, doc(cfg(feature = "wire-log")))]
pub use wire_log::{WireLogLayer, WireLogService};
