//! > A redux-like state manager based on [inu](https://github.com/ahdinosaur/inu).
//!
//! ## Example
//!
//! ```
//!    use inu_rs::*;
//!
//!    use async_std::{stream::interval, task::{block_on, spawn}};
//!    use futures::{FutureExt, Stream, StreamExt};
//!    use std::time::Duration;
//!    use std::pin::Pin;
//!
//!    #[derive(Copy, Clone, Debug)]
//!    struct MyState {
//!        count: i32,
//!    }
//!
//!    #[derive(Debug, Clone)]
//!    enum MyActions {
//!        TimerTicked,
//!    }
//!
//!    #[derive(Debug, Clone)]
//!    enum MyEffects {
//!        ScheduleTick(u64),
//!    }
//!
//!    impl State for MyState {
//!        type Action = MyActions;
//!        type Effect = MyEffects;
//!
//!        fn apply_action(&mut self, action: &Self::Action) {
//!            match action {
//!                MyActions::TimerTicked => self.count = self.count + 1,
//!            }
//!        }
//!        fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Option<Self::Action>>>> {
//!            match effect {
//!                MyEffects::ScheduleTick(tick_interval) => {
//!                    let interval = interval(Duration::from_millis(*tick_interval));
//!                    let stream = interval.take(2).map(|_| Some(MyActions::TimerTicked));
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
//!                .await
//!                .filter(|state| futures::future::ready(state.count >= 2))
//!                .into_future()
//!                .then(|_| async { inu_handle.stop().await.unwrap() });
//!
//!            let mut inu_handle = inu.get_handle();
//!            // You can dispatch actions and effects asynchronously from other tasks using the
//!            // handle.
//!            spawn(async move {
//!                 inu_handle.dispatch(None, Some(MyEffects::ScheduleTick(1)))
//!                     .await
//!                     .unwrap();
//!            });
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
pub use futures::channel::oneshot::{Receiver as OneshotReceiver, Sender as OneshotSender};
use futures::lock::Mutex;
use futures::Stream;
use futures::{SinkExt, StreamExt};
use std::collections::HashSet;
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

mod handle;
mod state;
mod state_sender;

pub use handle::*;
pub use state::*;
use state_sender::*;

/// The `Inu` state manager.
pub struct Inu<S: State> {
    state: Arc<Mutex<S>>,

    action_sender: UnboundedSender<S::Action>,
    action_receiver: Option<UnboundedReceiver<S::Action>>,

    effect_sender: UnboundedSender<S::Effect>,
    effect_receiver: Option<UnboundedReceiver<S::Effect>>,

    get_state_sender: UnboundedSender<OneshotSender<S>>,
    get_state_receiver: Option<UnboundedReceiver<OneshotSender<S>>>,

    state_subscribers: Arc<Mutex<HashSet<StateSender<S>>>>,
    state_subscriber_monotonic_counter: usize,

    stop_sender: Sender<()>,
    stop_receiver: Option<Receiver<()>>,
}

impl<S: State + Clone + Debug> Inu<S> {
    /// Create a new `Inu` instance. Note that you need to `run` it to drive the futures / streams
    /// to completion.
    pub fn new(initial_state: S) -> Inu<S> {
        let (action_sender, action_receiver) = unbounded();
        let (effect_sender, effect_receiver) = unbounded();
        let (get_state_sender, get_state_receiver) = unbounded();
        let (stop_sender, stop_receiver) = channel(1);

        Inu {
            state: Arc::new(Mutex::new(initial_state)),
            action_sender,
            action_receiver: Some(action_receiver),
            effect_sender,
            effect_receiver: Some(effect_receiver),
            state_subscribers: Arc::new(Mutex::new(HashSet::new())),
            state_subscriber_monotonic_counter: 0,
            stop_sender,
            stop_receiver: Some(stop_receiver),
            get_state_sender,
            get_state_receiver: Some(get_state_receiver),
        }
    }

