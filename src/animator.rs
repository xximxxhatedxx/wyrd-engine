//! Animator: Spring and Easing interpolation.

use serde::{Deserialize, Serialize};
use slotmap::{new_key_type, SlotMap};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnimationConfig {
    pub kind: String, // "spring" | "easing"
    #[serde(default)]
    pub stiffness: Option<f32>, // spring only
    #[serde(default)]
    pub damping: Option<f32>, // spring only
    #[serde(default)]
    pub duration_ms: Option<u32>, // easing only
    #[serde(default)]
    pub curve: Option<String>, // easing only
    #[serde(default)]
    pub from: Option<AnimationFrom>,
    #[serde(default)]
    pub to: Option<AnimationFrom>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnimationFrom {
    #[serde(default)]
    pub x: Option<f32>,
    #[serde(default)]
    pub y: Option<f32>,
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub scale: Option<f32>,
}

pub fn default_animation_presets() -> HashMap<String, AnimationConfig> {
    let mut map = HashMap::new();
    map.insert(
        "@popup_open".to_string(),
        AnimationConfig {
            kind: "spring".to_string(),
            stiffness: Some(300.0),
            damping: Some(26.0),
            duration_ms: None,
            curve: None,
            from: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(-12.0),
                opacity: Some(0.0),
                scale: Some(1.0),
            }),
            to: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(0.0),
                opacity: Some(1.0),
                scale: Some(1.0),
            }),
        },
    );
    map.insert(
        "@popup_close".to_string(),
        AnimationConfig {
            kind: "spring".to_string(),
            stiffness: Some(300.0),
            damping: Some(26.0),
            duration_ms: None,
            curve: None,
            from: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(0.0),
                opacity: Some(1.0),
                scale: Some(1.0),
            }),
            to: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(-12.0),
                opacity: Some(0.0),
                scale: Some(1.0),
            }),
        },
    );
    map.insert(
        "@popup_open_left_bar".to_string(),
        AnimationConfig {
            kind: "spring".to_string(),
            stiffness: Some(300.0),
            damping: Some(26.0),
            duration_ms: None,
            curve: None,
            from: Some(AnimationFrom {
                x: Some(-12.0),
                y: Some(0.0),
                opacity: Some(0.0),
                scale: Some(1.0),
            }),
            to: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(0.0),
                opacity: Some(1.0),
                scale: Some(1.0),
            }),
        },
    );
    map.insert(
        "@popup_open_bottom_bar".to_string(),
        AnimationConfig {
            kind: "spring".to_string(),
            stiffness: Some(300.0),
            damping: Some(26.0),
            duration_ms: None,
            curve: None,
            from: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(12.0),
                opacity: Some(0.0),
                scale: Some(1.0),
            }),
            to: Some(AnimationFrom {
                x: Some(0.0),
                y: Some(0.0),
                opacity: Some(1.0),
                scale: Some(1.0),
            }),
        },
    );
    map
}

new_key_type! {
    pub struct AnimationHandle;
}

/// Active animation registry.
pub struct Animator {
    animations: SlotMap<AnimationHandle, Animation>,
    named_animations: HashMap<String, Animation>,
}

impl Default for Animator {
    fn default() -> Self {
        Self::new()
    }
}

impl Animator {
    pub fn new() -> Self {
        Self {
            animations: SlotMap::with_key(),
            named_animations: HashMap::new(),
        }
    }

    pub fn has_active(&self) -> bool {
        !self.animations.is_empty() || !self.named_animations.is_empty()
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let mut needs_frame = false;
        self.animations.retain(|_handle, anim| {
            needs_frame = true;

            anim.tick(now)
        });
        self.named_animations.retain(|_, anim| {
            needs_frame = true;

            anim.tick(now)
        });
        needs_frame
    }

    pub fn start_named_spring(
        &mut self,
        name: &str,
        initial: f64,
        target: f64,
        stiffness: f64,
        damping: f64,
    ) {
        self.named_animations.insert(
            name.to_string(),
            Animation {
                kind: AnimationKind::Spring {
                    stiffness,
                    damping,
                    velocity: 0.0,
                },
                value: initial,
                target,
                start: Instant::now(),
                finished: false,
            },
        );
    }

    pub fn start_named_easing(
        &mut self,
        name: &str,
        initial: f64,
        target: f64,
        duration: Duration,
        curve: EasingCurve,
    ) {
        self.named_animations.insert(
            name.to_string(),
            Animation {
                kind: AnimationKind::Easing { duration, curve },
                value: initial,
                target,
                start: Instant::now(),
                finished: false,
            },
        );
    }

    pub fn get_named(&self, name: &str) -> Option<f64> {
        self.named_animations.get(name).map(|anim| anim.value)
    }

    pub fn get_named_or(&self, name: &str, default: f64) -> f64 {
        self.named_animations
            .get(name)
            .map(|anim| anim.value)
            .unwrap_or(default)
    }

    pub fn is_named_active(&self, name: &str) -> bool {
        self.named_animations
            .get(name)
            .is_some_and(|anim| !anim.finished)
    }

    pub fn remove_named(&mut self, name: &str) {
        self.named_animations.remove(name);
    }

    pub fn start_spring(&mut self, target: f64, stiffness: f64, damping: f64) -> AnimationHandle {
        self.animations.insert(Animation {
            kind: AnimationKind::Spring {
                stiffness,
                damping,
                velocity: 0.0,
            },
            value: 0.0,
            target,
            start: Instant::now(),
            finished: false,
        })
    }

    pub fn start_easing(
        &mut self,
        target: f64,
        duration: Duration,
        curve: EasingCurve,
    ) -> AnimationHandle {
        self.animations.insert(Animation {
            kind: AnimationKind::Easing { duration, curve },
            value: 0.0,
            target,
            start: Instant::now(),
            finished: false,
        })
    }

