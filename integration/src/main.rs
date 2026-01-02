use tokio::sync::mpsc;
use tokio_stream::{pending, StreamExt};
use crate::api::test_server::{Test, TestServer};
use crate::api::unary_request::UnaryTestCase;
use crate::api::{ServerStreamRequest, ServerStreamResponse, UnaryRequest, UnaryResponse};
use lambda_grpc_web::lambda_runtime::Error;
use tokio_stream::wrappers::ReceiverStream;
use tonic::service::Routes;
use tonic::{Request, Response, Status};
use crate::api::server_stream_request::StreamTestCase;

pub mod api {
    tonic::include_proto!("integration.v1");
}


#[derive(Debug, Default)]
pub struct IntegrationTestService {}

#[tonic::async_trait]
impl Test for IntegrationTestService {
    async fn unary(&self, request: Request<UnaryRequest>) -> Result<Response<UnaryResponse>, Status> {
        match request.into_inner().test_case() {
            UnaryTestCase::Unknown => panic!("Unknown test case"),
            UnaryTestCase::Ok => Ok(Response::new(UnaryResponse {})),
            UnaryTestCase::Panic => panic!("panic test case"),
        }
    }

    type ServerStreamStream = ReceiverStream<Result<ServerStreamResponse, Status>>;
    async fn server_stream(&self, request: Request<ServerStreamRequest>) -> Result<Response<Self::ServerStreamStream>, Status> {

        let (tx, rx) = mpsc::channel::<Result<ServerStreamResponse, Status>>(1);

        tokio::spawn(async move {

            match request.into_inner().test_case() {
                StreamTestCase::Unknown => panic!("Unknown test case"),
                StreamTestCase::Ok => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                },
                StreamTestCase::PanicAfterPartialResponse => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                    panic!("panic test case")
                },
                StreamTestCase::Empty => {}, // nothing to do, will return Ok
                StreamTestCase::ImmediateError => {
                    tx.send(Err(Status::internal("immediate error"))).await.unwrap();
                },
                StreamTestCase::ErrorAfterPartialResponse => {
                    tx.send(Ok(ServerStreamResponse {})).await.unwrap();
                    tx.send(Err(Status::internal("immediate error"))).await.unwrap();
                },
                StreamTestCase::NeverRespond => {
                    pending::<()>().next().await;
                },
            }

        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

}

// run with `cargo lambda watch -p integration`
#[tokio::main]
async fn main() -> Result<(), Error> {
    let greeter = IntegrationTestService::default();

    let routes = Routes::default().add_service(TestServer::new(greeter));

    lambda_grpc_web::run(routes).await?;

    Ok(())
}
