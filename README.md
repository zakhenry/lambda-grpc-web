# lambda-grpc-web

Run **[Tonic](https://github.com/hyperium/tonic) gRPC services on AWS Lambda**.

This crate makes gRPC usable on Lambda, but with unavoidable tradeoffs.

It is designed for:

- Unary gRPC APIs
- Short-lived server streaming
- Scale-to-zero, low-ops deployments

It is *not* a full replacement for a native HTTP/2 gRPC server.

---

## What this is (and is not)

### This crate **does**

- Run existing `tonic` services on AWS Lambda
- Preserve Tower middleware composition
- Support gRPC-Web clients
- Work with Lambda Function URLs / API Gateway

### This crate **does not**

- Provide full HTTP/2 semantics
- Support bidirectional or client streaming
- Preserve connection-level state (note client cancellations is not possible)
- Eliminate Lambda cold starts or latency tradeoffs

---

## Quick start

### 1. Write service
Define a normal Tonic service, and substitute only the `tonic::transport::Server` builder with `lambda_grpc_web::LambdaServer`

```rust
use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};
use lambda_grpc_web::lambda_runtime::Error;
use lambda_grpc_web::LambdaServer;
use tonic::{Request, Response, Status};

// note everything from here until the main fn is vanilla tonic service

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

#[tokio::main]
async fn main() -> Result<(), Error> { // <- note error here is `lambda_grpc_web::lambda_runtime::Error` 
    let greeter = MyGreeter::default();

    LambdaServer::builder() // <- Different builder
        .add_service(GreeterServer::new(greeter))
        .serve() // <- no socket declared
        .await?;

    Ok(())
}
```

### 2. Test locally with cargo lambda

Refer to [https://cargo-lambda.info]() for more information

```shell
cargo lambda watch
```

Important note - the grpc service frames messages with grpc-web - your test client must be able to talk this protocol.

### 3. Deploy

Compile with cargo lambda (refer to their docs)

#### Tips:
* Make sure to configure invoke mode as `RESPONSE_STREAM`
* Configure a sensible timeout as client disconnects cannot propagate to lambda cancellation.

## Supported features

| Feature                     | Status        |
| --------------------------- |---------------|
| Unary RPCs                  | Supported     |
| Server streaming            | Limited       |
| Client streaming            | Not supported |
| Bidirectional streaming     | Not supported |
| Interceptors / Tower layers | Supported     |
| Trailers                    | Supported     |

---

## Compatibility notes

### Transport

- Requests are handled via **HTTP/1.1**
- gRPC-Web framing is used
- HTTP/2-specific features are unavailable

### Middleware

Works:

- Logging
- Auth
- Metrics
- Deadlines (request-scoped)

Does **not** work as expected:

- Middleware requiring connection state
- Peer IP / TLS inspection
- Channel-level interceptors

---

## Performance expectations

This crate optimizes for:

- Operational simplicity
- Cost efficiency
- Scale-to-zero workloads

It does **not** optimize for:

- Lowest possible latency (cold starts while fast in rust still add latency)
- Long-lived connections (Lambda has a 15-minute hard maximum duration)

If you need those, run native gRPC behind ALB / ECS / EC2.

---

## Future work
* Support managed lambdas - should mostly just work
* Flesh out docs as more of a tutorial style including deployment
