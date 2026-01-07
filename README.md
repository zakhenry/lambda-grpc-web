# lambda-grpc-web

Run **[Tonic](https://github.com/hyperium/tonic) gRPC services on AWS Lambda**.

This crate makes [gRPC](https://grpc.io) usable on [AWS Lambda](https://aws.amazon.com/lambda/), with [grpc-web message framing](https://grpc.github.io/grpc/core/md_doc__p_r_o_t_o_c_o_l-_w_e_b.html) as lambda-compatible http/1.1 transport.

This enables serverless deployments for gRPC workloads that are spiky, generally low volume (i.e. benefit from being
able to need to scale to zero), and typically are connect to from web browsers (i.e. already limited to using gRPC web 
protocol)

> [!IMPORTANT]
> This is *not* a full replacement for a native http/2 gRPC server. Limitations inherent to the AWS lambda runtime apply
> [See below](#supported-features) for more detail of supported capabilities

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

> [!TIP]
> * Make sure to configure invoke mode as `RESPONSE_STREAM`
> * Configure a sensible timeout as client disconnects cannot propagate to lambda cancellation.

## Supported features

| Feature                     | Status        | Note                      |
|-----------------------------|---------------|---------------------------|
| Unary RPCs                  | Supported     |                           |
| Server streaming            | Limited       | Capped by lambda timeout  |
| Client streaming            | Not supported | Not supported in gRPC web |
| Bidirectional streaming     | Not supported | Not supported in gRPC web |
| Interceptors / Tower layers | Supported     |                           |
| Metadata (Headers+Trailers) | Supported     |                           |

---

## Performance

Since this is a serverless environment, it is subject to cold start times. While Rust runtime is very fast 
(typically 20-30ms), it is not going to be as fast as a standard always-running gRPC service on ECS or similar.

When executing on a warm instance, latency should be very low albeit with minor overhead from the grpc-web framing.

*If maximum performance is your goal, gRPC might not be the best fit to begin with*. For nearly all other workloads, 
this architecture will be more than fast enough.

---

## Future work
* Support managed lambdas - should mostly just work
* Flesh out docs as more of a tutorial style including deployment
