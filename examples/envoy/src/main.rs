//! The `transport-envoy` counterpart to the `example-hello-world` crate.
//!
//! The service itself is ordinary Tonic and identical to the API Gateway example - the only
//! difference is the transport feature in `Cargo.toml`, and how you poke at it locally (see the
//! tests at the bottom of this file).

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use lambda_grpc_web::LambdaServer;
use lambda_grpc_web::lambda_runtime::Error;
use tonic::{Request, Response, Status};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[derive(Debug, Default)]
pub struct MyGreeter {}

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<HelloReply>, Status> {
        println!("Got a request: {request:?}");

        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };

        Ok(Response::new(reply))
    }
}

// run with `cargo lambda watch` from this directory (`examples/envoy`) - it is its own workspace,
// so `-p example-envoy` from the repo root will not find it
// invoke with `cargo lambda invoke example-envoy --data-file events/say_hello.json`
// build for aws with `cargo lambda build --release --output-format zip --arm64`
//
// when deploying, configure the function's invoke mode as `BUFFERED` - Envoy's filter calls the
// buffered `Invoke` api and cannot consume a streamed response.
#[tokio::main]
async fn main() -> Result<(), Error> {
    let greeter = MyGreeter::default();

    LambdaServer::builder()
        .add_service(GreeterServer::new(greeter))
        .serve()
        .await?;

    Ok(())
}

// integration, run `cargo lambda watch` in this directory first to have the local lambda running.
//
// The generated Tonic client is used exactly as in the API Gateway example, with one difference in
// the stack underneath it: Envoy invokes the function with a JSON envelope rather than proxying
// http to it, so there is no endpoint for a plain http client to dial. `tonic_web`'s
// `GrpcWebClientLayer` still does all the grpc-web framing - only the innermost service changes,
// to one that packs the framed request into the envelope and unpacks the reply.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello_world::greeter_client::GreeterClient;
    use bytes::Bytes;
    use http::header::CONTENT_TYPE;
    use http_body_util::{BodyExt, Full};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use lambda_grpc_web::{EnvoyRequest, EnvoyResponse};
    use tonic_web::GrpcWebClientLayer;

    const INVOKE_URL: &str = "http://127.0.0.1:9000/2015-03-31/functions/example-envoy/invocations";

    /// Stands in for Envoy: takes the grpc-web request `GrpcWebClientLayer` produced, packs it into
    /// the JSON envelope, invokes the function, and unpacks the envelope back into an http
    /// response for the layer to decode.
    ///
    /// The grpc-web framing is entirely `tonic-web`'s job on both sides - the only thing this crate
    /// contributes is the envelope, via [`EnvoyRequest`] / [`EnvoyResponse`].
    async fn invoke<B>(request: http::Request<B>) -> Result<http::Response<Full<Bytes>>, Error>
    where
        B: http_body::Body<Data = Bytes>,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        let (parts, body) = request.into_parts();
        let frames = body.collect().await?.to_bytes();

        // `raw_path` is the gRPC method path; Envoy forwards the request headers as a flat map
        let mut event = EnvoyRequest::new(parts.uri.path(), frames);
        for (name, value) in parts.headers.iter() {
            event = event.with_header(name.as_str(), value.to_str()?);
        }

        let http_client = Client::builder(TokioExecutor::new()).build_http();

        let invocation = http::Request::builder()
            .method(http::Method::POST)
            .uri(INVOKE_URL)
            .header(CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(serde_json::to_vec(&event)?)))?;

        let invoked = http_client.request(invocation).await?;
        let envelope: EnvoyResponse =
            serde_json::from_slice(&invoked.into_body().collect().await?.to_bytes())?;

        let mut response = http::Response::builder().status(envelope.status_code());
        for (name, value) in envelope.headers() {
            response = response.header(name, value);
        }

        Ok(response.body(Full::new(Bytes::from(envelope.body()?)))?)
    }

    #[tokio::test]
    async fn unary_test() -> Result<(), Box<dyn std::error::Error>> {
        let svc = tower::ServiceBuilder::new()
            .layer(GrpcWebClientLayer::new())
            .service(tower::service_fn(invoke));

        // the origin is never dialled - `invoke` posts at the emulator - but tonic needs one to
        // build request uris from
        let mut client = GreeterClient::with_origin(svc, "http://lambda.invalid".try_into()?);

        let response = client
            .say_hello(HelloRequest {
                name: "grpc web client".into(),
            })
            .await?;

        assert_eq!(
            response.into_inner(),
            HelloReply {
                message: "Hello grpc web client!".to_string(),
            }
        );

        Ok(())
    }
}
