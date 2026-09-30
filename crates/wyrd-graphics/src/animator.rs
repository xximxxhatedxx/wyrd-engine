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

new_key_type! {
    pub struct AnimationHandle;
}

struct ColorAnimation {
    from: [f32; 4],
    current: [f32; 4],
    target: [f32; 4],
    progress: f64,
    velocity: f64,
    stiffness: f64,
    damping: f64,
    start: Instant,
    finished: bool,
}

impl ColorAnimation {
    fn tick(&mut self, now: Instant) -> bool {
        if self.finished {
            return false;
        }
        let total_dt = now
            .duration_since(self.start)
            .as_secs_f64()
            .clamp(0.0001, 0.020);
        self.start = now;
        let steps = ((total_dt / 0.004).ceil() as usize).clamp(1, 8);
        let step_dt = total_dt / steps as f64;
        for _ in 0..steps {
            let displacement = 1.0 - self.progress;
            let accel = displacement * self.stiffness - self.velocity * self.damping;
            self.velocity += accel * step_dt;
            self.progress += self.velocity * step_dt;
        }
        if (1.0 - self.progress).abs() < 0.01 && self.velocity.abs() < 0.02 {
            self.progress = 1.0;
            self.current = self.target;
            self.finished = true;
        } else {
            let p = self.progress.clamp(0.0, 1.0) as f32;
            for i in 0..4 {
                self.current[i] =
                    (self.from[i] + (self.target[i] - self.from[i]) * p).clamp(0.0, 1.0);
            }
        }
        !self.finished
    }
}

