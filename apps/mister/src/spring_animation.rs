// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Frame-driven spring animation with persistent position and velocity.

use std::f64::consts::TAU;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfiguration {
    mass: f64,
    stiffness: f64,
    damping: f64,
    damping_ratio: f64,
    angular_frequency: f64,
    value_epsilon: f64,
    velocity_epsilon: f64,
}

impl SpringConfiguration {
    pub fn from_response(response: Duration, damping_ratio: f64) -> Self {
        assert!(!response.is_zero(), "spring response must be positive");
        assert!(
            damping_ratio.is_finite() && damping_ratio >= 0.0,
            "spring damping ratio must be finite and non-negative"
        );
        let response = response.as_secs_f64();
        let mass = 1.0;
        let angular_frequency = TAU / response;
        let stiffness = mass * angular_frequency * angular_frequency;
        let damping = 2.0 * damping_ratio * mass * angular_frequency;
        Self::from_physical(mass, stiffness, damping)
    }

    pub fn from_physical(mass: f64, stiffness: f64, damping: f64) -> Self {
        assert!(
            mass.is_finite() && mass > 0.0,
            "spring mass must be finite and positive"
        );
        assert!(
            stiffness.is_finite() && stiffness > 0.0,
            "spring stiffness must be finite and positive"
        );
        assert!(
            damping.is_finite() && damping >= 0.0,
            "spring damping must be finite and non-negative"
        );
        let angular_frequency = (stiffness / mass).sqrt();
        let damping_ratio = damping / (2.0 * (stiffness * mass).sqrt());
        Self {
            mass,
            stiffness,
            damping,
            damping_ratio,
            angular_frequency,
            value_epsilon: 0.01,
            velocity_epsilon: 0.01,
        }
    }

    /// A smooth, critically damped spring with a 0.5-second response.
    pub fn smooth() -> Self {
        Self::smooth_with_response(Duration::from_millis(500))
    }

    /// A smooth, critically damped spring retimed to the requested response.
    pub fn smooth_with_response(response: Duration) -> Self {
        Self::from_response(response, 1.0)
    }

    pub fn angular_frequency(self) -> f64 {
        self.angular_frequency
    }

    pub fn damping_ratio(self) -> f64 {
        self.damping_ratio
    }

    pub fn mass(self) -> f64 {
        self.mass
    }

    pub fn stiffness(self) -> f64 {
        self.stiffness
    }

    pub fn damping(self) -> f64 {
        self.damping
    }
}

