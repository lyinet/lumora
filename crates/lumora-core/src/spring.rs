//! Closed-form damped harmonic oscillator.
//!
//! Lumora's motion language is built on springs. The same [`Spring`] value is
//! used in two ways:
//!
//! 1. **Core Animation** — its `mass`/`stiffness`/`damping`/`initial_velocity`
//!    map 1:1 onto `CASpringAnimation`, and [`Spring::settling_time`] gives the
//!    `duration` to use (Core Animation does not pick one for you unless you
//!    read `settlingDuration`, which is computed with a looser threshold).
//! 2. **Manual animation** — [`SpringAnimator`] samples the analytic solution
//!    at an arbitrary time, so frame-driven code (e.g. a `CVDisplayLink` or
//!    timer-driven layout) can follow the exact same curve and can be
//!    *retargeted* mid-flight while preserving velocity.
//!
//! The solution is expressed for a unit step: progress starts at `0.0` and
//! settles at `1.0`. Writing `x(t) = progress(t) - 1` (the displacement from
//! rest), the equation of motion is
//!
//! ```text
//! m·x'' + c·x' + k·x = 0,   x(0) = -1,   x'(0) = v0
//! ```
//!
//! with `ω0 = √(k/m)` and damping ratio `ζ = c / (2√(km))`. The three regimes
//! (under-, critically and over-damped) each have a closed form; see
//! [`Spring::displacement`].
//!
//! `initial_velocity` uses Core Animation's convention: units of "the whole
//! distance per second", so `1.0` means the value would cover the entire
//! from→to distance in one second at its initial speed.

/// Physical spring parameters. All fields must be positive (velocity may be
/// any sign).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub mass: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub initial_velocity: f64,
}

/// Which closed-form solution applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    Underdamped,
    Critical,
    Overdamped,
}

impl Default for Spring {
    fn default() -> Self {
        Spring::SNAPPY
    }
}

impl Spring {
    /// Quick UI response with a hint of overshoot (selection corners, pills).
    pub const SNAPPY: Spring = Spring::new(1.0, 420.0, 30.0);
    /// The Quick Access card slide-in (DESIGN.md: stiffness 300, damping 26).
    pub const CARD: Spring = Spring::new(1.0, 300.0, 26.0);
    /// Gentle, no-overshoot movement (reflow of stacked cards).
    pub const SMOOTH: Spring = Spring::new(1.0, 220.0, 30.0);
    /// Playful, visibly bouncy (countdown digits, pin "drop").
    pub const BOUNCY: Spring = Spring::new(1.0, 260.0, 14.0);

    /// A spring with zero initial velocity.
    pub const fn new(mass: f64, stiffness: f64, damping: f64) -> Self {
        Spring { mass, stiffness, damping, initial_velocity: 0.0 }
    }

    /// Same spring with a different initial velocity.
    pub const fn with_velocity(self, initial_velocity: f64) -> Self {
        Spring { initial_velocity, ..self }
    }

    /// Build a spring from perceptual parameters, as SwiftUI does:
    /// `response` is the period (seconds) of the undamped oscillation and
    /// `damping_fraction` is the damping ratio ζ (1.0 = critically damped).
    pub fn from_response(response: f64, damping_fraction: f64) -> Spring {
        let response = response.max(1e-3);
        let omega0 = std::f64::consts::TAU / response;
        let mass = 1.0;
        let stiffness = omega0 * omega0 * mass;
        let damping = 2.0 * damping_fraction.max(0.0) * (stiffness * mass).sqrt();
        Spring::new(mass, stiffness, damping)
    }

    /// Undamped angular frequency ω0 (rad/s).
    pub fn omega0(&self) -> f64 {
        (self.stiffness / self.mass).sqrt()
    }

    /// Damping ratio ζ.
    pub fn damping_ratio(&self) -> f64 {
        self.damping / (2.0 * (self.stiffness * self.mass).sqrt())
    }

    pub fn regime(&self) -> Regime {
        let z = self.damping_ratio();
        // Treat a tiny band around 1 as critical to avoid catastrophic
        // cancellation in the over/under-damped formulas.
        if (z - 1.0).abs() < 1e-6 {
            Regime::Critical
        } else if z < 1.0 {
            Regime::Underdamped
        } else {
            Regime::Overdamped
        }
    }

