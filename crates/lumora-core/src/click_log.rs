//! Click-event log for recordings: the `<movie stem>.events.json` sidecar.
//!
//! While recording, the app collects raw mouse-downs (global Quartz points
//! plus the event's host-clock timestamp). When the movie is finalised they
//! are turned into [`ClickEvent`]s with this module's pure helpers:
//!
//! * **time** — seconds since the movie's first video sample, on the same
//!   host clock the writer used to start its session (see [`time`]);
//! * **space** — the point on the display the click happened on (points and
//!   pixels) and, when it fell inside the recorded area, the matching pixel
//!   in the encoded video (see [`RecordingGeometry::map`]).
//!
//! The file is versioned ([`SCHEMA_VERSION`]) so later tooling (step-by-step
//! tutorials, chapters) can evolve it without guessing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::{Point, Rect};

/// Current `events.json` schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Sidecar path for a movie: `/dir/Recording 1.mp4` → `/dir/Recording 1.events.json`.
pub fn sidecar_path(movie: &Path) -> PathBuf {
    let stem = movie.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "recording".into());
    movie.with_file_name(format!("{stem}.events.json"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Other,
}

/// Modifier keys held at the click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub option: bool,
    pub command: bool,
    pub caps_lock: bool,
    pub function: bool,
}

impl Modifiers {
    /// Decode `NSEventModifierFlags` raw bits.
    pub fn from_ns_bits(bits: u64) -> Modifiers {
        Modifiers {
            caps_lock: bits & (1 << 16) != 0,
            shift: bits & (1 << 17) != 0,
            control: bits & (1 << 18) != 0,
            option: bits & (1 << 19) != 0,
            command: bits & (1 << 20) != 0,
            function: bits & (1 << 23) != 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Modifiers::default()
    }
}

/// The frontmost app when the click happened.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AppContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Title of the app's frontmost window, when the system reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
}

/// One display as seen at record start (global Quartz points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayGeometry {
    pub id: u32,
    pub frame: Rect,
    pub scale: f64,
}

/// What was recorded and how it maps to video pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingGeometry {
    pub displays: Vec<DisplayGeometry>,
    /// The recorded display's id.
    pub display_id: u32,
    /// Recorded area, global Quartz points (the whole display or a sub-rect).
    pub capture_frame: Rect,
    /// Encoded video size in pixels (even-rounded).
    pub video_width: u32,
    pub video_height: u32,
}

/// Where a click landed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MappedPoint {
    /// Display the click was on (`None` if between displays).
    pub display_id: Option<u32>,
    /// Point relative to that display's top-left, in points.
    pub display_point: Point,
    /// The same in that display's pixels.
    pub display_pixel: Point,
    /// Pixel in the video, only when inside the recorded area.
    pub video_pixel: Option<Point>,
}

impl RecordingGeometry {
    fn display_at(&self, p: Point) -> Option<&DisplayGeometry> {
        self.displays.iter().find(|d| d.frame.contains(p))
    }

    /// Map a global Quartz point.
    pub fn map(&self, p: Point) -> MappedPoint {
        let display = self.display_at(p);
        let (display_point, display_pixel) = match display {
            Some(d) => {
                let local = Point::new(p.x - d.frame.min_x(), p.y - d.frame.min_y());
                (local, Point::new(local.x * d.scale, local.y * d.scale))
            }
            None => (p, p),
        };
        let inside = display.is_some_and(|d| d.id == self.display_id) && self.capture_frame.contains(p);
        let video_pixel = inside.then(|| {
            let sx = f64::from(self.video_width) / self.capture_frame.width().max(f64::EPSILON);
            let sy = f64::from(self.video_height) / self.capture_frame.height().max(f64::EPSILON);
            let x = ((p.x - self.capture_frame.min_x()) * sx).clamp(0.0, f64::from(self.video_width.saturating_sub(1)));
            let y =
                ((p.y - self.capture_frame.min_y()) * sy).clamp(0.0, f64::from(self.video_height.saturating_sub(1)));
            Point::new(x, y)
        });
        MappedPoint { display_id: display.map(|d| d.id), display_point, display_pixel, video_pixel }
    }
}

