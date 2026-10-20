pub enum Region {
    Rom,
    Sram,
    Vram,
    Wram,
    Unbanked,
}

pub struct PhysAddr {
    pub region: Region,
    pub bank: u16, // MBC5 has 512 ROM banks
    pub offset: u16,
}

pub trait BankResolver {
    fn resolve(&self, address: u16) -> Option<PhysAddr>;
}
