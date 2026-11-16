//! What reaches a window while the app is in the middle of a task.
//!
//! THE TRAP. A foreground task holds the app while it runs (`AsyncApp::update`). If something inside it makes Windows call a
//! window procedure before it returns, GPUI cannot hand that message to the app: the borrow fails, the log gets "... RefCell
//! already borrowed", and the message is DROPPED. Two things were lost that way in rustyfiles:
//!
//! * a size or a place. A task that moves or resizes one of the app's own windows (`ShowWindow`, `SetWindowPos`) gets `WM_SIZE` and
//!   `WM_MOVE` sent to that window at once. The window changed, the app never heard: its layout, and every rectangle a click is
//!   tested against, stayed at the old size until the next resize. A click on the close button then landed on whatever the old
//!   layout had there.
//! * a click. Anything in a task that pumps messages (a call into another apartment or process, a modal Windows loop) can hand a
//!   mouse message to a window. It was dropped, and the user had to click again (rustyfiles issue 31: "App requires clicking the
//!   close button twice", with that log line before each second click).
//!
//! WHAT IS DONE. While a task runs on this thread ([`TaskGuard`], around every runnable), a mouse message is not dispatched but
//! kept, and a size or place change is noted. When the task ends, the kept messages are posted to their windows again, in order,
//! and each window whose bounds changed is told to read them once more (`WM_GPUI_REPLAY_BOUNDS`). They arrive from the main
//! loop, with nothing borrowed. Nothing else is touched: every other message is handled at once, as before.
//!
//! A task that calls into Windows OUTSIDE an update (the right way to move a window from a task) gets its size message handled at
//! once, successfully, and then replayed too: the same size told twice, which changes nothing.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::events::WM_GPUI_REPLAY_BOUNDS;

/// At most this many messages are kept for one task: a task that pumps for a long time under a moving pointer must not grow a list
/// without end (pointer moves replace each other, so it takes real clicks to get here).
const KEPT_AT_MOST: usize = 256;

const WM_MOUSEMOVE: u32 = 0x0200;
const WM_MOUSELAST: u32 = 0x020E;
const WM_NCMOUSEMOVE: u32 = 0x00A0;
const WM_NCXBUTTONDBLCLK: u32 = 0x00AD;

thread_local! {
    /// How many runnables are running on this thread, one inside the other.
    static IN_TASK: Cell<u32> = const { Cell::new(0) };
    /// What is kept for the end of the task: the window, the message, its two words.
    static KEPT: RefCell<Vec<(isize, u32, usize, isize)>> = const { RefCell::new(Vec::new()) };
}

/// Held while a runnable runs.
pub(crate) struct TaskGuard;

impl TaskGuard {
    pub(crate) fn enter() -> TaskGuard {
        IN_TASK.with(|c| c.set(c.get() + 1));
        TaskGuard
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        let left = IN_TASK.with(|c| {
            let n = c.get().saturating_sub(1);
            c.set(n);
            n
        });
        if left == 0 {
            post_kept();
        }
    }
}

/// A runnable is running on this thread.
pub(crate) fn in_task() -> bool {
    IN_TASK.with(|c| c.get() > 0)
}

/// A message of the pointer: over the client area or over the frame.
pub(crate) fn is_mouse_message(msg: u32) -> bool {
    (WM_MOUSEMOVE..=WM_MOUSELAST).contains(&msg) || (WM_NCMOUSEMOVE..=WM_NCXBUTTONDBLCLK).contains(&msg)
}

/// What a kept list looks like after one more message. A pointer move replaces the move just before it for the same window (only
/// the last place matters), and so does a second note of changed bounds; everything else is added, up to [`KEPT_AT_MOST`].
pub(crate) fn kept_with(list: &mut Vec<(isize, u32, usize, isize)>, entry: (isize, u32, usize, isize)) {
    let replaces = matches!(entry.1, WM_MOUSEMOVE | WM_NCMOUSEMOVE) || entry.1 == WM_GPUI_REPLAY_BOUNDS;
    if replaces
        && let Some(last) = list.last_mut()
        && last.0 == entry.0
        && last.1 == entry.1
    {
        *last = entry;
        return;
    }
    if list.len() < KEPT_AT_MOST {
        list.push(entry);
    }
}

/// Keeps a message for the end of the task.
pub(crate) fn keep(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
    KEPT.with(|k| kept_with(&mut k.borrow_mut(), (hwnd.0 as isize, msg, wparam.0, lparam.0)));
}

/// Notes that a window's size or place changed during the task: it reads them again when the task has ended.
pub(crate) fn note_bounds(hwnd: HWND) {
    keep(hwnd, WM_GPUI_REPLAY_BOUNDS, WPARAM(0), LPARAM(0));
}

fn post_kept() {
    let list = KEPT.with(|k| std::mem::take(&mut *k.borrow_mut()));
    for (hwnd, msg, wparam, lparam) in list {
        // SAFETY: a plain post; a window that is gone meanwhile makes it fail, which is fine.
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(wparam), LPARAM(lparam));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pointers_messages_are_told_from_the_rest() {
        for m in [0x0200u32, 0x0201, 0x0202, 0x0204, 0x020A, 0x020E, 0x00A0, 0x00A1, 0x00A2, 0x00AD] {
            assert!(is_mouse_message(m), "{m:#x}");
        }
        // WM_NCHITTEST, WM_SIZE, WM_MOVE, WM_KEYDOWN, WM_CHAR, WM_PAINT, WM_TIMER, our own messages
        for m in [0x0084u32, 0x0005, 0x0003, 0x0100, 0x0102, 0x000F, 0x0113, WM_GPUI_REPLAY_BOUNDS] {
            assert!(!is_mouse_message(m), "{m:#x}");
        }
    }

    #[test]
    fn moves_replace_each_other_clicks_are_all_kept_in_order() {
        let mut list = Vec::new();
        kept_with(&mut list, (1, WM_MOUSEMOVE, 0, 10));
        kept_with(&mut list, (1, WM_MOUSEMOVE, 0, 20));
        assert_eq!(list, vec![(1, WM_MOUSEMOVE, 0, 20)], "only the last place");
        kept_with(&mut list, (1, 0x0201, 1, 20));
        kept_with(&mut list, (1, 0x0202, 0, 20));
        kept_with(&mut list, (1, WM_MOUSEMOVE, 0, 30));
        kept_with(&mut list, (2, WM_MOUSEMOVE, 0, 30));
        assert_eq!(list.len(), 5, "a press, a release, and a move for each of two windows after the first");
        assert_eq!((list[1].1, list[2].1), (0x0201, 0x0202), "the click in its order");
        // bounds are noted once for a run of changes of one window
        let mut b = Vec::new();
        kept_with(&mut b, (7, WM_GPUI_REPLAY_BOUNDS, 0, 0));
        kept_with(&mut b, (7, WM_GPUI_REPLAY_BOUNDS, 0, 0));
        kept_with(&mut b, (8, WM_GPUI_REPLAY_BOUNDS, 0, 0));
        assert_eq!(b.len(), 2);
        // and the list has an end
        let mut many = Vec::new();
        for i in 0..1000 {
            kept_with(&mut many, (1, 0x0201, i, 0));
        }
        assert_eq!(many.len(), KEPT_AT_MOST);
    }

    #[test]
    fn a_task_inside_a_task_counts_and_the_end_of_the_outer_one_is_the_end() {
        assert!(!in_task());
        let outer = TaskGuard::enter();
        let inner = TaskGuard::enter();
        assert!(in_task());
        drop(inner);
        assert!(in_task(), "the outer one still runs");
        drop(outer);
        assert!(!in_task());
    }
}
