use lambda_http::http::{Request, Response};
use lambda_http::tracing::log::{error, info, warn};
use lambda_runtime::Context as LambdaContext;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::time::{Instant, sleep_until};
use tonic::Status;
use tower::{Layer, Service};

#[derive(Clone, Default)]
pub(crate) struct LambdaDeadlineLayer {
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
pub(crate) struct LambdaDeadlineService<S> {
    inner: S,
    margin: Duration,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for LambdaDeadlineService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let ctx = req.extensions().get::<LambdaContext>();

        let deadline: Option<SystemTime> = ctx.map(|c| c.deadline());

        let fut = self.inner.call(req);
        let margin = self.margin;

        Box::pin(async move {
            let now = SystemTime::now();

            let Some(deadline) = deadline else {
                warn!(
                    "lambda Context missing from request extension. Deadline cannot be determined, continuing..."
                );
                return fut.await;
            };

            let Some(deadline) = deadline.checked_sub(margin) else {
                error!("Unexpected time offset failure. Continuing request...");
                return fut.await;
            };

            let Ok(remaining) = deadline.duration_since(now) else {
                error!("Clock may have gone backwards. Continuing request...");
                return fut.await;
            };

            let sleep = sleep_until(Instant::now() + remaining);

            tokio::select! {
                res = fut => res,
                _ = sleep => {
                    info!("Lambda request deadline imminent, terminating request with `deadline_exceeded`");
                    Ok(Status::deadline_exceeded("Lambda deadline exceeded").into_http())
                }
            }
        })
    }
}