    /// Get a `Handle` that can be used to stop the instance.
    pub fn get_handle(&mut self) -> Handle<S> {
        let stopper = self.stop_sender.clone();
        let action_sender = self.action_sender.clone();
        let effect_sender = self.effect_sender.clone();
        let get_state_sender = self.get_state_sender.clone();
        Handle::new(stopper, action_sender, effect_sender, get_state_sender)
    }

    /// Get the current `State`
    pub async fn get_state(&mut self) -> S {
        self.state.lock().await.clone()
    }

    /// Subscribe to changes to `State`
    pub async fn subscribe(&mut self) -> UnboundedReceiver<S> {
        let (sender, receiver) = unbounded();
        let state_sender = StateSender::new(sender, self.state_subscriber_monotonic_counter);
        self.state_subscriber_monotonic_counter += 1;
        self.state_subscribers.lock().await.insert(state_sender);
        receiver
    }

    /// Run the `Inu` instance
    pub async fn run(&mut self) {
        let action_receiver = self.action_receiver.take().unwrap();
        let effect_receiver = self.effect_receiver.take().unwrap();
        let stop_receiver = self.stop_receiver.take().unwrap();
        let get_state_receiver = self.get_state_receiver.take().unwrap();

        let (unsubscribe_sender, unsubscribe_receiver) = unbounded::<StateSender<S>>();

        let unsubscribe_stream = unsubscribe_receiver
            .map(|unsubscriber| (unsubscriber, self.state_subscribers.clone()))
            .for_each(|(unsubscriber, subscribers)| async move {
                subscribers.lock().await.remove(&unsubscriber);
            });

        let stopper_stream = stop_receiver.into_future();

        let get_state_stream = get_state_receiver
            .map(|sender| (sender, self.state.clone()))
            .for_each(|(sender, state)| async move {
                let state = state.lock().await.clone();
                sender.send(state).unwrap_or(());
            });

        let actions_stream = action_receiver
            .map(|action| {
                (
                    action,
                    &self.state_subscribers,
                    self.state.clone(),
                    unsubscribe_sender.clone(),
                )
            })
            .for_each(
                |(action, state_subscribers, state, unsubscribe_sender)| async move {
                    Self::update_state_from_action(state.clone(), &action).await;

                    let state_subscribers = state_subscribers.lock().await;
                    // Send the new state to all the state subscribers
                    futures::stream::iter(state_subscribers.iter())
                        .map(|state_subscriber| {
                            (state_subscriber, state.clone(), unsubscribe_sender.clone())
                        })
                        .for_each(
                            |(state_subscriber, state, mut unsubscribe_sender)| async move {
                                let current_state = state.lock().await.clone();
                                let send_result =
                                    state_subscriber.sender.clone().send(current_state).await;

                                if send_result.is_err() {
                                    unsubscribe_sender
                                        .send(state_subscriber.clone())
                                        .await
                                        .unwrap();
                                }
                            },
                        )
                        .await;
                },
            );

        let effects_stream = effect_receiver
            .map(|effect| (effect, self.action_sender.clone(), self.state.clone()))
            .for_each_concurrent(None, |(effect, action_sender, state)| async move {
                let actions_stream =
                    Self::get_action_stream_from_effect(state.clone(), &effect).await;

                actions_stream
                    .filter_map(|action| async { action })
                    .map(|action| (action, action_sender.clone()))
                    .for_each_concurrent(None, |(action, mut action_sender)| async move {
                        action_sender.send(action).await.unwrap();
                    })
                    .await
            });

        futures::select! {
            _ = Box::pin(stopper_stream) => (),
            _ = Box::pin(actions_stream) => (),
            _ = Box::pin(effects_stream) => (),
            _ = Box::pin(get_state_stream) => (),
            _ = Box::pin(unsubscribe_stream) => ()
        };
    }

    async fn update_state_from_action(state: Arc<Mutex<S>>, action: &S::Action) {
        let mut mutable_state = state.lock().await;
        mutable_state.apply_action(&action);
    }

