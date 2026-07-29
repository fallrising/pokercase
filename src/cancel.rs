//! Cancel upstream work when the client disconnects.

use tokio_util::sync::CancellationToken;

/// RAII: cancels the token when dropped, unless [`disarm`](Self::disarm) was called.
///
/// Use during request handling (forward). For streaming responses, **disarm** before
/// returning the body so the stream is not aborted when the handler completes.
pub struct CancelOnDrop {
    token: CancellationToken,
    armed: bool,
}

impl CancelOnDrop {
    pub fn new() -> (Self, CancellationToken) {
        let token = CancellationToken::new();
        (
            Self {
                token: token.clone(),
                armed: true,
            },
            token,
        )
    }

    /// Prevent cancel on drop (e.g. about to return a long-lived SSE body).
    pub fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}
