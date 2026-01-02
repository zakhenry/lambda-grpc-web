use lambda_http::http::{Request, Response};
use lambda_runtime::Context as LambdaContext;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::time::{sleep_until, Instant};
use tonic::service::AxumBody;
use tonic::Status;
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub struct LambdaDeadlineLayer {
    /// Safety margin before the Lambda hard-deadline
    margin: Duration,
}

impl LambdaDeadlineLayer {
    pub fn new(margin: Duration) -> Self {
        Self { margin }
    }
}

impl<S> Layer<S> for LambdaDeadlineLayer {
    type Service = LambdaDeadlineService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        LambdaDeadlineService {
            inner,
            margin: self.margin,
        }
    }
}

#[derive(Clone)]
pub struct LambdaDeadlineService<S> {
    inner: S,
    margin: Duration,
}

impl<S> Service<Request<AxumBody>> for LambdaDeadlineService<S>
where
    S: Service<Request<AxumBody>, Response = Response<AxumBody>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<AxumBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<AxumBody>) -> Self::Future {
        let ctx = req
            .extensions()
            .get::<LambdaContext>()
            .expect("LambdaContext missing from request extensions"); // @todo make error log, not panic

        let deadline: SystemTime = ctx.deadline();

        let fut = self.inner.call(req);
        let margin = self.margin;

        Box::pin(async move {
            let now = SystemTime::now();

            let deadline = match deadline.checked_sub(margin) {
                Some(d) => d,
                None => {
                    return Ok(Status::deadline_exceeded("Lambda deadline exceeded").into_http());
                }
            };

            let remaining = match deadline.duration_since(now) {
                Ok(d) => d,
                Err(_) => {
                    return Ok(Status::deadline_exceeded("Lambda deadline exceeded").into_http());
                }
            };

            let sleep = sleep_until(Instant::now() + remaining);

            tokio::select! {
                res = fut => res,
                _ = sleep => {
                    Ok(Status::deadline_exceeded("Lambda deadline exceeded").into_http())
                }
            }
        })
    }
}
