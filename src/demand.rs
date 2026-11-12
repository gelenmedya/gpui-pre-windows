//! rustyfiles fork: demand-driven frames.
//!
//! Upstream invalidates every window on every vsync, so an idle window still wakes the UI thread
//! 60–240 times per second (≈1–3% of a core doing nothing). GPUI core already supports
//! demand-driven platforms through `PlatformWindow::frame_waker` / `schedule_frame`; this module
//! implements them for Windows:
//! - the waker sets a "frame wanted" flag and wakes the vsync thread,
//! - the vsync thread only invalidates windows when a frame is wanted,
//! - after ~1 s without demand it stops waiting on vsync and parks (waking every 250 ms to keep
//!   servicing device-lost checks and hybrid-renderer switches).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

static WANTED: AtomicBool = AtomicBool::new(true);
static PARK: Mutex<bool> = Mutex::new(false);
static PARK_CV: Condvar = Condvar::new();
static LAST_DEMAND: Mutex<Option<Instant>> = Mutex::new(None);

/// Disable with `RUSTYFILES_CONTINUOUS_FRAMES=1` (upstream behaviour, for comparison).
fn continuous() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *C.get_or_init(|| std::env::var_os("RUSTYFILES_CONTINUOUS_FRAMES").is_some())
}

/// A window needs a frame. Cheap; callable from any thread.
pub(crate) fn request() {
    if !WANTED.swap(true, Ordering::AcqRel) {
        let mut woke = PARK.lock().unwrap();
        *woke = true;
        PARK_CV.notify_all();
    }
}

/// A frame wanted by one window. A window that is not being presented (hidden or minimized) is not sent `WM_PAINT`, so a frame it
/// asks for can never be drawn and it stays dirty; GPUI's core asks again for every dirty window at the end of every effect cycle,
/// and each of those asks made the vsync thread invalidate all windows, whose paint messages ended in the next effect cycle: the
/// UI thread and the vsync thread turned at the display's rate for as long as such a window existed. Measured in rustyfiles' dock
/// with one hidden pop-up window: about 530 wake-ups a second of the UI thread and 3 % of a core on an idle desktop, against 16 ms
/// of CPU in 6 s without it. Such a window therefore asks for nothing; it is drawn when it is shown or restored (`WM_SHOWWINDOW`
/// and `WM_SIZE` ask for a frame themselves).
pub(crate) fn request_for(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    let presented = unsafe { IsWindowVisible(hwnd).as_bool() && !IsIconic(hwnd).as_bool() };
    if presented {
        request();
    }
}

/// Called by the vsync thread each iteration: returns true when windows should be invalidated.
pub(crate) fn take() -> bool {
    if continuous() {
        return true;
    }
    let wanted = WANTED.swap(false, Ordering::AcqRel);
    if wanted {
        *LAST_DEMAND.lock().unwrap() = Some(Instant::now());
    }
    wanted
}

/// True when the vsync thread may stop following vsync and park.
pub(crate) fn idle() -> bool {
    if continuous() {
        return false;
    }
    match *LAST_DEMAND.lock().unwrap() {
        Some(t) => t.elapsed() > Duration::from_millis(1000) && !WANTED.load(Ordering::Acquire),
        None => false,
    }
}

/// Parks until a frame is requested or `timeout` elapses.
pub(crate) fn park(timeout: Duration) {
    let mut woke = PARK.lock().unwrap();
    if WANTED.load(Ordering::Acquire) {
        *woke = false;
        return;
    }
    let (mut g, _) = PARK_CV.wait_timeout_while(woke, timeout, |w| !*w).unwrap();
    *g = false;
    woke = g;
    drop(woke);
}
