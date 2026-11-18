use std::{cell::Cell, convert::Infallible, pin::pin, rc::Rc};

use crate::{
    clock::{Fixed, Timeline},
    executor::{edge, EdgeListener, EdgeRaiser},
    is_bit_set,
    util::future::{Either, Race},
};

#[derive(Clone, Copy)]
struct RtcDh(u8);

impl RtcDh {
    fn with(&self, days_hi: bool, halted: bool, carry: bool) -> Self {
        let mask = 0b11000001;
        let vals = ((carry as u8) << 7) | ((halted as u8) << 6) | (days_hi as u8);

        RtcDh((self.0 & !mask) | vals)
    }

    fn days_hi(&self) -> u16 {
        ((self.0 & 1) as u16) << 8
    }

    fn halted(&self) -> bool {
        is_bit_set!(self.0, 6)
    }

    fn carry(&self) -> bool {
        is_bit_set!(self.0, 7)
    }
}

#[derive(Clone, Copy)]
struct RtcCounters {
    seconds: u8,
    minutes: u8,
    hours: u8,
    days_lo: u8,
    days_hi: RtcDh,
}

impl RtcCounters {
    fn new() -> Self {
        Self {
            seconds: 0,
            minutes: 0,
            hours: 0,
            days_lo: 0,
            days_hi: RtcDh(0),
        }
    }

    fn days(&self) -> u16 {
        self.days_hi.days_hi() | self.days_lo as u16
    }

    fn incr(&mut self) {
        let seconds = self.seconds;
        self.seconds = (seconds + 1) % 60;

        let minutes = self.minutes;
        self.minutes = (minutes + (self.seconds < seconds) as u8) % 60;

        let hours = self.hours;
        self.hours = (hours + (self.minutes < minutes) as u8) % 24;

        let days = self.days();
        let new_days = (days + (self.hours < hours) as u16) % 512;

        let carry = days > new_days;
        let day_hi_bit = is_bit_set!(new_days, 7);

        self.days_lo = (new_days as u8) & 0xFF;
        self.days_hi = self.days_hi.with(day_hi_bit, self.days_hi.halted(), carry);
    }
}

pub struct Rtc {
    counters: Cell<RtcCounters>,
    latched: Cell<[u8; 5]>,
    latch_write: Cell<bool>,

    reset_listener: Cell<Option<EdgeListener>>,
    reset: EdgeRaiser,
}

impl Rtc {
    pub fn new() -> Self {
        let (reset, reset_listener) = edge();
        Self {
            counters: Cell::new(RtcCounters::new()),
            latched: Cell::new([0; 5]),
            latch_write: Cell::new(false),
            reset_listener: Cell::new(Some(reset_listener)),
            reset,
        }
    }

    /// RTC S $08
    pub fn seconds(&self) -> u8 {
        self.counters.get().seconds
    }

    /// RTC M $09
    pub fn minutes(&self) -> u8 {
        self.counters.get().minutes
    }

    /// RTC H $0A
    pub fn hours(&self) -> u8 {
        self.counters.get().hours
    }

    /// RTC DL $0B
    pub fn days_lo(&self) -> u8 {
        self.counters.get().days_lo
    }

    /// RTC DH $0C
    pub fn days_hi(&self) -> u8 {
        self.counters.get().days_hi.0
    }

    /// RTC S $08
    pub fn set_seconds(&self, seconds: u8) {
        self.counters.get().seconds = seconds;
        self.latched.update(|mut latched| {
            latched[0] = seconds;
            latched
        });
        self.reset.raise();
    }

    /// RTC M $09
    pub fn set_minutes(&self, minutes: u8) {
        self.counters.get().minutes = minutes;
        self.latched.update(|mut latched| {
            latched[1] = minutes;
            latched
        });
    }

    /// RTC H $0A
    pub fn set_hours(&self, hours: u8) {
        self.counters.get().hours = hours;
        self.latched.update(|mut latched| {
            latched[2] = hours;
            latched
        });
    }

    /// RTC DL $0B
    pub fn set_days_lo(&self, days_lo: u8) {
        self.counters.get().days_lo = days_lo;
        self.latched.update(|mut latched| {
            latched[3] = days_lo;
            latched
        });
    }

    /// RTC DH $0C
    pub fn set_days_hi(&self, days_hi: u8) {
        self.counters.get().days_hi = RtcDh(days_hi);
        self.latched.update(|mut latched| {
            latched[4] = days_hi;
            latched
        });
        self.reset.raise();
    }

    /// $6000-$7FFF Latch clock data
    pub fn maybe_latch(&self, value: u8) {
        if self.latch_write.take() && value == 1 {
            let counters = self.counters.get();
            let latched = [
                counters.seconds,
                counters.minutes,
                counters.hours,
                counters.days_lo,
                counters.days_hi.0,
            ];
            self.latched.set(latched)
        } else {
            self.latch_write.set(value == 0);
        }
    }

    pub async fn task(this: Rc<Self>, t: Timeline<Fixed>) -> Infallible {
        let mut writes_listener = this.reset_listener.take().expect("Spawned Rtc task twice");
        loop {
            while this.counters.get().days_hi.halted() {
                t.after(writes_listener.next()).await;
            }
            // wait one second or until a write to seconds register
            let delay_fut = pin!(t.wait(const { 4 * 1024 * 1024 }));
            let writes_fut = pin!(t.after(writes_listener.next()));
            let win = Race::new(writes_fut, delay_fut).await;
            match win {
                Either::Left(_) => continue,
                Either::Right(_) => (),
            }
            this.counters.update(|mut counters| {
                counters.incr();
                counters
            });
        }
    }
}
