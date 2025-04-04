use node::core::channels::mpsc;
use node::event_source::Event;

pub type EventSender = mpsc::UnboundedSender<Event>;

pub struct EventReceiver {
    rx: mpsc::UnboundedReceiver<Event>,
    queue: Vec<Event>,
}

impl EventReceiver {
    pub fn is_empty(&self) -> bool {
        !self.has_next()
    }

    pub fn len(&self) -> usize {
        self.rx.len() + self.queue.len()
    }

    /// If `Err(())`, `mpsc::Sender` for this channel was dropped.
    pub async fn wait_for_events(&mut self) -> Result<(), ()> {
        if !self.queue.is_empty() {
            return Ok(());
        }
        let next = self.rx.recv().await.ok_or(())?;
        self.queue.push(next);
        Ok(())
    }

    pub fn has_next(&self) -> bool {
        !self.queue.is_empty() || !self.rx.is_empty()
    }

    pub fn try_next(&mut self) -> Option<Event> {
        if !self.queue.is_empty() {
            Some(self.queue.remove(0))
        } else {
            self.rx.try_recv().ok()
        }
    }
}

impl From<mpsc::UnboundedReceiver<Event>> for EventReceiver {
    fn from(rx: mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            rx,
            queue: Vec::with_capacity(1),
        }
    }
}
