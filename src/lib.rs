//! > A redux-like state manager based on [inu](https://github.com/ahdinosaur/inu).
//!
//! ## Example
//!
//! ```
//!    use inu_rs::*;
//!
//!    use async_std::{stream::interval, task::block_on};
//!    use futures::{FutureExt, Stream, StreamExt};
//!    use std::time::Duration;
//!    use std::pin::Pin;
//!
//!    #[derive(Copy, Clone, Debug)]
//!    struct MyState {
//!        count: i32,
//!    }
//!
//!    enum MyActions {
//!        TimerTicked,
//!    }
//!
//!    enum MyEffects {
//!        ScheduleTick(u64),
//!    }
//!
//!    impl State for MyState {
//!        type Action = MyActions;
//!        type Effect = MyEffects;
//!
//!        fn apply_action(&self, action: &Self::Action) -> Self {
//!            match action {
//!                MyActions::TimerTicked => MyState {
//!                    count: self.count + 1,
//!                },
//!            }
//!        }
//!        fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Self::Action>>> {
//!            match effect {
//!                MyEffects::ScheduleTick(tick_interval) => {
//!                    let interval = interval(Duration::from_millis(*tick_interval));
//!                    let stream = interval.take(2).map(|_| MyActions::TimerTicked);
//!                    Box::pin(stream)
//!                }
//!            }
//!        }
//!    }
//!
//!    fn main() {
//!        block_on(async {
//!            let initial_state = MyState { count: 0 };
//!            let mut inu = Inu::new(initial_state);
//!            let inu_handle = inu.get_handle();
//!
//!            let stopped = inu
//!                .subscribe()
//!                .take_while(|state| futures::future::ready(state.count < 2))
//!                .into_future()
//!                .then(|_| async { inu_handle.stop().await.unwrap() });
//!
//!            inu.dispatch(None, Some(MyEffects::ScheduleTick(1)))
//!                .await
//!                .unwrap();
//!
//!
//!            futures::join! {inu.run(), stopped };
//!
//!            assert_eq!(inu.get_state().await.count, 2);
//!        });
//!    }
//! ```
//!
use futures::channel::mpsc::{channel, unbounded, UnboundedSender};
pub use futures::channel::mpsc::{Receiver, SendError, Sender, UnboundedReceiver};
use futures::lock::Mutex;
use futures::Stream;
use futures::{SinkExt, StreamExt};
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

/// Implement this trait for your application's state.  
pub trait State {
    type Action;
    type Effect;

    fn apply_action(&self, action: &Self::Action) -> Self;
    fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Self::Action>>>;
}

/// The handle you can use to stop an `Inu`
pub struct Handle {
    stopper: Sender<()>,
}

impl Handle {
    /// Stop the `Inu` instance.
    pub async fn stop(mut self) -> Result<(), SendError> {
        self.stopper.send(()).await
    }
}

/// The `Inu` state manager.
pub struct Inu<S: State> {
    state: Arc<Mutex<S>>,

    action_sender: UnboundedSender<S::Action>,
    action_receiver: Option<UnboundedReceiver<S::Action>>,

    effect_sender: UnboundedSender<S::Effect>,
    effect_receiver: Option<UnboundedReceiver<S::Effect>>,

    state_subscribers: Vec<UnboundedSender<S>>,

    stop_sender: Sender<()>,
    stop_receiver: Option<Receiver<()>>,
}

impl<S: State + Clone + Copy + Debug> Inu<S> {
    /// Create a new `Inu` instance. Note that you need to `run` it to drive the futures / streams
    /// to completion.
    pub fn new(initial_state: S) -> Inu<S> {
        let (action_sender, action_receiver) = unbounded();
        let (effect_sender, effect_receiver) = unbounded();
        let (stop_sender, stop_receiver) = channel(1);

        Inu {
            state: Arc::new(Mutex::new(initial_state)),
            action_sender,
            action_receiver: Some(action_receiver),
            effect_sender,
            effect_receiver: Some(effect_receiver),
            state_subscribers: Vec::new(),
            stop_sender,
            stop_receiver: Some(stop_receiver),
        }
    }

