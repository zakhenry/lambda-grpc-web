use crate::api::health_check_response::ServingStatus;
use crate::api::health_client::HealthClient;
use crate::api::server_stream_request::StreamTestCase;
use crate::api::test_client::TestClient;
use crate::api::unary_request::UnaryTestCase;
use crate::api::{
    HealthCheckRequest, ServerStreamRequest, ServerStreamResponse, UnaryRequest, UnaryResponse,
};
use http::Uri;
use hyper_rustls::HttpsConnector;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use test_context::{AsyncTestContext, test_context};
use tonic::body::Body;
use tonic_web::{GrpcWebCall, GrpcWebClientLayer, GrpcWebClientService};

pub mod api {
    tonic::include_proto!("integration.v1");
    tonic::include_proto!("grpc.health.v1");
}

/// Note these tests are not self-contained yet. currently the process is to run with cargo lambda:
/// `cargo lambda watch -p integration`
/// and then run the tests from this crate. Future plan:
///
/// 1. create a binary server implementing the above api
/// 2. define docker build to build using cargo lambda
/// 3. extend same dockerfile to copy that built bin to image based on https://hub.docker.com/r/amazon/aws-lambda-provided
/// 4. configure integration tests to use testcontainers to build and start the lambda
/// 5. execute integration tests

struct IntegrationContext {
    pub(crate) test_client:
        TestClient<GrpcWebClientService<Client<HttpsConnector<HttpConnector>, GrpcWebCall<Body>>>>,
    health_client: HealthClient<
        GrpcWebClientService<Client<HttpsConnector<HttpConnector>, GrpcWebCall<Body>>>,
    >,
}

impl AsyncTestContext for IntegrationContext {
    async fn setup() -> IntegrationContext {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_platform_verifier(rustls::crypto::aws_lc_rs::default_provider())
            .expect("should configure crypto library")
            .https_or_http()
            .enable_http1()
            .build();

        let client =
            hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector);

        let svc = tower::ServiceBuilder::new()
            .layer(GrpcWebClientLayer::new())
            .service(client);

        let origin: Uri = "http://0.0.0.0:9000".try_into().unwrap();

        let test_client = TestClient::with_origin(svc.clone(), origin.clone());
        let health_client = HealthClient::with_origin(svc, origin.clone());

        IntegrationContext {
            test_client,
            health_client,
        }
    }

    async fn teardown(self) {}
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_health(ctx: &mut IntegrationContext) {
    let res = ctx
        .health_client
        .check(HealthCheckRequest {
            service: Default::default(),
        })
        .await
        .expect("health check success");
    assert_eq!(res.into_inner().status, ServingStatus::Serving as i32);
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_unary_ok(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(UnaryRequest {
        test_case: UnaryTestCase::Ok.into(),
    });

    let response = ctx.test_client.unary(request).await.unwrap();

    assert_eq!(
        response.into_inner(),
        UnaryResponse {
            message: Some("ok response".to_string()),
        }
    );
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_unary_panic(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(UnaryRequest {
        test_case: UnaryTestCase::Panic.into(),
    });

    let response = ctx.test_client.unary(request).await;

    let err = response.unwrap_err();

    assert_eq!(err.code(), tonic::Code::Internal);
    assert_eq!(err.message(), "panic test case");
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_unary_lambda_context(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(UnaryRequest {
        test_case: UnaryTestCase::LambdaContextInHeaders.into(),
    });

    let response = ctx.test_client.unary(request).await.unwrap();

    let header_value = response
        .metadata()
        .get("lambda_ctx_deadline_ms")
        .unwrap()
        .to_str()
        .unwrap();
    let deadline_ms = header_value.parse::<u32>().unwrap();

    dbg!(deadline_ms);
    assert!(deadline_ms > 0);
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_stream_ok(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(ServerStreamRequest {
        test_case: StreamTestCase::Ok.into(),
    });

    let response = ctx.test_client.server_stream(request).await.unwrap();

    let mut stream = response.into_inner();

    let message = stream.message().await.unwrap().expect("stream message");
    assert_eq!(
        message,
        ServerStreamResponse {
            message: Some("ok response".to_string()),
        }
    );
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_stream_empty(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(ServerStreamRequest {
        test_case: StreamTestCase::Empty.into(),
    });

    let response = ctx.test_client.server_stream(request).await.unwrap();

    let mut stream = response.into_inner();

    assert_eq!(stream.message().await.unwrap(), None);
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_stream_error_after_partial_response(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(ServerStreamRequest {
        test_case: StreamTestCase::ErrorAfterPartialResponse.into(),
    });

    let response = ctx.test_client.server_stream(request).await.unwrap();

    let mut stream = response.into_inner();

    let response = stream.message().await.unwrap().expect("stream message");

    assert_eq!(response.message.unwrap().as_str(), "first ok response");

    let err_status = stream.message().await.unwrap_err();

    assert_eq!(err_status.code(), tonic::Code::Aborted);
    assert_eq!(err_status.message(), "error after partial response");
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_stream_no_response(ctx: &mut IntegrationContext) {
    let request = tonic::Request::new(ServerStreamRequest {
        test_case: StreamTestCase::NeverRespond.into(),
    });

    let response = ctx.test_client.server_stream(request).await.unwrap();

    let mut stream = response.into_inner();

    let result =
        tokio::time::timeout(std::time::Duration::from_millis(100), stream.message()).await;

    assert!(result.is_err(), "Expected timeout but stream responded");

    drop(stream);
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_meta_echo_tower_layer(ctx: &mut IntegrationContext) {
    let mut request = tonic::Request::new(UnaryRequest {
        test_case: UnaryTestCase::Ok.into(),
    });

    request.metadata_mut().insert("echo-meta", "abc-123".parse().unwrap());

    let response = ctx.test_client.unary(request).await.unwrap();

    assert_eq!(response.metadata().get("meta-value").unwrap(), "abc-123");
}

#[test_context(IntegrationContext)]
#[tokio::test]
async fn test_auth_interceptor(ctx: &mut IntegrationContext) {
    let mut request = tonic::Request::new(UnaryRequest {
        test_case: UnaryTestCase::Ok.into(),
    });

    request.metadata_mut().insert("authorization", "reject".parse().unwrap());

    let err_response = ctx.test_client.unary(request).await.unwrap_err();

    assert_eq!(err_response.code(), tonic::Code::PermissionDenied);
    assert_eq!(err_response.message(), "requested to reject");
}
