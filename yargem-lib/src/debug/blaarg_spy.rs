use crate::memory::mmu::{MemoryHandler, MemoryRead, MemoryWrite, Mmu};
pub struct BlaargSpy;

impl MemoryHandler for BlaargSpy {
    fn read(&self, _mmu: &crate::memory::mmu::Mmu, _address: u16) -> MemoryRead {
        MemoryRead::Pass
    }

    fn write(&self, mmu: &Mmu, address: u16, value: u8) -> MemoryWrite {
        if address == 0xA000 {
            println!("BlaargSpy writing to address 0xA000: {}", value);
            let has_signature =
                mmu.peek(0xA001) == 0xDE && mmu.peek(0xA002) == 0xB0 && mmu.peek(0xA003) == 0x61;
            let previous = mmu.peek(address);

            if has_signature && previous == 0x80 {
                let mut result = String::with_capacity(100);
                let mut addr = 0xA004;

                loop {
                    let value = mmu.peek(addr);
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
