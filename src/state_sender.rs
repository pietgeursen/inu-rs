use std::hash::Hasher;
use std::hash::Hash;
use futures::channel::mpsc::UnboundedSender;
use crate::State;

#[derive(Clone)]
pub struct StateSender<S: State> {
    id: usize,
    pub sender: UnboundedSender<S>,
}

impl<S: State> StateSender<S> {
    pub fn new(sender: UnboundedSender<S>, id: usize) -> StateSender<S> {
        StateSender {
            id,
            sender,
        }
    }
}

impl<S: State> PartialEq for StateSender<S> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<S: State> Eq for StateSender<S> {}

impl<S: State> Hash for StateSender<S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}
