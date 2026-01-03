mod log_layer;

use crate::api::server_stream_request::StreamTestCase;
use crate::api::test_server::{Test, TestServer};
use crate::api::unary_request::UnaryTestCase;
use crate::api::{ServerStreamRequest, ServerStreamResponse, UnaryRequest, UnaryResponse};
use crate::log_layer::LogServiceNameLayer;
use lambda_grpc_web::lambda_runtime::{Context, Error};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{pending, StreamExt};
use tonic::service::{LayerExt, Routes};
use tonic::{Request, Response, Status};
use tonic::transport::Server;
use tower::{Layer, MakeService, ServiceBuilder};
use lambda_grpc_web::LambdaServer;

pub mod api {
    tonic::include_proto!("integration.v1");
}

#[derive(Debug, Default)]
pub struct IntegrationTestService {}

#[tonic::async_trait]
impl Test for IntegrationTestService {
    async fn unary(
        &self,
        request: Request<UnaryRequest>,
    ) -> Result<Response<UnaryResponse>, Status> {
        let (_meta, extensions, payload) = request.into_parts();

        match payload.test_case() {
            UnaryTestCase::Unknown => panic!("Unknown test case"),
            UnaryTestCase::Ok => Ok(Response::new(UnaryResponse {})),
            UnaryTestCase::Panic => panic!("panic test case"),
            UnaryTestCase::LambdaContextInHeaders => {
                let mut res = Response::new(UnaryResponse {});

                let ctx = extensions
                    .get::<Context>()
                    .expect("lambda context missing from request extensions");

                res.metadata_mut().insert(
                    "lambda_ctx_deadline_ms",
                    ctx.deadline.to_string().parse().unwrap(),
                );

                Ok(res)
            }
        }
    }

    type ServerStreamStream = ReceiverStream<Result<ServerStreamResponse, Status>>;
    async fn server_stream(
        &self,
        request: Request<ServerStreamRequest>,
    ) -> Result<Response<Self::ServerStreamStream>, Status> {
        let (tx, rx) = mpsc::channel::<Result<ServerStreamResponse, Status>>(1);

        tokio::spawn(async move {
            match request.into_inner().test_case() {
                StreamTestCase::Unknown => panic!("Unknown test case"),
                StreamTestCase::Ok => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                }
                StreamTestCase::PanicAfterPartialResponse => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                    panic!("panic test case")
                }
                StreamTestCase::Empty => {} // nothing to do, will return Ok
                StreamTestCase::ImmediateError => {
                    tx.send(Err(Status::internal("immediate error")))
                        .await
                        .unwrap();
                }
                StreamTestCase::ErrorAfterPartialResponse => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                    tx.send(Err(Status::internal("immediate error")))
                        .await
                        .unwrap();
                }
                StreamTestCase::NeverRespond => {
                    pending::<()>().next().await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

// run with `cargo lambda watch -p integration`
#[tokio::main]
async fn main() -> Result<(), Error> {
    let greeter = IntegrationTestService::default();


    LambdaServer::builder()
        .layer(LogServiceNameLayer::default())
        .add_service(TestServer::new(greeter))
        .serve().await?;

    Ok(())
}
