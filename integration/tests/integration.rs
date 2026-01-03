use crate::api::server_stream_request::StreamTestCase;
use crate::api::test_client::TestClient;
use crate::api::{ServerStreamRequest, UnaryRequest};
use hyper_util::client::legacy::connect::HttpInfo;
use hyper_util::rt::TokioExecutor;
use tokio_stream::StreamExt;
use tonic_web::GrpcWebClientLayer;

pub mod api {
    tonic::include_proto!("integration.v1");
}

/// Integration testing plan
/// 1. create a binary server implementing the above api
/// 2. define docker build to build using cargo lambda
/// 3. extend same dockerfile to copy that built bin to image based on https://hub.docker.com/r/amazon/aws-lambda-provided
/// 4. configure integration tests to use testcontainers to build and start the lambda
/// 5. execute integration tests (using test-context crate)

#[tokio::test]
async fn test_stream() {
    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http();

    let svc = tower::ServiceBuilder::new()
        .layer(GrpcWebClientLayer::new())
        .service(client);

    let mut client = TestClient::with_origin(svc, "http://0.0.0.0:9000".try_into().unwrap());

    let request = tonic::Request::new(ServerStreamRequest {
        // test_case: StreamTestCase::NeverRespond.into(),
        // test_case: StreamTestCase::PanicAfterPartialResponse.into(),
        test_case: StreamTestCase::ImmediateError.into(),
        // test_case: StreamTestCase::Ok.into(),
    });

    let response = client.server_stream(request).await.unwrap();

    let mut stream = response.into_inner();

    while let Some(result) = stream.next().await {
        println!("RESPONSE={result:#?}");
    }
}
