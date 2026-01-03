use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use lambda_grpc_web::lambda_runtime::Error;
use lambda_grpc_web::lambda_runtime::tracing::log::info;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::service::Routes;
use tonic::{Request, Response, Status};
use lambda_grpc_web::LambdaServer;

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

    type StreamHelloStream = ReceiverStream<Result<HelloReply, Status>>;

    async fn stream_hello(
        &self,
        request: Request<HelloRequest>,
    ) -> Result<Response<Self::StreamHelloStream>, Status> {
        let (tx, rx) = mpsc::channel::<Result<HelloReply, Status>>(1);

        let responses: Vec<String> = request
            .into_inner()
            .name
            .chars()
            .into_iter()
            .scan(String::new(), |acc, c| {
                acc.push(c);
                Some(acc.clone())
            })
            .collect();

        tokio::spawn(async move {
            for response in responses {
                if tx.send(Ok(HelloReply { message: response })).await.is_err() {
                    info!("client dropped connection");
                    return;
                }

                sleep(Duration::from_millis(20)).await;
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// run with `cargo lambda watch -p example-hello-world`
// build for aws with `cargo lambda build -p example-hello-world --release --output-format zip --arm64`
#[tokio::main]
async fn main() -> Result<(), Error> {
    let greeter = MyGreeter::default();

    LambdaServer::builder().add_service(GreeterServer::new(greeter)).serve().await?;

    Ok(())
}

// integration, run `cargo lambda watch -p example-hello-world` first to have the local lambda
// running, or change the url to the deployed env
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hello_world::greeter_client::GreeterClient;
    use hyper_rustls::HttpsConnector;
    use hyper_util::client::legacy::Client;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::rt::TokioExecutor;
    use lambda_grpc_web::lambda_runtime::tower;
    use tonic::body::Body;
    use tonic::codegen::tokio_stream::StreamExt;
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

    #[tokio::test]
    async fn stream_test() -> Result<(), Box<dyn std::error::Error>> {
        let request = Request::new(HelloRequest {
            name: "foobar".into(),
        });

        let response = make_greeter_client()?.stream_hello(request).await?;

        println!("HEADERS={headers:?}", headers = response.metadata());
        let mut stream = response.into_inner();

        let result: Vec<String> = stream
            .map(|response| {
                println!("RESPONSE={response:?}");
                response.unwrap().message
            })
            .collect()
            .await;

        // stream.trailers().await?;

        assert_eq!(
            result,
            vec![
                "f".to_string(),
                "fo".to_string(),
                "foo".to_string(),
                "foob".to_string(),
                "fooba".to_string(),
                "foobar".to_string(),
            ]
        );

        Ok(())
    }
}
