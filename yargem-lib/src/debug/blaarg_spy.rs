use crate::memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu};
pub struct BlaargSpy;

impl MemoryHandler for BlaargSpy {
    fn write(&self, mmu: &Mmu, address: u16, value: u8) -> MemoryWrite {
        if address == 0xA000 {
            println!("BlaargSpy writing to address 0xA000: {}", value);
            let has_signature =
                mmu.read(0xA001) == 0xDE && mmu.read(0xA002) == 0xB0 && mmu.read(0xA003) == 0x61;
            let previous = mmu.read(address);

            if has_signature && previous == 0x80 {
                let mut result = String::with_capacity(100);
                let mut addr = 0xA004;

                loop {
                    let value = mmu.read(addr);
                    if value == 0 {
                        break;
                    }
                    result.push(value as char);
                    addr += 1;
                }
                println!("Test result: {} ({})", result, value);
            }
        }
        MemoryWrite::Pass
    }
}
