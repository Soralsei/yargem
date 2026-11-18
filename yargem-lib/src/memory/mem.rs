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
pub type ExternalRam = BankedMemory<RAM_BANK_SIZE>;

pub struct CartRam<const PAGE_SIZE: usize = RAM_BANK_SIZE> {
    mem: ExternalRam,
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
