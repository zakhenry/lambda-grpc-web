mod log_layer;
mod meta_echo_layer;
mod auth_interceptor;

use crate::api::health_check_response::ServingStatus;
use crate::api::health_server::{Health, HealthServer};
use crate::api::server_stream_request::StreamTestCase;
use crate::api::test_server::{Test, TestServer};
use crate::api::unary_request::UnaryTestCase;
use crate::api::{
    HealthCheckRequest, HealthCheckResponse, ServerStreamRequest, ServerStreamResponse,
    UnaryRequest, UnaryResponse,
};
use crate::auth_interceptor::AuthInterceptor;
use crate::log_layer::LogServiceNameLayer;
use crate::meta_echo_layer::MetaEchoLayer;
use lambda_grpc_web::lambda_runtime::{Context, Error};
use lambda_grpc_web::LambdaServer;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{pending, StreamExt};
use tonic::{Request, Response, Status};

pub mod api {
    tonic::include_proto!("integration.v1");
    tonic::include_proto!("grpc.health.v1");
}

struct IntegrationTestService;
struct HealthTestService;

#[tonic::async_trait]
impl Test for IntegrationTestService {
    async fn unary(
        &self,
        request: Request<UnaryRequest>,
    ) -> Result<Response<UnaryResponse>, Status> {
        let (_meta, extensions, payload) = request.into_parts();

        match payload.test_case() {
            UnaryTestCase::Unknown => panic!("Unknown test case"),
            UnaryTestCase::Ok => Ok(Response::new(UnaryResponse {
                message: Some("ok response".to_string()),
            })),
            UnaryTestCase::Panic => panic!("panic test case"),
            UnaryTestCase::LambdaContextInHeaders => {
                let mut res = Response::new(UnaryResponse {
                    message: Some("lambda context response".to_string()),
                });

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
                    tx.send(Ok(ServerStreamResponse {
                        message: Some("ok response".to_string()),
                    }))
                    .await
                    .unwrap();
                }
                StreamTestCase::Empty => {} // nothing to do, will return Ok
                StreamTestCase::ImmediateError => {
                    tx.send(Err(Status::internal("immediate error")))
                        .await
                        .unwrap();
                }
                StreamTestCase::ErrorAfterPartialResponse => {
                    tx.send(Ok(ServerStreamResponse {
                        message: Some("first ok response".to_string()),
                    }))
                    .await
                    .unwrap();
                    tx.send(Err(Status::aborted("error after partial response")))
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

#[tonic::async_trait]
impl Health for HealthTestService {
    async fn check(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        Ok(Response::new(HealthCheckResponse {
            status: ServingStatus::Serving.into(),
        }))
    }

    type WatchStream = ReceiverStream<Result<HealthCheckResponse, Status>>;
    async fn watch(
        &self,
        _request: Request<HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        todo!()
    }
}

// run with `cargo lambda watch -p integration`
// build for aws with `cargo lambda build -p integration --release --output-format zip --arm64`
#[tokio::main]
async fn main() -> Result<(), Error> {

    LambdaServer::builder()
        .layer(LogServiceNameLayer::default())
        .layer(MetaEchoLayer::default())
        .add_service(TestServer::with_interceptor(IntegrationTestService, AuthInterceptor))
        .add_service(HealthServer::new(HealthTestService))
        .serve()
        .await?;

    Ok(())

}
