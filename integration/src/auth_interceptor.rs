//! this is a dummy scaffold for auth using tonic interceptors. it accepts all, and rejects only when
//! authorization is declared with explict value `reject`. NEVER use this in production, it does the
//! opposite of what you might want!

use tonic::{Request, Status};
use tonic::service::Interceptor;

#[derive(Clone)]
pub(crate) struct AuthInterceptor;

impl Interceptor for AuthInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        match request.metadata().get("authorization").map(|value| value.to_str()) {
            Some(Ok("reject")) => {
                Err(Status::permission_denied("requested to reject").into())
            }
            _ => Ok(request)
        }
    }
}