//! Everything the actions show on the deck or send to a property inspector
//! goes through `Ui`, addressed by instance id. The real implementation
//! looks the `openaction::Instance` up; tests use a recording fake, so the
//! action logic runs under test without a live OpenDeck.

use crate::actions::Control;
use crate::controller::Controller;
use crate::render::level::{self, LevelView};
use crate::render::profile::{self, ProfileView};
use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[async_trait]
pub trait Ui: Send + Sync {
    async fn show_level(&self, control: &Control, view: &LevelView);
    async fn show_profile(&self, control: &Control, view: &ProfileView);
    async fn alert(&self, id: &str);
    async fn send_to_pi(&self, id: &str, payload: Value);
    /// The instance (re)appeared or disappeared: its next frame must be sent
    /// even if it matches the last one.
    fn forget(&self, id: &str);
}

/// What one instance is shown: a key image or a touch-strip payload.
enum Frame {
    Image(String),
    Feedback(Value),
}

impl Frame {
    fn digest(&self) -> u64 {
        let mut h = DefaultHasher::new();
        match self {
            Frame::Image(s) => s.hash(&mut h),
            Frame::Feedback(v) => v.to_string().hash(&mut h),
        }
        h.finish()
    }
}

/// Sends through openaction, skipping a frame identical to the one the
/// instance already shows (a re-render after an unrelated change is free).
#[derive(Default)]
pub struct OpenDeckUi {
    last: DashMap<String, u64>,
}

impl OpenDeckUi {
    async fn show(&self, control: &Control, frame: Frame) {
        let digest = frame.digest();
        let first = match self.last.get(&control.id) {
            Some(d) if *d == digest => return,
            Some(_) => false,
            None => true,
        };
        let Some(instance) = openaction::get_instance(control.id.clone()).await else {
            return;
        };
        let result = match frame {
            Frame::Image(image) => {
                // Text is baked into the image; clear the native title once
                // per appearance so OpenDeck doesn't paint a second copy.
                if first && let Err(e) = instance.set_title(Some(String::new()), None).await {
                    log::warn!("set_title failed: {e}");
                }
                instance.set_image(Some(image), None).await
            }
            Frame::Feedback(payload) => instance.set_feedback(&payload).await,
        };
        match result {
            Ok(()) => {
                self.last.insert(control.id.clone(), digest);
            }
            Err(e) => {
                log::warn!("render failed for {}: {e}", control.id);
                self.last.remove(&control.id);
            }
        }
    }
}

#[async_trait]
impl Ui for OpenDeckUi {
    async fn show_level(&self, control: &Control, view: &LevelView) {
        let frame = match control.controller {
            Controller::Keypad => Frame::Image(level::tile_image(view)),
            Controller::Encoder => Frame::Feedback(level::strip_feedback(view)),
        };
        self.show(control, frame).await;
    }

    async fn show_profile(&self, control: &Control, view: &ProfileView) {
        let frame = match control.controller {
            Controller::Keypad => Frame::Image(profile::tile_image(view)),
            Controller::Encoder => Frame::Feedback(profile::strip_feedback(view)),
        };
        self.show(control, frame).await;
    }

    async fn alert(&self, id: &str) {
        if let Some(instance) = openaction::get_instance(id.to_string()).await
            && let Err(e) = instance.show_alert().await
        {
            log::warn!("show_alert failed for {id}: {e}");
        }
    }

    async fn send_to_pi(&self, id: &str, payload: Value) {
        if let Some(instance) = openaction::get_instance(id.to_string()).await
            && let Err(e) = instance.send_to_property_inspector(payload).await
        {
            log::warn!("send_to_property_inspector failed for {id}: {e}");
        }
    }

    fn forget(&self, id: &str) {
        self.last.remove(id);
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::sync::Mutex;

    /// Records what would have been shown, one readable line per call.
    #[derive(Default)]
    pub struct FakeUi {
        events: Mutex<Vec<String>>,
    }

    impl FakeUi {
        pub fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        pub fn alerts(&self) -> usize {
            self.events()
                .iter()
                .filter(|e| e.starts_with("alert "))
                .count()
        }

        pub fn clear(&self) {
            self.events.lock().unwrap().clear();
        }

        fn push(&self, e: String) {
            self.events.lock().unwrap().push(e);
        }
    }

    #[async_trait]
    impl Ui for FakeUi {
        async fn show_level(&self, control: &Control, view: &LevelView) {
            self.push(format!("level {} {}", control.id, view.value_text));
        }
        async fn show_profile(&self, control: &Control, view: &ProfileView) {
            self.push(format!(
                "profile {} {} {}",
                control.id, view.shown, view.hint
            ));
        }
        async fn alert(&self, id: &str) {
            self.push(format!("alert {id}"));
        }
        async fn send_to_pi(&self, id: &str, payload: Value) {
            self.push(format!(
                "pi {id} {}",
                payload["event"].as_str().unwrap_or("")
            ));
        }
        fn forget(&self, _id: &str) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_frames_have_identical_digests() {
        let a = Frame::Feedback(serde_json::json!({"value": "45%"}));
        let b = Frame::Feedback(serde_json::json!({"value": "45%"}));
        let c = Frame::Feedback(serde_json::json!({"value": "50%"}));
        assert_eq!(a.digest(), b.digest());
        assert_ne!(a.digest(), c.digest());
    }
}
