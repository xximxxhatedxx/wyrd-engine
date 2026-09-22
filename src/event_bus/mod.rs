//! Event Bus: broadcast, watch, mpsc channels and named topics.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::{broadcast, mpsc, watch};

type WatchPair = (watch::Sender<BusEvent>, watch::Receiver<BusEvent>);
type MpscPair = (mpsc::Sender<BusEvent>, Option<mpsc::Receiver<BusEvent>>);

/// Event Bus with typed channel semantics and named pub/sub topics.
pub struct EventBus {
    broadcast: Mutex<HashMap<String, broadcast::Sender<BusEvent>>>,
    watch: Mutex<HashMap<String, WatchPair>>,
    mpsc: Mutex<HashMap<String, MpscPair>>,
    topics: Mutex<HashMap<String, broadcast::Sender<Value>>>,
}

#[derive(Debug, Clone)]
pub enum BusEvent {
    ThemeChanged,
    ConfigReloaded,
    ModuleUpdate { module: String, payload: Value },
    SurfaceDamage { surface_id: String },
    PointerEvent { x: f64, y: f64 },
    KeyEvent { keysym: u32 },
    AnimationTick,
    TopicEvent { topic: String, value: Value },
    Shutdown,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            broadcast: Mutex::new(HashMap::new()),
            watch: Mutex::new(HashMap::new()),
            mpsc: Mutex::new(HashMap::new()),
            topics: Mutex::new(HashMap::new()),
        }
    }

    /// Create or get broadcast channel (fan-out).
    pub fn broadcast_channel(&self, name: &str, capacity: usize) -> broadcast::Sender<BusEvent> {
        self.broadcast
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| broadcast::channel(capacity).0)
            .clone()
    }

    /// Create or get watch channel (last-value).
    pub fn watch_channel(&self, name: &str, initial: BusEvent) -> watch::Sender<BusEvent> {
        self.watch
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| watch::channel(initial.clone()))
            .0
            .clone()
    }

    /// Create or get mpsc channel (point-to-point).
    pub fn mpsc_channel(&self, name: &str, capacity: usize) -> mpsc::Sender<BusEvent> {
        self.mpsc
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| {
                let (sender, receiver) = mpsc::channel(capacity);
                (sender, Some(receiver))
            })
            .0
            .clone()
    }

    pub fn take_mpsc_receiver(&self, name: &str) -> Option<mpsc::Receiver<BusEvent>> {
        self.mpsc.lock().unwrap().get_mut(name)?.1.take()
    }

    pub fn subscribe_broadcast(&self, name: &str) -> Option<broadcast::Receiver<BusEvent>> {
        self.broadcast
            .lock()
            .unwrap()
            .get(name)
            .map(|tx| tx.subscribe())
    }

    pub fn subscribe_watch(&self, name: &str) -> Option<watch::Receiver<BusEvent>> {
        self.watch
            .lock()
            .unwrap()
            .get(name)
            .map(|(_, rx)| rx.clone())
    }

    /// Publish a message to a named topic.
    pub fn publish_topic(&self, topic: &str, value: Value) -> usize {
        let mut topics = self.topics.lock().unwrap();
        let tx = topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(32).0);
        tx.send(value).unwrap_or(0)
    }

    /// Subscribe to a named topic.
    pub fn subscribe_topic(&self, topic: &str) -> broadcast::Receiver<Value> {
        let mut topics = self.topics.lock().unwrap();
        let tx = topics
            .entry(topic.to_string())
            .or_insert_with(|| broadcast::channel(32).0);
        tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn channels_keep_their_declared_delivery_semantics() {
        let bus = EventBus::new();

        let broadcast = bus.broadcast_channel("theme", 4);
        let mut first = bus.subscribe_broadcast("theme").unwrap();
        let mut second = bus.subscribe_broadcast("theme").unwrap();
        broadcast.send(BusEvent::ThemeChanged).unwrap();
        assert!(matches!(
            first.recv().await.unwrap(),
            BusEvent::ThemeChanged
        ));
        assert!(matches!(
            second.recv().await.unwrap(),
            BusEvent::ThemeChanged
        ));

        let watch = bus.watch_channel("volume", BusEvent::KeyEvent { keysym: 0 });
        let mut latest = bus.subscribe_watch("volume").unwrap();
        watch.send(BusEvent::KeyEvent { keysym: 10 }).unwrap();
        watch.send(BusEvent::KeyEvent { keysym: 20 }).unwrap();
        latest.changed().await.unwrap();
        assert!(matches!(
            *latest.borrow(),
            BusEvent::KeyEvent { keysym: 20 }
        ));

        let commands = bus.mpsc_channel("commands", 2);
        let mut receiver = bus.take_mpsc_receiver("commands").unwrap();
        commands.send(BusEvent::Shutdown).await.unwrap();
        assert!(matches!(receiver.recv().await, Some(BusEvent::Shutdown)));

        // Test topic pub/sub
        let mut topic_sub = bus.subscribe_topic("theme.accent");
        bus.publish_topic("theme.accent", serde_json::json!("#ff0055"));
        let received = topic_sub.recv().await.unwrap();
        assert_eq!(received, serde_json::json!("#ff0055"));
    }
}
