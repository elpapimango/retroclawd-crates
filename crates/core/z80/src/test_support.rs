use crate::Bus;

pub(crate) struct FlatBus {
    mem: [u8; 0x10000],
}

impl FlatBus {
    pub(crate) fn new() -> Self {
        Self { mem: [0; 0x10000] }
    }
}

impl Bus for FlatBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.mem[addr as usize] = val;
    }

    fn read_io(&mut self, _port: u16) -> u8 {
        0
    }

    fn write_io(&mut self, _port: u16, _val: u8) {}
}