    /// Displacement from rest `x(t)` and its derivative `x'(t)`, for
    /// `x(0) = -1`, `x'(0) = initial_velocity`.
    pub fn displacement(&self, t: f64) -> (f64, f64) {
        let t = t.max(0.0);
        let x0 = -1.0;
        let v0 = self.initial_velocity;
        let w0 = self.omega0();
        let z = self.damping_ratio();
        match self.regime() {
            Regime::Underdamped => {
                let wd = w0 * (1.0 - z * z).sqrt();
                let a = x0;
                let b = (v0 + z * w0 * x0) / wd;
                let env = (-z * w0 * t).exp();
                let (s, c) = (wd * t).sin_cos();
                let x = env * (a * c + b * s);
                // d/dt [e^{-σt}(A cos ωt + B sin ωt)]
                let v = env * ((b * wd - z * w0 * a) * c - (a * wd + z * w0 * b) * s);
                (x, v)
            }
            Regime::Critical => {
                let b = v0 + w0 * x0;
                let env = (-w0 * t).exp();
                let x = env * (x0 + b * t);
                let v = env * (b - w0 * (x0 + b * t));
                (x, v)
            }
            Regime::Overdamped => {
                let s = (z * z - 1.0).sqrt();
                let r1 = -w0 * (z - s);
                let r2 = -w0 * (z + s);
                let c2 = (v0 - r1 * x0) / (r2 - r1);
                let c1 = x0 - c2;
                let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
                (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
            }
        }
    }

    /// Normalised progress at time `t` (0 → 1, may overshoot).
    pub fn value(&self, t: f64) -> f64 {
        1.0 + self.displacement(t).0
    }

    /// Normalised velocity at time `t` (distance-per-second units).
    pub fn velocity(&self, t: f64) -> f64 {
        self.displacement(t).1
    }

    /// Time after which the spring stays within `epsilon` of rest (both
    /// position and, scaled, velocity). Use `0.001` for "invisible" on a
    /// typical 300pt move.
    ///
    /// The solution is sampled at 1 kHz up to 30 s; the last sample outside the
    /// tolerance determines the answer. Cheap enough to call per animation.
    pub fn settling_time(&self, epsilon: f64) -> f64 {
        const DT: f64 = 1.0 / 1000.0;
        const LIMIT: f64 = 30.0;
        let eps = epsilon.max(1e-9);
        let w0 = self.omega0().max(1e-9);
        let mut last_outside = 0.0;
        let mut t = 0.0;
        while t <= LIMIT {
            let (x, v) = self.displacement(t);
            // Velocity is compared in "displacement units" by dividing by ω0.
            if x.abs() > eps || (v / w0).abs() > eps {
                last_outside = t;
            }
            t += DT;
        }
        (last_outside + DT).min(LIMIT)
    }

    /// The largest overshoot past the target, as a fraction of the distance
    /// (0 for critically/over-damped springs without strong initial velocity).
    pub fn overshoot(&self) -> f64 {
        let horizon = self.settling_time(1e-4);
        let steps = (horizon * 1000.0) as usize + 1;
        (0..=steps).map(|i| self.value(i as f64 / 1000.0) - 1.0).fold(0.0_f64, f64::max)
    }
}

/// Samples a spring between two scalar values, with support for changing the
/// target mid-flight without a velocity discontinuity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringAnimator {
    spring: Spring,
    from: f64,
    to: f64,
    start: f64,
}

