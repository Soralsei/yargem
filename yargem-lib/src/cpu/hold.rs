use std::{cell::Cell, rc::Rc};

use crate::clock::{signal, Listener, Raiser, Until, Untimed};

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineState {
    Idle,
    Requested,
    Granted,
}

pub struct HoldRequest {
    state: Rc<Cell<LineState>>,
    release: Raiser,
    granted: Listener,
}
pub struct HoldLine {
    state: Rc<Cell<LineState>>,
    grant: Raiser,
    released: Listener,
}

impl HoldRequest {
    pub fn release(&self) {
        self.state.set(LineState::Idle);
        self.release.raise();
    }

    pub fn until_granted(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        self.state.set(LineState::Requested);
        let state = self.state.clone();
        self.granted
            .until(move || state.get() == LineState::Granted)
    }
}

impl HoldLine {
    pub fn requested(&self) -> bool {
        self.state.get() == LineState::Requested
    }

    pub fn grant(&self) {
        self.state.set(LineState::Granted);
        self.grant.raise();
    }

    pub fn until_released(&mut self) -> Untimed<Until<'_, impl Fn() -> bool + '_>> {
        let state = self.state.clone();
        self.released.until(move || state.get() == LineState::Idle)
    }
}

pub fn hold_line() -> (HoldRequest, HoldLine) {
    let held = Rc::new(Cell::new(LineState::Idle));
    let (release, released) = signal();
    let (grant, granted) = signal();
    (
        HoldRequest {
            state: held.clone(),
            release,
            granted,
        },
        HoldLine {
            state: held,
            grant,
            released,
        },
    )
}