pub struct Animator {
    animations: SlotMap<AnimationHandle, Animation>,
    named_animations: HashMap<String, Animation>,
    settled_values: HashMap<String, (f64, u64)>,
    settled_colors: HashMap<String, ([f32; 4], u64)>,
    color_animations: HashMap<String, ColorAnimation>,
    tick_gen: u64,
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
            settled_values: HashMap::new(),
            settled_colors: HashMap::new(),
            color_animations: HashMap::new(),
            tick_gen: 0,
        }
    }

    pub fn has_active(&self) -> bool {
        !self.animations.is_empty()
            || !self.named_animations.is_empty()
            || !self.color_animations.is_empty()
    }

    /// Returns true if any named or color animation is active for the given surface prefix.
    pub fn has_active_for_prefix(&self, prefix: &str) -> bool {
        self.named_animations
            .iter()
            .any(|(k, a)| !a.finished && k.starts_with(prefix))
            || self
                .color_animations
                .iter()
                .any(|(k, a)| !a.finished && k.starts_with(prefix))
    }

    pub fn clear_prefix(&mut self, prefix: &str) {
        self.named_animations.retain(|k, _| !k.starts_with(prefix));
        self.settled_values.retain(|k, _| !k.starts_with(prefix));
        self.settled_colors.retain(|k, _| !k.starts_with(prefix));
        self.color_animations.retain(|k, _| !k.starts_with(prefix));
    }

    pub fn animate_value(
        &mut self,
        key: &str,
        target: f64,
        stiffness: f64,
        damping: f64,
        initial_if_new: Option<f64>,
    ) -> f64 {
        let cur_gen = self.tick_gen;
        if self.settled_values.len() > 2048 {
            self.settled_values.retain(|k, (_, gen)| {
                cur_gen.saturating_sub(*gen) <= 60 || self.named_animations.contains_key(k)
            });
        }
        if let Some(anim) = self.named_animations.get_mut(key) {
            if (anim.target - target).abs() > 0.001 {
                anim.initial = anim.value;
                anim.target = target;
                anim.finished = false;
                anim.start = Instant::now();
            }
            self.settled_values
                .insert(key.to_string(), (target, cur_gen));
            return anim.value;
        }
        if let Some(&(prev, _)) = self.settled_values.get(key) {
            self.settled_values
                .insert(key.to_string(), (target, cur_gen));
            if (prev - target).abs() > 0.002 {
                self.start_named_spring(key, prev, target, stiffness, damping);
                return prev;
            }
            return target;
        }
        self.settled_values
            .insert(key.to_string(), (target, cur_gen));
        if let Some(init) = initial_if_new {
            if (init - target).abs() > 0.002 {
                self.start_named_spring(key, init, target, stiffness, damping);
                return init;
            }
        }
        target
    }

    pub fn animate_color(
        &mut self,
        key: &str,
        target: [f32; 4],
        stiffness: f64,
        damping: f64,
    ) -> [f32; 4] {
        let color_dist = |a: [f32; 4], b: [f32; 4]| -> f32 {
            (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs() + (a[3] - b[3]).abs()
        };
        let normalize_pair = |from: [f32; 4], to: [f32; 4]| -> ([f32; 4], [f32; 4]) {
            if from[3] <= 0.001 && to[3] > 0.001 {
                ([to[0], to[1], to[2], 0.0], to)
            } else if to[3] <= 0.001 && from[3] > 0.001 {
                (from, [from[0], from[1], from[2], 0.0])
            } else {
                (from, to)
            }
        };
        let cur_gen = self.tick_gen;
        if self.settled_colors.len() > 2048 {
            self.settled_colors.retain(|k, (_, gen)| {
                cur_gen.saturating_sub(*gen) <= 60 || self.color_animations.contains_key(k)
            });
        }
        if let Some(anim) = self.color_animations.get_mut(key) {
            if color_dist(anim.target, target) > 0.008 {
                let (f, t) = normalize_pair(anim.current, target);
                anim.from = f;
                anim.current = f;
                anim.target = t;
                anim.progress = 0.0;
                anim.velocity = 0.0;
                anim.start = Instant::now();
                anim.finished = false;
            }
            self.settled_colors
                .insert(key.to_string(), (target, cur_gen));
            return anim.current;
        }
        if let Some(&(prev, _)) = self.settled_colors.get(key) {
            self.settled_colors
                .insert(key.to_string(), (target, cur_gen));
            if color_dist(prev, target) > 0.008 {
                let (f, t) = normalize_pair(prev, target);
                self.color_animations.insert(
                    key.to_string(),
                    ColorAnimation {
                        from: f,
                        current: f,
                        target: t,
                        progress: 0.0,
                        velocity: 0.0,
                        stiffness,
                        damping,
                        start: Instant::now(),
                        finished: false,
                    },
                );
                return f;
            }
            return target;
        }
        self.settled_colors
            .insert(key.to_string(), (target, cur_gen));
        target
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        self.tick_gen = self.tick_gen.wrapping_add(1);
        let mut needs_frame = false;
        self.animations.retain(|_handle, anim| {
            needs_frame = true;
            anim.tick(now)
        });
        self.named_animations.retain(|_, anim| {
            if anim.finished {
                return false;
            }
            needs_frame = true;
            let _ = anim.tick(now);
            true
        });
        self.color_animations.retain(|_, anim| {
            if anim.finished {
                return false;
            }
            needs_frame = true;
            let _ = anim.tick(now);
            true
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
                initial,
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
                initial,
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
            initial: 0.0,
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
            initial: 0.0,
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
            initial: 0.0,
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
    initial: f64,
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
                let total_dt = now
                    .duration_since(self.start)
                    .as_secs_f64()
                    .clamp(0.0001, 0.020);
                self.start = now;
                let steps = ((total_dt / 0.004).ceil() as usize).clamp(1, 8);
                let step_dt = total_dt / steps as f64;
                for _ in 0..steps {
                    let displacement = self.target - self.value;
                    let spring_force = displacement * *stiffness;
                    let damping_force = *velocity * *damping;
                    let acceleration = spring_force - damping_force;
                    *velocity += acceleration * step_dt;
                    self.value += *velocity * step_dt;
                }

                if (self.target - self.value).abs() < 0.005 && velocity.abs() < 0.015 {
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
                self.value = self.initial + (self.target - self.initial) * eased;
                if t >= 1.0 {
                    self.value = self.target;
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