/// A click as captured by the app, before mapping.
#[derive(Debug, Clone, PartialEq)]
pub struct RawClick {
    /// Host-clock seconds (`NSEvent.timestamp`).
    pub host_time: f64,
    /// Global Quartz points.
    pub point: Point,
    pub button: MouseButton,
    pub click_count: u32,
    pub modifiers: Modifiers,
    pub app: AppContext,
}

/// One click in the sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClickEvent {
    /// Seconds since the first video frame.
    pub t: f64,
    pub button: MouseButton,
    pub click_count: u32,
    #[serde(default, skip_serializing_if = "Modifiers::is_empty")]
    pub modifiers: Modifiers,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    /// `[x, y]` points from the display's top-left.
    pub display_point: [f64; 2],
    /// `[x, y]` pixels from the display's top-left.
    pub display_pixel: [f64; 2],
    /// `[x, y]` pixels in the video; `null` when outside the recorded area.
    pub video_pixel: Option<[f64; 2]>,
    /// `false` when the click was outside the recorded area (or display).
    pub inside_recording: bool,
    #[serde(default)]
    pub app: AppContext,
}

/// Recording metadata at the top of the sidecar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingInfo {
    /// Movie file name (no directory).
    pub movie: String,
    pub display_id: u32,
    /// `[x, y, w, h]` recorded area, global points (top-left origin).
    pub capture_rect: [f64; 4],
    pub scale: f64,
    pub video_width: u32,
    pub video_height: u32,
    pub duration: f64,
}

/// The whole `events.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventLog {
    pub schema_version: u32,
    pub recording: RecordingInfo,
    pub clicks: Vec<ClickEvent>,
}

