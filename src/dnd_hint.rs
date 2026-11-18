//! rustyfiles fork: lets the application choose the drop effect shown to the user while files
//! are dragged over its window (upstream always reports "copy").
//!
//! The effect returned from `Drop` stays `DROPEFFECT_COPY` on purpose: the application performs
//! the copy/move itself, and returning MOVE would make some sources delete the originals.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};

/// MK_ALT (0x20) is not exported by the windows crate.
const MK_ALT_BIT: u32 = 0x20;

pub const NONE: u32 = 0;
pub const COPY: u32 = 1;
pub const MOVE: u32 = 2;
pub const LINK: u32 = 4;

static HINT: AtomicU32 = AtomicU32::new(COPY);

/// The effects the source of the drag over a window allows (the `pdwEffect` of its `DoDragDrop`), while one is over it; every
/// effect when none is. A source that allows only COPY (an archive's files extracted for the drag, a phone, a mail's attachment)
/// must be copied: `allowed()` lets the application see that while it handles the drop.
static ALLOWED: AtomicU32 = AtomicU32::new(COPY | MOVE | LINK);

/// The effects the source of the current drag allows (`COPY | MOVE | LINK` when no drag from another program is over a window).
pub fn allowed() -> u32 {
    ALLOWED.load(Ordering::Relaxed)
}

pub(crate) fn set_allowed(ok: DROPEFFECT) {
    ALLOWED.store(ok.0, Ordering::Relaxed);
}

pub(crate) fn clear_allowed() {
    ALLOWED.store(COPY | MOVE | LINK, Ordering::Relaxed);
}

/// [`effect_for`], and one the source allows: an effect it does not allow makes Windows refuse the drop, and the user sees nothing
/// happen (an archive's copy-only drag over a window that would move). Then COPY, MOVE or LINK, the first the source allows.
pub(crate) fn choose(keys: MODIFIERKEYS_FLAGS, ok: DROPEFFECT) -> DROPEFFECT {
    pick(effect_for(keys).0, ok.0)
}

/// The effect `Drop` reports: COPY, or the first effect the source allows when COPY is not among them (a source that says nothing
/// gets COPY, as before).
pub(crate) fn for_drop(ok: DROPEFFECT) -> DROPEFFECT {
    if ok.0 == 0 { DROPEFFECT(COPY) } else { pick(COPY, ok.0) }
}

fn pick(want: u32, ok: u32) -> DROPEFFECT {
    if want & ok != 0 {
        return DROPEFFECT(want);
    }
    [COPY, MOVE, LINK].into_iter().find(|e| e & ok != 0).map(DROPEFFECT).unwrap_or(DROPEFFECT(NONE))
}

#[cfg(test)]
mod pick_tests {
    use super::*;

    #[test]
    fn an_effect_the_source_does_not_allow_falls_back_to_one_it_does() {
        assert_eq!(pick(MOVE, COPY | MOVE).0, MOVE);
        assert_eq!(pick(MOVE, COPY).0, COPY);
        assert_eq!(pick(COPY, MOVE).0, MOVE);
        assert_eq!(pick(LINK, COPY | MOVE).0, COPY);
        assert_eq!(pick(MOVE, 0).0, NONE);
    }
}

/// Effect to show when no modifier key is held (e.g. MOVE for same-volume drops).
pub fn set(effect: u32) {
    HINT.store(effect, Ordering::Relaxed);
}

pub(crate) fn effect_for(keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
    let k = keys.0;
    let ctrl = k & MK_CONTROL.0 != 0;
    let shift = k & MK_SHIFT.0 != 0;
    let alt = k & MK_ALT_BIT != 0;
    if (ctrl && shift) || alt {
        DROPEFFECT_LINK
    } else if ctrl {
        DROPEFFECT_COPY
    } else if shift {
        DROPEFFECT_MOVE
    } else {
        DROPEFFECT(HINT.load(Ordering::Relaxed))
    }
}