    async fn get_action_stream_from_effect(
        state: Arc<Mutex<S>>,
        effect: &S::Effect,
    ) -> Pin<Box<dyn Stream<Item = Option<S::Action>>>> {
        let state = state.lock().await;
        state.apply_effect(&effect)
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    use async_std::{
        stream::interval,
        task::{block_on, spawn},
    };
    use futures::{stream::once, FutureExt, Stream, StreamExt};
    use std::pin::Pin;
    use std::time::Duration;

    #[derive(Copy, Clone, Debug)]
    struct MyState {
        count: i32,
        a_bool: bool,
    }

    #[derive(Debug, Clone)]
    enum MyActions {
        TimerTicked,
        SetBool,
    }

    #[derive(Debug, Clone)]
    enum MyEffects {
        ScheduleTick(u64),
        ScheduleSetBool,
    }

    impl State for MyState {
        type Action = MyActions;
        type Effect = MyEffects;

        fn apply_action(&mut self, action: &Self::Action) {
            match action {
                MyActions::TimerTicked => self.count = self.count + 1,
                MyActions::SetBool => self.a_bool = true,
            }
        }
        fn apply_effect(
            &self,
            effect: &Self::Effect,
        ) -> Pin<Box<dyn Stream<Item = Option<Self::Action>>>> {
            match effect {
                MyEffects::ScheduleTick(tick_interval) => {
                    let interval = interval(Duration::from_millis(*tick_interval));
                    let stream = interval.take(5).map(|_| Some(MyActions::TimerTicked));
                    Box::pin(stream)
                }
                MyEffects::ScheduleSetBool => Box::pin(once(async { Some(MyActions::SetBool) })),
            }
        }
    }

    #[test]
    fn it_works() {
        block_on(async {
            let initial_state = MyState {
                count: 0,
                a_bool: false,
            };
            let mut inu = Inu::new(initial_state);
            let mut inu_handle = inu.get_handle();
            let handle = inu.get_handle();

            spawn(async move {
                inu_handle
                    .dispatch(None, Some(MyEffects::ScheduleTick(5)))
                    .await
                    .unwrap();
            });

            let stopped = inu
                .subscribe()
                .await
                .filter(|state| futures::future::ready(state.count >= 2))
                .into_future()
                .then(|_| async { handle.stop().await.unwrap() });

            futures::join! {inu.run(), stopped };

            assert_eq!(inu.get_state().await.count, 2);
        });
    }

    #[test]
    fn subscribers_can_unsubscribe() {
        block_on(async {
            let initial_state = MyState {
                count: 0,
                a_bool: false,
            };
            let mut inu = Inu::new(initial_state);
            let mut inu_handle = inu.get_handle();
            let handle = inu_handle.clone();

            spawn(async move {
                inu_handle
                    .dispatch(None, Some(MyEffects::ScheduleTick(5)))
                    .await
                    .unwrap();
            });

            let stopped = inu
                .subscribe()
                .await
                .filter(|state| futures::future::ready(state.count >= 2))
                .into_future()
                .then(|_| async { handle.stop().await.unwrap() });

            // create a subscriber and close it immediately.
            inu.subscribe().await.close();

            futures::join! {inu.run(), stopped };

            assert_eq!(inu.get_state().await.count, 2);
        });
    }

    #[test]
    fn effects_resolve_concurrently() {
        block_on(async {
            let initial_state = MyState {
                count: 0,
                a_bool: false,
            };
            let mut inu = Inu::new(initial_state);
            let mut inu_handle = inu.get_handle();
            let handle = inu_handle.clone();

            spawn(async move {
                inu_handle
                    .dispatch(None, Some(MyEffects::ScheduleTick(5)))
                    .await
                    .unwrap();

                inu_handle
                    .dispatch(None, Some(MyEffects::ScheduleSetBool))
                    .await
                    .unwrap();
            });

            let stopped = inu
                .subscribe()
                .await
                .filter(|state| futures::future::ready(state.a_bool))
                .into_future()
                .then(|_| async move { handle.stop().await.unwrap() });

            futures::join! {inu.run(), stopped };

            assert!(inu.get_state().await.a_bool);
            assert_eq!(inu.get_state().await.count, 0);
        });
    }
}
