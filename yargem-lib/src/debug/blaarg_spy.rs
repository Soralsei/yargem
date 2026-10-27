use crate::memory::mmu::{MemoryContext, MemoryHandler, Source};
pub struct BlaargSpy;

impl MemoryHandler for BlaargSpy {
    fn write(&self, address: u16, value: u8, ctx: MemoryContext<'_>) {
        if address == 0xA000 {
            println!("BlaargSpy writing to address 0xA000: {}", value);
            let debug_ctx = ctx.as_source(Source::Debugger);
            let has_signature = debug_ctx.read(0xA001) == 0xDE
                && debug_ctx.read(0xA002) == 0xB0
                && debug_ctx.read(0xA003) == 0x61;
            let previous = debug_ctx.read(address);

            if has_signature && previous == 0x80 {
                let mut result = String::with_capacity(100);
                let mut addr = 0xA004;

                loop {
                    let value = debug_ctx.read(addr);
                    if value == 0 {
                        break;
                    }
                    result.push(value as char);
                    addr += 1;
                }
                println!("Test result: {} ({})", result, value);
            }
        }
        ctx.write(address, value);
    }
}