    pub fn start_staggered(
        &mut self,
        target: f64,
        duration: Duration,
        delay: Duration,
        curve: EasingCurve,
    ) -> AnimationHandle {
        self.animations.insert(Animation {
            kind: AnimationKind::Easing { duration, curve },
            value: 0.0,
            target,
            start: Instant::now() + delay,
            finished: false,
        })
    }

    pub fn start_morph(
        &mut self,
        from: [f64; 2],
        to: [f64; 2],
        duration: Duration,
        curve: EasingCurve,
    ) -> MorphHandle {
        let first = self.start_easing(to[0] - from[0], duration, curve);
        let second = self.start_easing(to[1] - from[1], duration, curve);
        MorphHandle {
            first,
            second,
            from,
            to,
        }
    }

    pub fn get(&self, handle: AnimationHandle) -> Option<f64> {
        self.animations.get(handle).map(|anim| anim.value)
    }

    pub fn is_active(&self, handle: AnimationHandle) -> bool {
        self.animations
            .get(handle)
            .is_some_and(|anim| !anim.finished)
    }

    pub fn stop(&mut self, handle: AnimationHandle) {
        self.animations.remove(handle);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MorphHandle {
    pub first: AnimationHandle,
    pub second: AnimationHandle,
    pub from: [f64; 2],
    pub to: [f64; 2],
}

struct Animation {
    kind: AnimationKind,
    value: f64,
    target: f64,
    start: Instant,
    finished: bool,
}

enum AnimationKind {
    Spring {
        stiffness: f64,
        damping: f64,
        velocity: f64,
    },
    Easing {
        duration: Duration,
        curve: EasingCurve,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingCurve {
    Linear,
    EaseInOutQuad,
    EaseOutCubic,
}

impl Animation {
    fn tick(&mut self, now: Instant) -> bool {
        if self.finished {
            return false;
        }

        if now < self.start {
            return true;
        }

        match &mut self.kind {
            AnimationKind::Spring {
                stiffness,
                damping,
                velocity,
            } => {
                let dt = now
                    .duration_since(self.start)
                    .as_secs_f64()
                    .clamp(0.0001, 0.1);
                self.start = now;
                let displacement = self.target - self.value;
                let spring_force = displacement * *stiffness;
                let damping_force = *velocity * *damping;
                let acceleration = spring_force - damping_force;
                *velocity += acceleration * dt;
                self.value += *velocity * dt;

                if displacement.abs() < 0.01 && velocity.abs() < 0.01 {
                    self.value = self.target;
                    self.finished = true;
                }
            }
            AnimationKind::Easing { duration, curve } => {
                let t = if duration.is_zero() {
                    1.0
                } else {
                    let elapsed = now.duration_since(self.start).as_secs_f64();
                    (elapsed / duration.as_secs_f64()).min(1.0)
                };
                let eased = match curve {
                    EasingCurve::Linear => t,
                    EasingCurve::EaseInOutQuad => {
                        if t < 0.5 {
                            2.0 * t * t
                        } else {
                            1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                        }
                    }
                    EasingCurve::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
                };
                self.value = self.target * eased;
                if t >= 1.0 {
                    self.finished = true;
                }
            }
        }

        !self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_ticks_until_completion() {
        let mut animator = Animator::new();
        animator.start_easing(100.0, Duration::from_millis(100), EasingCurve::Linear);

        assert!(animator.has_active());
        assert!(animator.tick(Instant::now() + Duration::from_millis(50)));
        assert!(animator.has_active());
        assert!(animator.tick(Instant::now() + Duration::from_millis(150)));
        assert!(!animator.has_active());
    }

    #[test]
    fn zero_duration_easing_finishes_without_division_by_zero() {
        let mut animator = Animator::new();
        animator.start_easing(1.0, Duration::ZERO, EasingCurve::Linear);

        assert!(animator.tick(Instant::now()));
        assert!(!animator.has_active());
    }

    #[test]
    fn staggered_animation_waits_for_its_delay() {
        let mut animator = Animator::new();
        let start = Instant::now();
        animator.start_staggered(
            1.0,
            Duration::from_millis(100),
            Duration::from_millis(50),
            EasingCurve::Linear,
        );

        assert!(animator.tick(start + Duration::from_millis(25)));
        assert!(animator.tick(start + Duration::from_millis(100)));
        assert!(animator.tick(start + Duration::from_millis(300)));
        assert!(!animator.has_active());
    }

    #[test]
    fn morph_registers_both_axes() {
        let mut animator = Animator::new();
        let morph = animator.start_morph(
            [0.0, 0.0],
            [100.0, 40.0],
            Duration::from_millis(10),
            EasingCurve::Linear,
        );

        assert_ne!(morph.first, morph.second);
        assert!(animator.has_active());
    }

    #[test]
    fn handles_remain_valid_after_other_animation_removed() {
        let mut animator = Animator::new();
        let h1 = animator.start_easing(50.0, Duration::from_millis(20), EasingCurve::Linear);
        let h2 = animator.start_easing(100.0, Duration::from_millis(200), EasingCurve::Linear);
        let h3 = animator.start_easing(150.0, Duration::from_millis(300), EasingCurve::Linear);

        // Advance past h1 duration so h1 finishes and gets pruned
        animator.tick(Instant::now() + Duration::from_millis(50));
        assert!(!animator.is_active(h1));
        assert!(animator.is_active(h2));
        assert!(animator.is_active(h3));

        // Explicitly stop h2
        animator.stop(h2);
        assert!(!animator.is_active(h2));
        assert!(animator.is_active(h3));
        assert!(animator.get(h3).is_some());
    }
}
