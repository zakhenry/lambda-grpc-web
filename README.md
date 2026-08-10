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

### 0. Pick a transport

A lambda function is invoked with a JSON event whose shape is decided by whatever sits in front of
it, so the transport is a compile time choice. There is deliberately **no default** — pick exactly
one:

| Feature                | Invoked by                                             | Lambda invoke mode |
|------------------------|--------------------------------------------------------|--------------------|
| `transport-apigw-http` | API Gateway HTTP API (v2), Lambda function URLs         | `RESPONSE_STREAM`  |
| `transport-envoy`      | Envoy's [`aws_lambda` http filter][envoy-lambda-filter] | `BUFFERED`         |

```toml
[dependencies]
lambda-grpc-web = { version = "0.1", features = ["transport-apigw-http"] }
```

Enabling both is a compile error. The rest of the API is identical either way — nothing below
changes apart from the deployment notes in [step 3](#3-deploy).

[envoy-lambda-filter]: https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/aws_lambda_filter.html

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
> Configure a sensible timeout as client disconnects cannot propagate to lambda cancellation. This
> applies to both transports.

#### With `transport-apigw-http`

> [!IMPORTANT]
> Configure the invoke mode as `RESPONSE_STREAM`. Responses are streamed frame by frame, so a
> server streaming RPC is delivered incrementally.

#### With `transport-envoy`

> [!IMPORTANT]
> Configure the invoke mode as `BUFFERED`, **not** `RESPONSE_STREAM`. Envoy's filter calls the
> buffered `Invoke` api and has no way to consume a streamed response.

Envoy invokes the function directly, so there is no API Gateway in the picture:

```yaml
route_config:
  virtual_hosts:
    - name: grpc_web
      domains: ["*"]
      routes:
        - match: { prefix: "/" }
          route: { cluster: lambda_greeter }

http_filters:
  - name: envoy.filters.http.aws_lambda
    typed_config:
      "@type": type.googleapis.com/envoy.extensions.filters.http.aws_lambda.v3.Config
      arn: "arn:aws:lambda:eu-west-1:123456789012:function:greeter"
      # required - see the payload_passthrough note below
      payload_passthrough: false
      invocation_mode: SYNCHRONOUS

clusters:
  - name: lambda_greeter
    type: LOGICAL_DNS
    # per-route metadata that tells the filter this cluster is a lambda egress gateway
    metadata:
      filter_metadata:
        com.amazonaws.lambda:
          egress_gateway: true
    load_assignment:
      cluster_name: lambda_greeter
      endpoints:
        - lb_endpoints:
            - endpoint:
                address:
                  socket_address: { address: lambda.eu-west-1.amazonaws.com, port_value: 443 }
    transport_socket:
      name: envoy.transport_sockets.tls
      typed_config:
        "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
        sni: lambda.eu-west-1.amazonaws.com
```

##### The downstream client must still speak grpc-web

Adding Envoy in front does **not** make the service reachable from an ordinary HTTP/2 gRPC client.

* ✅ `grpc-web client → Envoy → Lambda` — Envoy is a routing / auth / mesh hop, and the grpc-web
  body (including its in-body trailer frame) passes through the JSON envelope untouched in both
  directions. This is what the feature is for.
* ❌ `gRPC client (HTTP/2) → Envoy → Lambda` — the *request* direction happens to work, because
  gRPC and grpc-web use identical 5 byte message framing. The *response* direction does not: the
  lambda returns grpc-web with trailers packed into the body, and nothing in the stock filter
  chain unpacks that back into HTTP/2 trailers. The client sees a stray `0x80` frame at the end of
  the body and no `grpc-status`, and errors.

Envoy's `envoy.filters.http.grpc_web` filter converts the *opposite* direction (downstream
grpc-web → upstream gRPC), so it does not help here. Converting downstream gRPC → upstream
grpc-web needs a dynamic module, currently blocked on
[envoyproxy/envoy#42559](https://github.com/envoyproxy/envoy/issues/42559).

##### Testing without an Envoy

Envoy invokes the function with a JSON envelope rather than proxying http to it, so there is no
endpoint to point a gRPC client at. The envelope types are exported for this — `EnvoyRequest::new`
takes the *unencoded* grpc-web body and applies the base64 Envoy would, and `EnvoyResponse::body`
takes it back off:

```rust
use lambda_grpc_web::{EnvoyRequest, EnvoyResponse};

let event = EnvoyRequest::new("/helloworld.Greeter/SayHello", grpc_web_frames);
let response: EnvoyResponse = serde_json::from_slice(&invoke(serde_json::to_vec(&event)?)?)?;

assert_eq!(response.status_code(), 200); // a gRPC error is a 200 too, see the trailer frame
let grpc_web_body = response.body()?;   // base64 layer removed
```

You do not have to frame the grpc-web body yourself: drop that conversion in as the innermost
service under `tonic_web::GrpcWebClientLayer` and the generated Tonic client works unchanged, with
real `Status` errors and metadata.

A worked example — service, `envoy.yaml`, and a test doing exactly that against the
`cargo lambda watch` emulator — is in [`examples/envoy`](examples/envoy). It is deliberately its
own workspace, because cargo unifies features across the members it builds together and the other
examples select `transport-apigw-http`:

```shell
cd examples/envoy
cargo lambda watch                                              # terminal 1
cargo lambda invoke example-envoy --data-file events/say_hello.json  # terminal 2
```

##### Limitations

* **`payload_passthrough: true` is not supported.** With passthrough enabled the function receives
  the raw body with no path and no `content-type`, which leaves nothing to route a gRPC method on.
* **Repeated request metadata arrives coalesced.** Envoy's envelope carries headers as a flat map,
  having already joined repeated headers into a single comma separated value. Repeated
  `grpc-metadata-*` keys therefore arrive as one value, and splitting them back apart is not
  attempted because it would corrupt values that legitimately contain commas.
* **Server streaming is buffered** — see below.

## Supported features

| Feature                     | Status        | Note                                                                                    |
|-----------------------------|---------------|-----------------------------------------------------------------------------------------|
| Unary RPCs                  | Supported     |                                                                                         |
| Server streaming            | Limited       | `transport-apigw-http`: streamed, capped by lambda timeout. `transport-envoy`: buffered¹ |
| Client streaming            | Not supported | Not supported in gRPC web                                                               |
| Bidirectional streaming     | Not supported | Not supported in gRPC web                                                               |
| Interceptors / Tower layers | Supported     |                                                                                         |
| Metadata (Headers+Trailers) | Supported     | `transport-envoy`: repeated request metadata arrives coalesced                          |

¹ Under `transport-envoy` the whole stream is collected before anything is returned, so it is
bounded by the lambda **response payload size limit** as well as the timeout, and messages are not
delivered incrementally. A warning is logged when a buffered response turns out to contain more
than one message.

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