    /// Get a `Handle` that can be used to stop the instance.
    pub fn get_handle(&mut self) -> Handle {
        let stopper = self.stop_sender.clone();
        Handle { stopper }
    }

    /// Get the current `State`
    pub async fn get_state(&mut self) -> S {
        self.state.lock().await.clone()
    }

    /// Subscribe to changes to `State`
    pub fn subscribe(&mut self) -> UnboundedReceiver<S> {
        let (sender, receiver) = unbounded();
        self.state_subscribers.push(sender);

        receiver
    }

    /// Run the `Inu` instance
    pub async fn run(&mut self) {
        let action_receiver = self.action_receiver.take().unwrap();
        let effect_receiver = self.effect_receiver.take().unwrap();
        let stop_receiver = self.stop_receiver.take().unwrap();

        let action_sender = self.action_sender.clone();
        let effect_sender = self.effect_sender.clone();

        let stopper_stream = stop_receiver
            .take(1)
            .map(|_| (action_sender.clone(), effect_sender.clone()))
            .for_each(|(action_sender, effect_sender)| async move {
                action_sender.close_channel();
                effect_sender.close_channel();
            });

        let actions_stream = action_receiver
            .map(|action| (action, &self.state_subscribers, self.state.clone()))
            .for_each(|(action, state_subscribers, state)| async move {
                {
                    let mut mutable_state = state.lock().await;
                    *mutable_state = mutable_state.apply_action(&action);
                }

                // Send the new state to all the state subscribers
                futures::stream::iter(state_subscribers)
                    .map(|state_subscriber| (state_subscriber, state.clone()))
                    .for_each(|(mut state_subscriber, state)| async move {
                        let current_state = state.lock().await.clone();
                        state_subscriber.send(current_state).await.unwrap();
                    })
                    .await;
            });

        let effects_stream = effect_receiver
            .map(|effect| (effect, self.action_sender.clone(), self.state.clone()))
            .for_each_concurrent(None, |(effect, action_sender, state)| async move {
                let state = state.lock().await;
                let actions_stream = state.apply_effect(&effect);

                actions_stream
                    .map(|action| (action, action_sender.clone()))
                    .for_each_concurrent(None, |(action, mut action_sender)| async move {
                        action_sender.send(action).await.unwrap();
                    })
                    .await
            });

        futures::join! {actions_stream, effects_stream, stopper_stream};
    }

    /// Dispatch `State::Action`s or `State::Effect`s
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
    use crate::*;

    use async_std::{stream::interval, task::block_on};
    use futures::{FutureExt, StreamExt};
    use std::time::Duration;

    #[derive(Copy, Clone, Debug)]
    struct MyState {
        count: i32,
    }

    enum MyActions {
        TimerTicked,
    }

    enum MyEffects {
        ScheduleTick(u64),
    }

    impl State for MyState {
        type Action = MyActions;
        type Effect = MyEffects;

        fn apply_action(&self, action: &Self::Action) -> Self {
            match action {
                MyActions::TimerTicked => MyState {
                    count: self.count + 1,
                },
            }
        }
        fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Self::Action>>> {
            match effect {
                MyEffects::ScheduleTick(tick_interval) => {
                    let interval = interval(Duration::from_millis(*tick_interval));
                    let stream = interval.take(2).map(|_| MyActions::TimerTicked);
                    Box::pin(stream)
                }
            }
        }
    }

    #[test]
    fn it_works() {
        block_on(async {
            let initial_state = MyState { count: 0 };
            let mut inu = Inu::new(initial_state);
            let inu_handle = inu.get_handle();

            let stopped = inu
                .subscribe()
                .take_while(|state| futures::future::ready(state.count < 2))
                .into_future()
                .then(|_| async { inu_handle.stop().await.unwrap() });

            inu.dispatch(None, Some(MyEffects::ScheduleTick(1)))
                .await
                .unwrap();

            futures::join! {inu.run(), stopped };

            assert_eq!(inu.get_state().await.count, 2);
        });
    }
    #[test]
    fn subscribers_can_unsubscribe(){}

    #[test]
    fn effects_resolve_concurrently(){}
}
