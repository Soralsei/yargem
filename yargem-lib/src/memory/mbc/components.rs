use std::{cell::Cell, convert::Infallible, pin::pin};

use crate::{
    clock::{Fixed, Timeline},
    executor::{edge, EdgeListener, EdgeRaiser},
    is_bit_set,
    util::future::{Either, Race},
};

const ROM_BANK_SIZE: usize = 16 * 1024;
pub const RAM_BANK_SIZE: usize = 8 * 1024;
pub const MAX_ROM_SIZE_NORMAL: usize = 512 * 1024;

pub struct BankedMemory<const PAGE_SIZE: usize> {
    data: Box<[u8]>,
    page_mask: usize,
    total_mask: usize,
}

impl<const PAGE_SIZE: usize> BankedMemory<PAGE_SIZE> {
    pub fn new(data: Vec<u8>) -> Self {
        const { assert!(PAGE_SIZE.is_power_of_two()) };
        debug_assert!(data.is_empty() || data.len().is_power_of_two());

        let num_pages = data.len() / PAGE_SIZE;
        let total_mask = data.len().saturating_sub(1);
        Self {
            data: data.into_boxed_slice(),
            page_mask: num_pages.next_power_of_two().saturating_sub(1),
            total_mask,
        }
    }

    pub fn zeroed(size: usize) -> Self {
        Self::new(vec![0u8; size])
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    fn offset(&self, bank: usize, address: u16) -> usize {
        let masked_bank = bank & self.page_mask;
        ((masked_bank * PAGE_SIZE) | (address as usize & (PAGE_SIZE - 1))) & self.total_mask
    }

    #[inline]
    pub fn read(&self, bank: usize, address: u16) -> u8 {
        if self.data.is_empty() {
            return 0xFF;
        }
        self.data[self.offset(bank, address)]
    }

    #[inline]
    pub fn write(&mut self, bank: usize, address: u16, value: u8) {
        if self.data.is_empty() {
            return;
        }
        let idx = self.offset(bank, address);
        self.data[idx] = value;
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

pub type Rom = BankedMemory<ROM_BANK_SIZE>;
pub type Ram = BankedMemory<RAM_BANK_SIZE>;

pub struct CartRam<const PAGE_SIZE: usize = RAM_BANK_SIZE> {
    mem: Ram,
    pub enabled: bool,
    pub dirty: bool,
}

impl<const PAGE_SIZE: usize> CartRam<PAGE_SIZE> {
    pub fn new(size: usize, enabled: bool) -> Self {
        Self {
            mem: BankedMemory::zeroed(size),
            enabled,
            dirty: false,
        }
    }

    #[inline]
    pub fn read(&self, bank: usize, address: u16) -> u8 {
        if !self.enabled {
            return 0xFF;
        }
        self.mem.read(bank, address)
    }

    #[inline]
    pub fn write(&mut self, bank: usize, address: u16, value: u8) {
        if !self.enabled {
            return;
        }
        if !self.mem.is_empty() {
            self.mem.write(bank, address, value);
            self.dirty = true;
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        self.mem.as_slice()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.mem.as_mut_slice()
    }
}

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

    pub async fn task<'a>(&'a self, t: Timeline<Fixed>) -> Infallible {
        let mut writes_listener = self.reset_listener.take().expect("Spawned Rtc task twice");
        loop {
            // wait one second or until a write to seconds register
            let delay_fut = pin!(t.wait(const { 4 * 1024 * 1024 }));
            let writes_fut = pin!(t.after(writes_listener.next()));
            let win = Race::new(writes_fut, delay_fut).await;
            match win {
                Either::Left(_) => continue,
                Either::Right(_) => (),
            }
            self.counters.update(|mut counters| {
                counters.incr();
                counters
            });
        }
    }
}