impl EventLog {
    /// Build the log: clicks before the first frame or after `duration`
    /// are dropped; the rest are mapped and sorted by time.
    pub fn build(
        movie: &Path,
        geometry: &RecordingGeometry,
        start_host_time: f64,
        duration: f64,
        raw: &[RawClick],
    ) -> EventLog {
        let scale = geometry.displays.iter().find(|d| d.id == geometry.display_id).map_or(1.0, |d| d.scale);
        let mut clicks: Vec<ClickEvent> = raw
            .iter()
            .filter_map(|c| {
                let t = time::offset_from_start(c.host_time, start_host_time, duration)?;
                let m = geometry.map(c.point);
                Some(ClickEvent {
                    t: round3(t),
                    button: c.button,
                    click_count: c.click_count.max(1),
                    modifiers: c.modifiers,
                    display_id: m.display_id,
                    display_point: [round2(m.display_point.x), round2(m.display_point.y)],
                    display_pixel: [round2(m.display_pixel.x), round2(m.display_pixel.y)],
                    video_pixel: m.video_pixel.map(|v| [round2(v.x), round2(v.y)]),
                    inside_recording: m.video_pixel.is_some(),
                    app: c.app.clone(),
                })
            })
            .collect();
        clicks.sort_by(|a, b| a.t.total_cmp(&b.t));
        let f = geometry.capture_frame;
        EventLog {
            schema_version: SCHEMA_VERSION,
            recording: RecordingInfo {
                movie: movie.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                display_id: geometry.display_id,
                capture_rect: [f.min_x(), f.min_y(), f.width(), f.height()],
                scale,
                video_width: geometry.video_width,
                video_height: geometry.video_height,
                duration: round3(duration),
            },
            clicks,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(s: &str) -> Result<EventLog, serde_json::Error> {
        serde_json::from_str(s)
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Clock alignment between capture sources and the writer's timeline.
///
/// All of ScreenCaptureKit's samples, `NSEvent.timestamp` and the writer's
/// session start are on the host clock (mach absolute time, in seconds).
/// `AVCaptureSession` audio arrives on the session's own synchronisation
/// clock and must be converted before it can be appended next to SC video.
pub mod time {
    /// Seconds since the first video frame, or `None` if outside
    /// `[0, duration]` (a small 50 ms grace is allowed at either end for
    /// clicks that race the start/stop).
    pub fn offset_from_start(event_host: f64, start_host: f64, duration: f64) -> Option<f64> {
        const GRACE: f64 = 0.05;
        let t = event_host - start_host;
        if !t.is_finite() || t < -GRACE || (duration > 0.0 && t > duration + GRACE) {
            return None;
        }
        Some(t.clamp(0.0, if duration > 0.0 { duration } else { f64::MAX }))
    }

    /// A simultaneous reading of a source clock and the host clock, used to
    /// carry timestamps across (linear; drift is negligible over a reading
    /// that is refreshed per buffer).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ClockAnchor {
        pub source: f64,
        pub host: f64,
    }

    impl ClockAnchor {
        pub fn to_host(&self, source_time: f64) -> f64 {
            source_time - self.source + self.host
        }
    }

    /// Whether an audio buffer `[pts, pts + duration)` (host seconds) should
    /// be appended to a writer session that started at `session_start`:
    /// buffers that end before the first video frame are dropped (the writer
    /// would otherwise extend the movie backwards).
    pub fn audio_overlaps_session(pts: f64, duration: f64, session_start: Option<f64>) -> bool {
        match session_start {
            Some(start) => pts + duration.max(0.0) > start,
            None => false,
        }
    }

    /// Convert seconds to a `CMTime` value at `timescale` (rounded).
    pub fn to_timescale(seconds: f64, timescale: i32) -> i64 {
        (seconds * f64::from(timescale)).round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::time::*;
    use super::*;

    fn retina_two_displays() -> Vec<DisplayGeometry> {
        vec![
            DisplayGeometry { id: 1, frame: Rect::new(0.0, 0.0, 1512.0, 982.0), scale: 2.0 },
            DisplayGeometry { id: 2, frame: Rect::new(1512.0, -200.0, 1920.0, 1080.0), scale: 1.0 },
        ]
    }

    #[test]
    fn sidecar_name() {
        assert_eq!(sidecar_path(Path::new("/a/Rec 1.mp4")), PathBuf::from("/a/Rec 1.events.json"));
        assert_eq!(sidecar_path(Path::new("x.mov")), PathBuf::from("x.events.json"));
    }

    #[test]
    fn full_display_retina_mapping() {
        let g = RecordingGeometry {
            displays: retina_two_displays(),
            display_id: 1,
            capture_frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            video_width: 3024,
            video_height: 1964,
        };
        let m = g.map(Point::new(100.0, 50.5));
        assert_eq!(m.display_id, Some(1));
        assert_eq!(m.display_point, Point::new(100.0, 50.5));
        assert_eq!(m.display_pixel, Point::new(200.0, 101.0));
        assert_eq!(m.video_pixel, Some(Point::new(200.0, 101.0)));
    }

    #[test]
    fn area_recording_mapping_and_outside_flag() {
        // Area on the Retina display, odd pixel width even-rounded by the writer.
        let g = RecordingGeometry {
            displays: retina_two_displays(),
            display_id: 1,
            capture_frame: Rect::new(200.0, 100.0, 400.5, 300.0),
            video_width: 800,
            video_height: 600,
        };
        let m = g.map(Point::new(300.0, 250.0));
        let v = m.video_pixel.unwrap();
        assert!((v.x - 100.0 * 800.0 / 400.5).abs() < 1e-9);
        assert_eq!(v.y, 300.0);
        assert_eq!(m.display_pixel, Point::new(600.0, 500.0));
        // Same display, outside the area.
        let out = g.map(Point::new(10.0, 10.0));
        assert_eq!(out.display_id, Some(1));
        assert!(out.video_pixel.is_none());
        // Other display, at the same relative coords as the area would be.
        let other = g.map(Point::new(1712.0, -100.0));
        assert_eq!(other.display_id, Some(2));
        assert_eq!(other.display_point, Point::new(200.0, 100.0));
        assert!(other.video_pixel.is_none());
        // Off every display.
        let none = g.map(Point::new(-50.0, -50.0));
        assert_eq!(none.display_id, None);
        assert!(none.video_pixel.is_none());
    }

    #[test]
    fn secondary_display_recording() {
        let g = RecordingGeometry {
            displays: retina_two_displays(),
            display_id: 2,
            capture_frame: Rect::new(1512.0, -200.0, 1920.0, 1080.0),
            video_width: 1920,
            video_height: 1080,
        };
        let m = g.map(Point::new(1512.0 + 960.0, -200.0 + 540.0));
        assert_eq!(m.video_pixel, Some(Point::new(960.0, 540.0)));
        // Bottom-right edge pixel clamps into the frame.
        assert!(g.map(Point::new(3431.999, 879.999)).video_pixel.unwrap().x <= 1919.0);
    }

    #[test]
    fn time_offsets() {
        assert_eq!(offset_from_start(105.25, 100.0, 10.0), Some(5.25));
        assert_eq!(offset_from_start(99.98, 100.0, 10.0), Some(0.0), "grace before start");
        assert_eq!(offset_from_start(99.0, 100.0, 10.0), None);
        assert_eq!(offset_from_start(111.0, 100.0, 10.0), None);
        assert_eq!(offset_from_start(110.02, 100.0, 10.0), Some(10.0));
        assert_eq!(offset_from_start(f64::NAN, 100.0, 10.0), None);
    }

    #[test]
    fn clock_anchor_and_audio_window() {
        let a = ClockAnchor { source: 5.0, host: 1000.0 };
        assert_eq!(a.to_host(5.5), 1000.5);
        assert!(!audio_overlaps_session(999.0, 0.02, Some(1000.0)));
        assert!(audio_overlaps_session(999.99, 0.02, Some(1000.0)));
        assert!(audio_overlaps_session(1001.0, 0.02, Some(1000.0)));
        assert!(!audio_overlaps_session(1001.0, 0.02, None));
        assert_eq!(to_timescale(1.5, 48_000), 72_000);
    }

    #[test]
    fn modifiers_decode() {
        let m = Modifiers::from_ns_bits((1 << 17) | (1 << 20));
        assert!(m.shift && m.command && !m.option && !m.control);
        assert!(Modifiers::from_ns_bits(0).is_empty());
    }

    #[test]
    fn build_log_roundtrip() {
        let g = RecordingGeometry {
            displays: retina_two_displays(),
            display_id: 1,
            capture_frame: Rect::new(0.0, 0.0, 1512.0, 982.0),
            video_width: 3024,
            video_height: 1964,
        };
        let click = |t: f64, x: f64, b| RawClick {
            host_time: t,
            point: Point::new(x, 10.0),
            button: b,
            click_count: 1,
            modifiers: Modifiers::default(),
            app: AppContext { bundle_id: Some("com.apple.finder".into()), name: None, window_title: None },
        };
        let raw = vec![
            click(103.0, 2000.0, MouseButton::Right),
            click(101.0, 10.0, MouseButton::Left),
            click(50.0, 10.0, MouseButton::Left),
        ];
        let log = EventLog::build(Path::new("/tmp/Rec.mp4"), &g, 100.0, 5.0, &raw);
        assert_eq!(log.schema_version, 1);
        assert_eq!(log.recording.movie, "Rec.mp4");
        assert_eq!(log.recording.scale, 2.0);
        assert_eq!(log.clicks.len(), 2);
        assert_eq!(log.clicks[0].t, 1.0);
        assert!(log.clicks[0].inside_recording);
        assert_eq!(log.clicks[1].button, MouseButton::Right);
        assert!(!log.clicks[1].inside_recording);
        assert_eq!(log.clicks[1].display_id, Some(2));
        let json = log.to_json();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"video_pixel\": null"));
        assert_eq!(EventLog::from_json(&json).unwrap(), log);
    }
}
