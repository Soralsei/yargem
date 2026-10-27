#![allow(dead_code)]

use std::ops::RangeInclusive;

use bitvec::array::BitArray;

use crate::debug::protocol::WatchId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AccessKind {
    Read,
    Write,
}

pub struct Watchpoint {
    bank: Option<u16>,
    range: RangeInclusive<u16>,
    kind: AccessKind,
}

impl Watchpoint {
    pub fn new(bank: Option<u16>, range: RangeInclusive<u16>, kind: AccessKind) -> Watchpoint {
        Self { bank, range, kind }
    }

    pub fn hit(&self, addr: u16, bank: Option<u16>, access: AccessKind) -> bool {
        if access != self.kind {
            return false;
        }
        let in_bank = match (self.bank, bank) {
            (Some(watched), Some(mapped)) => watched == mapped,
            _ => true,
        };
        self.range.contains(&addr) && in_bank
    }

    pub fn range(&self) -> RangeInclusive<u16> {
        self.range.clone()
    }

    pub fn access(&self) -> AccessKind {
        self.kind
    }
}

pub struct WatchSet {
    watches: Vec<(WatchId, Watchpoint)>,
    read_map: BitArray<[u64; 1024]>,
    write_map: BitArray<[u64; 1024]>,
}

impl WatchSet {
    pub fn new() -> Self {
        Self {
            watches: Vec::with_capacity(15),
            read_map: BitArray::ZERO,
            write_map: BitArray::ZERO,
        }
    }

    pub fn add(&mut self, id: WatchId, watchpoint: Watchpoint) {
        self.watches.retain(|(i, _)| *i != id);
        self.watches.push((id, watchpoint));
        self.rebuild_maps();
    }

    pub fn remove(&mut self, id: WatchId) {
        self.watches.retain(|(i, _)| *i != id);
        self.rebuild_maps();
    }

    pub fn is_watched(&self, addr: u16, kind: AccessKind) -> bool {
        // direct indexing is safe because addr is u16 and maps are u64*1024
        match kind {
            AccessKind::Read => self.read_map[addr as usize],
            AccessKind::Write => self.write_map[addr as usize],
        }
    }

    fn rebuild_maps(&mut self) {
        self.read_map = BitArray::ZERO;
        self.write_map = BitArray::ZERO;
        for (_, w) in &self.watches {
            let (start, end) = w.range().into_inner();
            match w.access() {
                AccessKind::Read => self.read_map[start as usize..=end as usize].fill(true),
                AccessKind::Write => self.write_map[start as usize..=end as usize].fill(true),
            }
        }
    }

    pub fn hit(&self, addr: u16, bank: Option<u16>, access: AccessKind) -> Option<WatchId> {
        self.watches.iter().find_map(|(id, w)| {
            if w.hit(addr, bank, access) {
                Some(*id)
            } else {
                None
            }
        })
    }

    pub fn has_watches(&self) -> bool {
        !self.watches.is_empty()
    }
}
