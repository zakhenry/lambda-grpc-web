//! Lambda event transports.
//!
//! A lambda function is invoked with a JSON event whose shape is decided by whatever is in front
//! of it, and the grpc-web request/response has to be dug out of (and packed back into) that
//! envelope. Each transport is a module with a `run` of its own, and the rest of the crate talks
//! plain `http::Request` / `http::Response` without caring which one drives it.
//!
//! The features are additive on purpose: enabling both compiles both, which is what lets one
//! workspace hold a function behind API Gateway and another behind Envoy. Which one a given
//! function uses is decided at the call site by the matching [`LambdaRouter`] method
//! (`serve_apigw_http` / `serve_envoy`), so a server can still only ever be driven by one.
//!
//! [`LambdaRouter`]: crate::LambdaRouter

#[cfg(feature = "transport-apigw-http")]
pub(crate) mod apigw_http;

#[cfg(feature = "transport-envoy")]
pub(crate) mod envoy;
#[cfg(feature = "transport-envoy")]
pub use envoy::{EnvoyRequest, EnvoyResponse};
