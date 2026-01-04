use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use lambda_grpc_web::lambda_runtime::Error;
use lambda_grpc_web::LambdaServer;
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
        println!("Got a request: {:?}", request);

        let reply = HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };

        Ok(Response::new(reply))
    }

}

// run with `cargo lambda watch -p example-hello-world`
// build for aws with `cargo lambda build -p example-hello-world --release --output-format zip --arm64`
#[tokio::main]
async fn main() -> Result<(), Error> {
    let greeter = MyGreeter::default();

    LambdaServer::builder()
        .add_service(GreeterServer::new(greeter))
        .serve()
        .await?;

    Ok(())
}

// integration, run `cargo lambda watch -p example-hello-world` first to have the local lambda
// running, or change the url to the deployed env
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello_world::greeter_client::GreeterClient;
    use hyper_rustls::HttpsConnector;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use lambda_grpc_web::lambda_runtime::tower;
    use tonic::body::Body;
    use tonic_web::{GrpcWebCall, GrpcWebClientLayer, GrpcWebClientService};

    fn make_greeter_client() -> Result<
        GreeterClient<
            GrpcWebClientService<Client<HttpsConnector<HttpConnector>, GrpcWebCall<Body>>>,
        >,
        Box<dyn std::error::Error>,
    > {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_platform_verifier(rustls::crypto::aws_lc_rs::default_provider())
            .expect("should configure crypto library")
            .https_or_http()
            .enable_http1()
            .build();

        let client = Client::builder(TokioExecutor::new()).build(connector);

        let svc = tower::ServiceBuilder::new()
            .layer(GrpcWebClientLayer::new())
            .service(client);

        let client = GreeterClient::with_origin(
            svc,
            "http://0.0.0.0:9000/lambda-url/example-hello-world".try_into()?,
        );

        Ok(client)
    }

    #[tokio::test]
    async fn unary_test() -> Result<(), Box<dyn std::error::Error>> {
        let request = tonic::Request::new(HelloRequest {
            name: "grpc web client".into(),
        });

        let response = make_greeter_client()?.say_hello(request).await?;

        println!("RESPONSE={response:?}");

        Ok(())
    }

}
