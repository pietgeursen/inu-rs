use crate::State;
use futures::channel::mpsc::{SendError, Sender, UnboundedSender};
use futures::channel::oneshot::{channel, Sender as OneshotSender};
use futures::prelude::*;

/// The handle you can use to stop an `Inu`
#[derive(Clone)]
pub struct Handle<S: State> {
    pub stopper: Sender<()>,
    pub action_sender: UnboundedSender<S::Action>,
    pub effect_sender: UnboundedSender<S::Effect>,
    pub state_get_sender: UnboundedSender<OneshotSender<S>>,
}

impl<S: State> Handle<S> {
    pub fn new(
        stopper: Sender<()>,
        action_sender: UnboundedSender<S::Action>,
        effect_sender: UnboundedSender<S::Effect>,
        state_get_sender: UnboundedSender<OneshotSender<S>>,
    ) -> Handle<S> {
        Handle {
            stopper,
            action_sender,
            effect_sender,
            state_get_sender,
        }
    }
    /// Stop the `Inu` instance.
    pub async fn stop(mut self) -> Result<(), SendError> {
        self.stopper.send(()).await
    }

    /// Get the inu state
    pub async fn get_state(&self) -> S {
        let (sender, receiver) = channel();
        self.state_get_sender.clone().send(sender).await.unwrap();
        receiver.await.unwrap()
    }

    /// Dispatch `State::Action`s and / or `State::Effect`s
    pub async fn dispatch(
        &mut self,
        action: Option<S::Action>,
        effect: Option<S::Effect>,
    ) -> Result<(), SendError> {
        if let Some(action) = action {
            self.action_sender.send(action).await?
        }
        if let Some(effect) = effect {
            self.effect_sender.send(effect).await?
        }
        Ok(())
    }
}
