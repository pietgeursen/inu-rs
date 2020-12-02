use futures::Stream;
use std::pin::Pin;

/// Implement this trait for your application's state.  
pub trait State {
    type Action;
    type Effect;

    fn apply_action(&mut self, action: &Self::Action);
    fn apply_effect(&self, effect: &Self::Effect) -> Pin<Box<dyn Stream<Item = Self::Action>>>;
}
