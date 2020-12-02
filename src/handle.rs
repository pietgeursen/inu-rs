use futures::channel::mpsc::SendError;
use futures::channel::mpsc::Sender;
use futures::prelude::*;

/// The handle you can use to stop an `Inu`
pub struct Handle {
    pub stopper: Sender<()>,
}

impl Handle {
    /// Stop the `Inu` instance.
    pub async fn stop(mut self) -> Result<(), SendError> {
        self.stopper.send(()).await
    }
}