impl Default for SpringConfiguration {
    fn default() -> Self {
        Self::smooth()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpringAnimation {
    value: f64,
    velocity: f64,
    target: f64,
    configuration: SpringConfiguration,
}

impl SpringAnimation {
    pub fn new(value: f64, configuration: SpringConfiguration) -> Self {
        assert!(value.is_finite(), "spring value must be finite");
        Self {
            value,
            velocity: 0.0,
            target: value,
            configuration,
        }
    }

    pub fn value(self) -> f64 {
        self.value
    }

    pub fn velocity(self) -> f64 {
        self.velocity
    }

    pub fn target(self) -> f64 {
        self.target
    }

    pub fn configuration(self) -> SpringConfiguration {
        self.configuration
    }

    pub fn set_target(&mut self, target: f64) {
        assert!(target.is_finite(), "spring target must be finite");
        self.target = target;
    }

    pub fn set_state(&mut self, value: f64, velocity: f64) {
        assert!(value.is_finite(), "spring value must be finite");
        assert!(velocity.is_finite(), "spring velocity must be finite");
        self.value = value;
        self.velocity = velocity;
    }

    pub fn snap_to(&mut self, value: f64) {
        assert!(value.is_finite(), "spring value must be finite");
        self.value = value;
        self.velocity = 0.0;
        self.target = value;
    }

    pub fn is_settled(self) -> bool {
        (self.value - self.target).abs() <= self.configuration.value_epsilon
            && self.velocity.abs() <= self.configuration.velocity_epsilon
    }

    pub fn advance(&mut self, delta: Duration) -> f64 {
        let time = delta.as_secs_f64();
        let settled = self.is_settled();
        if time == 0.0 || settled {
            if settled {
                self.value = self.target;
                self.velocity = 0.0;
            }
            return self.value;
        }

        let y = self.value - self.target;
        let velocity = self.velocity;
        let omega = self.configuration.angular_frequency();
        let ratio = self.configuration.damping_ratio();
        let (next_y, next_velocity) = if (ratio - 1.0).abs() < 1e-6 {
            let b = velocity + omega * y;
            let decay = (-omega * time).exp();
            (
                (y + b * time) * decay,
                (velocity - omega * b * time) * decay,
            )
        } else if ratio < 1.0 {
            let damped = omega * (1.0 - ratio * ratio).sqrt();
            let a = y;
            let b = (velocity + ratio * omega * y) / damped;
            let phase = damped * time;
            let decay = (-ratio * omega * time).exp();
            let wave = a * phase.cos() + b * phase.sin();
            let wave_velocity = -a * damped * phase.sin() + b * damped * phase.cos();
            (wave * decay, (wave_velocity - ratio * omega * wave) * decay)
        } else {
            let root = (ratio * ratio - 1.0).sqrt();
            let r1 = -omega * (ratio - root);
            let r2 = -omega * (ratio + root);
            let c1 = (velocity - r2 * y) / (r1 - r2);
            let c2 = y - c1;
            let e1 = (r1 * time).exp();
            let e2 = (r2 * time).exp();
            (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
        };

        self.value = self.target + next_y;
        self.velocity = next_velocity;
        if self.is_settled() {
            self.value = self.target;
            self.velocity = 0.0;
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_has_expected_physical_parameters() {
        let spring = SpringConfiguration::smooth();
        assert!((spring.mass() - 1.0).abs() < 1e-12);
        assert!((spring.stiffness() - 157.913_670_417_429_73).abs() < 1e-9);
        assert!((spring.damping() - 25.132_741_228_718_345).abs() < 1e-9);
        assert!((spring.damping_ratio() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn smooth_response_can_be_retimed_without_changing_damping() {
        let spring = SpringConfiguration::smooth_with_response(Duration::from_millis(200));
        assert_eq!(spring.damping_ratio(), 1.0);
        assert!((spring.angular_frequency() - 10.0 * std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn retargeting_preserves_velocity() {
        let mut spring = SpringAnimation::new(0.0, SpringConfiguration::smooth());
        spring.set_target(100.0);
        spring.advance(Duration::from_millis(100));
        let velocity = spring.velocity();
        spring.set_target(200.0);
        assert_eq!(spring.velocity(), velocity);
    }

    #[test]
    fn analytic_solution_is_frame_rate_independent() {
        let mut sixty = SpringAnimation::new(0.0, SpringConfiguration::smooth());
        let mut one_twenty = sixty;
        sixty.set_target(100.0);
        one_twenty.set_target(100.0);
        for _ in 0..30 {
            sixty.advance(Duration::from_secs_f64(1.0 / 60.0));
        }
        for _ in 0..60 {
            one_twenty.advance(Duration::from_secs_f64(1.0 / 120.0));
        }
        assert!(
            (sixty.value() - one_twenty.value()).abs() < 1e-5,
            "60Hz={} 120Hz={}",
            sixty.value(),
            one_twenty.value()
        );
        assert!((sixty.velocity() - one_twenty.velocity()).abs() < 1e-4);
    }

    #[test]
    fn smooth_converges_without_overshoot_from_rest() {
        let mut spring = SpringAnimation::new(0.0, SpringConfiguration::smooth());
        spring.set_target(100.0);
        let mut previous = spring.value();
        for _ in 0..120 {
            let value = spring.advance(Duration::from_secs_f64(1.0 / 60.0));
            assert!(value >= previous);
            assert!(value <= 100.0);
            previous = value;
        }
        assert!(spring.is_settled());
        assert_eq!(spring.value(), 100.0);
    }

    #[test]
    #[should_panic(expected = "spring response must be positive")]
    fn rejects_zero_response() {
        SpringConfiguration::from_response(Duration::ZERO, 1.0);
    }

    #[test]
    #[should_panic(expected = "spring damping ratio must be finite and non-negative")]
    fn rejects_invalid_damping_ratio() {
        SpringConfiguration::from_response(Duration::from_millis(500), f64::NAN);
    }

    #[test]
    #[should_panic(expected = "spring mass must be finite and positive")]
    fn rejects_invalid_physical_parameters() {
        SpringConfiguration::from_physical(0.0, 100.0, 10.0);
    }

    #[test]
    fn full_frame_delta_is_evaluated_without_truncation() {
        let mut one_step = SpringAnimation::new(0.0, SpringConfiguration::smooth());
        let mut split = one_step;
        one_step.set_target(100.0);
        split.set_target(100.0);
        one_step.advance(Duration::from_millis(250));
        split.advance(Duration::from_millis(100));
        split.advance(Duration::from_millis(150));
        assert!((one_step.value() - split.value()).abs() < 1e-9);
        assert!((one_step.velocity() - split.velocity()).abs() < 1e-9);
    }
}
