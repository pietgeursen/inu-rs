use futures::channel::mpsc::*;
use futures::lock::Mutex;
use futures::Future;
use futures::Stream;
use futures::{SinkExt, StreamExt};
use std::pin::Pin;
use std::sync::Arc;

pub trait State {
    type Action;
    type Effect;

    fn apply_action(&self, action: &Self::Action) -> Self;
    fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Self::Action>>>;
}

pub type Subscriber<State> = Box<dyn Fn(&State) -> ()>;

pub trait Redux<S: State> {
    fn dispatch(&mut self, action: &S::Action);
    fn get_state(&mut self) -> &S;
    fn subscribe(&mut self, subscriber: Subscriber<S>);
}

pub struct Inu<S: State> {
    initial_state: S,
    state: Arc<Mutex<S>>,
    action_sender: UnboundedSender<S::Action>,
    action_receiver: Option<UnboundedReceiver<S::Action>>,

    state_subscribers: Vec<UnboundedSender<S>>,

    effect_sender: UnboundedSender<S::Effect>,
    effect_receiver: Option<UnboundedReceiver<S::Effect>>,
}

impl<S: State + Clone + Copy> Inu<S> {
    pub fn new(initial_state: S) -> Inu<S> {
        //let actions_channel = unbounded();
        unimplemented!();
        //Inu { actions_channel, state: initial_state }
    }

    pub async fn subscribe_actions() {}
    pub async fn subscribe_effects() {}

    pub async fn run(&mut self) {
        let action_receiver = self.action_receiver.take().unwrap();
        let effect_receiver = self.effect_receiver.take().unwrap();
        let initial_state = self.initial_state.clone();

        action_receiver
            .map(|action| (action, &self.state_subscribers, self.state.clone()))
            .for_each(|(action, state_subscribers, state)| async move {
                {
                    let mut mutable_state = state.lock().await;
                    *mutable_state = initial_state.apply_action(&action);
                }

                // Send the new state to all the state subscribers
                futures::stream::iter(state_subscribers)
                    .map(|action_subscriber| (action_subscriber, state.clone()))
                    .for_each(|(mut action_subscriber, state)| async move {
                        let current_state = state.lock().await;
                        action_subscriber.send(current_state.clone()).await.unwrap();
                    })
                    .await;
            })
            .await;

        effect_receiver
            .map(|effect| (effect, self.action_sender.clone(), self.state.clone()))
            .for_each(|(effect, action_sender, state)| async move {
                let state = state.lock().await;
                let actions_stream = state.apply_effect(&effect);

                actions_stream
                    .map(|action|(action, action_sender.clone()))
                    .for_each(|(action, mut action_sender)| async move { action_sender.send(action).await.unwrap(); }).await
            }).await;
    }

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

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
