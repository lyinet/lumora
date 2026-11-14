//! Recording lifecycle as a validated state machine.
//!
//! The app owns one [`RecordingSession`]; UI elements (menu-bar timer, stop
//! control, countdown ring) render from it, and the recorder reports back
//! through [`RecordingSession::apply`]. Illegal transitions (e.g. stopping
//! twice, starting while finishing) are rejected instead of corrupting state.

use thiserror::Error;

/// Times are seconds on a monotonic clock supplied by the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Idle,
    /// Counting down; `remaining` whole seconds are left to show.
    Countdown {
        remaining: u32,
    },
    /// Waiting for the stream to deliver its first frame.
    Starting,
    Recording {
        started_at: f64,
    },
    /// Stop requested; the writer is finalising the file.
    Finishing {
        duration: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// User asked to record. `countdown` seconds (0 = start immediately).
    Begin { countdown: u32 },
    /// One countdown second elapsed.
    Tick,
    /// The stream started producing frames.
    Started { now: f64 },
    /// User pressed stop.
    Stop { now: f64 },
    /// The file was written (or failed); back to idle.
    Finished,
    /// Abort from any phase (Esc during countdown, stream error).
    Cancel,
}

#[derive(Debug, Error, PartialEq)]
#[error("invalid recording transition: {event:?} while {phase:?}")]
pub struct InvalidTransition {
    pub phase: Phase,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingSession {
    phase: Phase,
}

impl Default for RecordingSession {
    fn default() -> Self {
        RecordingSession { phase: Phase::Idle }
    }
}

impl RecordingSession {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Seconds recorded so far (0 outside `Recording`, frozen in `Finishing`).
    pub fn elapsed(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Recording { started_at } => (now - started_at).max(0.0),
            Phase::Finishing { duration } => duration,
            _ => 0.0,
        }
    }

    /// Apply an event, returning the new phase.
    pub fn apply(&mut self, event: Event) -> Result<Phase, InvalidTransition> {
        use Phase::*;
        let next = match (self.phase, event) {
            (Idle, Event::Begin { countdown: 0 }) => Starting,
            (Idle, Event::Begin { countdown }) => Countdown { remaining: countdown },
            (Countdown { remaining }, Event::Tick) if remaining > 1 => Countdown { remaining: remaining - 1 },
            (Countdown { .. }, Event::Tick) => Starting,
            (Starting, Event::Started { now }) => Recording { started_at: now },
            (Recording { started_at }, Event::Stop { now }) => Finishing { duration: (now - started_at).max(0.0) },
            (Finishing { .. }, Event::Finished) => Idle,
            (_, Event::Cancel) => Idle,
            (phase, event) => return Err(InvalidTransition { phase, event }),
        };
        self.phase = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_with_countdown() {
        let mut s = RecordingSession::default();
        assert_eq!(s.apply(Event::Begin { countdown: 3 }), Ok(Phase::Countdown { remaining: 3 }));
        assert_eq!(s.apply(Event::Tick), Ok(Phase::Countdown { remaining: 2 }));
        assert_eq!(s.apply(Event::Tick), Ok(Phase::Countdown { remaining: 1 }));
        assert_eq!(s.apply(Event::Tick), Ok(Phase::Starting));
        s.apply(Event::Started { now: 100.0 }).unwrap();
        assert_eq!(s.elapsed(107.5), 7.5);
        assert_eq!(s.apply(Event::Stop { now: 110.0 }), Ok(Phase::Finishing { duration: 10.0 }));
        assert_eq!(s.elapsed(500.0), 10.0);
        assert_eq!(s.apply(Event::Finished), Ok(Phase::Idle));
        assert!(!s.is_active());
    }

    #[test]
    fn zero_countdown_starts_immediately() {
        let mut s = RecordingSession::default();
        assert_eq!(s.apply(Event::Begin { countdown: 0 }), Ok(Phase::Starting));
    }

    #[test]
    fn invalid_transitions_rejected() {
        let mut s = RecordingSession::default();
        assert!(s.apply(Event::Stop { now: 1.0 }).is_err());
        s.apply(Event::Begin { countdown: 0 }).unwrap();
        assert!(s.apply(Event::Begin { countdown: 0 }).is_err());
        assert_eq!(s.phase(), Phase::Starting, "state untouched on error");
        s.apply(Event::Started { now: 0.0 }).unwrap();
        s.apply(Event::Stop { now: 1.0 }).unwrap();
        assert!(s.apply(Event::Stop { now: 2.0 }).is_err());
    }

    #[test]
    fn cancel_from_anywhere() {
        let mut s = RecordingSession::default();
        s.apply(Event::Begin { countdown: 3 }).unwrap();
        assert_eq!(s.apply(Event::Cancel), Ok(Phase::Idle));
    }
}
