use std::hash::Hasher;
use std::hash::Hash;
use futures::channel::mpsc::UnboundedSender;
use uuid::Uuid;
use crate::State;

#[derive(Clone)]
pub struct StateSender<S: State> {
    uuid: Uuid,
    pub sender: UnboundedSender<S>,
}

impl<S: State> StateSender<S> {
    pub fn new(sender: UnboundedSender<S>) -> StateSender<S> {
        StateSender {
            uuid: Uuid::new_v4(),
            sender,
        }
    }
}

impl<S: State> PartialEq for StateSender<S> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<S: State> Eq for StateSender<S> {}

impl<S: State> Hash for StateSender<S> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}