impl SpringAnimator {
    /// Start animating `from` → `to` at time `now` (seconds, any epoch).
    pub fn new(spring: Spring, from: f64, to: f64, now: f64) -> Self {
        SpringAnimator { spring, from, to, start: now }
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    /// Current value.
    pub fn value(&self, now: f64) -> f64 {
        self.from + (self.to - self.from) * self.spring.value(now - self.start)
    }

    /// Current velocity in value-units per second.
    pub fn velocity(&self, now: f64) -> f64 {
        (self.to - self.from) * self.spring.velocity(now - self.start)
    }

    /// Whether the animation has visually come to rest (within 0.5 units of
    /// the target and slower than 1 unit/s — sub-pixel for UI work).
    pub fn is_settled(&self, now: f64) -> bool {
        (self.value(now) - self.to).abs() < 0.5 && self.velocity(now).abs() < 1.0
    }

    /// Point the animation at a new target. The current position and velocity
    /// are carried over, so motion stays continuous.
    pub fn retarget(&mut self, new_to: f64, now: f64) {
        self.retarget_with(self.spring, new_to, now);
    }

    /// Like [`retarget`](Self::retarget) but also switches to a different
    /// spring (e.g. a stiffer one for dismissal), still carrying velocity.
    pub fn retarget_with(&mut self, spring: Spring, new_to: f64, now: f64) {
        let pos = self.value(now);
        let vel = self.velocity(now);
        let distance = new_to - pos;
        // Express the absolute velocity in the new normalised units.
        let v_norm = if distance.abs() > f64::EPSILON { vel / distance } else { 0.0 };
        self.spring = spring.with_velocity(v_norm);
        self.from = pos;
        self.to = new_to;
        self.start = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn starts_at_zero_and_settles_at_one() {
        for s in [Spring::SNAPPY, Spring::CARD, Spring::SMOOTH, Spring::BOUNCY] {
            assert!(approx(s.value(0.0), 0.0, 1e-12), "{s:?}");
            assert!(approx(s.value(10.0), 1.0, 1e-6), "{s:?}");
        }
    }

    #[test]
    fn regimes_classified() {
        assert_eq!(Spring::new(1.0, 100.0, 5.0).regime(), Regime::Underdamped);
        assert_eq!(Spring::new(1.0, 100.0, 20.0).regime(), Regime::Critical);
        assert_eq!(Spring::new(1.0, 100.0, 50.0).regime(), Regime::Overdamped);
    }

    /// The analytic solution must agree with a fine numerical integration in
    /// all three regimes, including non-zero initial velocity.
    #[test]
    fn matches_numerical_integration() {
        let springs = [
            Spring::new(1.0, 300.0, 10.0).with_velocity(3.0),
            Spring::new(1.0, 100.0, 20.0).with_velocity(-2.0),
            Spring::new(2.0, 100.0, 80.0).with_velocity(1.0),
            Spring::CARD,
        ];
        for s in springs {
            // Semi-implicit Euler at 100 kHz.
            let dt = 1e-5;
            let (mut x, mut v) = (-1.0_f64, s.initial_velocity);
            let mut t = 0.0;
            while t < 0.5 {
                let a = (-s.stiffness * x - s.damping * v) / s.mass;
                v += a * dt;
                x += v * dt;
                t += dt;
            }
            let (xa, va) = s.displacement(t);
            assert!(approx(x, xa, 1e-3), "{s:?}: x num={x} analytic={xa}");
            assert!(approx(v, va, 2e-2), "{s:?}: v num={v} analytic={va}");
        }
    }

    #[test]
    fn velocity_is_derivative_of_position() {
        let s = Spring::BOUNCY.with_velocity(0.7);
        for &t in &[0.0, 0.05, 0.1, 0.3, 0.8] {
            let h = 1e-6;
            let numeric = (s.value(t + h) - s.value((t - h).max(0.0))) / (t + h - (t - h).max(0.0));
            assert!(approx(numeric, s.velocity(t), 1e-3), "t={t}");
        }
        assert!(approx(s.velocity(0.0), 0.7, 1e-9));
    }

    #[test]
    fn critical_and_overdamped_do_not_overshoot() {
        assert!(Spring::new(1.0, 100.0, 20.0).overshoot() < 1e-9);
        assert!(Spring::new(1.0, 100.0, 60.0).overshoot() < 1e-9);
        let bouncy = Spring::BOUNCY.overshoot();
        assert!(bouncy > 0.1 && bouncy < 0.6, "bouncy overshoot {bouncy}");
    }

    #[test]
    fn card_spring_feels_right() {
        // DESIGN.md promises the card lands in roughly half a second with a
        // small (< 5 %) overshoot.
        let s = Spring::CARD;
        let t = s.settling_time(1e-3);
        assert!(t > 0.3 && t < 0.8, "settling {t}");
        assert!(s.overshoot() < 0.05, "overshoot {}", s.overshoot());
        assert!(s.damping_ratio() > 0.7 && s.damping_ratio() < 0.8);
    }

    #[test]
    fn stiffer_springs_settle_faster() {
        let soft = Spring::new(1.0, 100.0, 20.0).settling_time(1e-3);
        let stiff = Spring::new(1.0, 400.0, 40.0).settling_time(1e-3);
        assert!(stiff < soft);
    }

    #[test]
    fn from_response_matches_damping_fraction() {
        let s = Spring::from_response(0.5, 0.8);
        assert!(approx(s.damping_ratio(), 0.8, 1e-9));
        assert!(approx(std::f64::consts::TAU / s.omega0(), 0.5, 1e-9));
        assert_eq!(Spring::from_response(0.4, 1.0).regime(), Regime::Critical);
    }

    #[test]
    fn animator_maps_range_and_settles() {
        let a = SpringAnimator::new(Spring::CARD, 100.0, 400.0, 10.0);
        assert!(approx(a.value(10.0), 100.0, 1e-9));
        assert!(!a.is_settled(10.1));
        assert!(a.is_settled(12.0));
        assert!(approx(a.value(12.0), 400.0, 0.5));
    }

    #[test]
    fn retarget_preserves_position_and_velocity() {
        let mut a = SpringAnimator::new(Spring::SNAPPY, 0.0, 100.0, 0.0);
        let t = 0.06;
        let (p, v) = (a.value(t), a.velocity(t));
        a.retarget(-50.0, t);
        assert!(approx(a.value(t), p, 1e-9));
        assert!(approx(a.velocity(t), v, 1e-6), "{} vs {v}", a.velocity(t));
        assert!(approx(a.value(t + 5.0), -50.0, 1e-3));
        assert_eq!(a.target(), -50.0);
    }

    #[test]
    fn retarget_with_switches_spring_keeping_velocity() {
        let mut a = SpringAnimator::new(Spring::BOUNCY, 0.0, 100.0, 0.0);
        let t = 0.05;
        let v = a.velocity(t);
        a.retarget_with(Spring::SMOOTH, 300.0, t);
        assert!(approx(a.velocity(t), v, 1e-6));
        assert!(a.is_settled(t + 3.0));
        assert!(approx(a.value(t + 3.0), 300.0, 0.5));
    }
}
