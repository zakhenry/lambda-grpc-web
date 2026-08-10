//! Lambda event transports.
//!
//! A lambda function is invoked with a JSON event whose shape is decided by whatever is in front
//! of it, and the grpc-web request/response has to be dug out of (and packed back into) that
//! envelope. Exactly one transport is compiled in, selected by feature flag, so the rest of the
//! crate can talk plain `http::Request` / `http::Response` without caring which one it is.

// The `not(..)` halves keep a misconfigured feature set reporting only the `compile_error!` in
// `lib.rs`, rather than burying it under a duplicate definition of `run`.
#[cfg(all(feature = "transport-apigw-http", not(feature = "transport-envoy")))]
mod apigw_http;
#[cfg(all(feature = "transport-apigw-http", not(feature = "transport-envoy")))]
pub(crate) use apigw_http::run;

#[cfg(all(feature = "transport-envoy", not(feature = "transport-apigw-http")))]
mod envoy;
#[cfg(all(feature = "transport-envoy", not(feature = "transport-apigw-http")))]
pub(crate) use envoy::run;
#[cfg(all(feature = "transport-envoy", not(feature = "transport-apigw-http")))]
pub use envoy::{EnvoyRequest, EnvoyResponse};
