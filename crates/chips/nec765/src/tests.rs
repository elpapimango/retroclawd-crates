use super::*;

#[derive(Clone)]
struct Sec {
    id: SectorId,
    st1: u8,
    st2: u8,
    copies: Vec<Vec<u8>>,
}

fn sec(c: u8, h: u8, r: u8, n: u8, fill: u8) -> Sec {
    Sec {
        id: SectorId {
            cylinder: c,
            head: h,
            sector: r,
            size_code: n,
        },
        st1: 0,
        st2: 0,
        copies: vec![vec![fill; 128usize << n]],
    }
}

/// An in-memory disk: `tracks[cylinder * sides + head]`.
#[derive(Clone)]
struct TestDisk {
    tracks: Vec<Vec<Sec>>,
    sides: u8,
    wp: bool,
    formatted: Vec<(u8, u8, Vec<SectorId>, u8, u8)>,
}

impl TestDisk {
    fn new(sides: u8) -> Self {
        Self {
            tracks: Vec::new(),
            sides,
            wp: false,
            formatted: Vec::new(),
        }
    }

    fn track(mut self, cylinder: u8, head: u8, sectors: Vec<Sec>) -> Self {
        let i = cylinder as usize * self.sides as usize + head as usize;
        if self.tracks.len() <= i {
            self.tracks.resize(i + 1, Vec::new());
        }
        self.tracks[i] = sectors;
        self
    }

    fn slot(&self, cylinder: u8, head: u8) -> Option<&Vec<Sec>> {
        self.tracks
            .get(cylinder as usize * self.sides as usize + head as usize)
    }
}

impl DiskMedium for TestDisk {
    fn sector_count(&self, cylinder: u8, head: u8) -> usize {
        self.slot(cylinder, head).map_or(0, Vec::len)
    }

    fn sector_info(&self, cylinder: u8, head: u8, index: usize) -> Option<SectorInfo> {
        let s = self.slot(cylinder, head)?.get(index)?;
        Some(SectorInfo {
            id: s.id,
            st1: s.st1,
            st2: s.st2,
        })
    }

    fn sector_data(&self, cylinder: u8, head: u8, index: usize, attempt: u32) -> Option<&[u8]> {
        let s = self.slot(cylinder, head)?.get(index)?;
        Some(&s.copies[attempt as usize % s.copies.len()])
    }

    fn sides(&self) -> u8 {
        self.sides
    }

    fn write_protected(&self) -> bool {
        self.wp
    }

    fn write_sector(
        &mut self,
        cylinder: u8,
        head: u8,
        index: usize,
        data: &[u8],
        deleted: bool,
    ) -> bool {
        let i = cylinder as usize * self.sides as usize + head as usize;
        let Some(s) = self.tracks.get_mut(i).and_then(|t| t.get_mut(index)) else {
            return false;
        };
        s.copies = vec![data.to_vec()];
        s.st1 = 0;
        s.st2 = if deleted { 0x40 } else { 0 };
        true
    }

    fn format_track(
        &mut self,
        cylinder: u8,
        head: u8,
        ids: &[SectorId],
        gap3: u8,
        filler: u8,
    ) -> bool {
        self.formatted
            .push((cylinder, head, ids.to_vec(), gap3, filler));
        let sectors = ids
            .iter()
            .map(|id| Sec {
                id: *id,
                st1: 0,
                st2: 0,
                copies: vec![vec![filler; 128usize << id.size_code]],
            })
            .collect();
        let i = cylinder as usize * self.sides as usize + head as usize;
        if self.tracks.len() <= i {
            self.tracks.resize(i + 1, Vec::new());
        }
        self.tracks[i] = sectors;
        true
    }
}

fn fdc(disk: TestDisk) -> Fdc<TestDisk> {
    let mut f = Fdc::new();
    f.insert_disk(disk);
    f
}

fn send(f: &mut Fdc<TestDisk>, bytes: &[u8]) {
    for &b in bytes {
        f.write_data(b);
    }
}

/// What real firmware does after a read command: poll MSR, pull bytes
/// while the controller is in the execution phase.
fn drain_execution(f: &mut Fdc<TestDisk>) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..100_000 {
        let m = f.read_msr();
        if m & RQM == 0 {
            continue;
        }
        if m & EXM == 0 {
            return out;
        }
        assert_eq!(m & DIO, DIO, "execution read must report DIO");
        out.push(f.read_data());
    }
    panic!("execution phase never ended");
}

fn results(f: &mut Fdc<TestDisk>) -> Vec<u8> {
    let mut out = Vec::new();
    while f.read_msr() == RQM | DIO | CB {
        out.push(f.read_data());
    }
    out
}

/// Read Data with the MFM flag, like real AMSDOS: `0x46`.
fn read_cmd(unit: u8, c: u8, h: u8, r: u8, n: u8, eot: u8) -> [u8; 9] {
    [0x46, unit, c, h, r, n, eot, 0x2A, 0xFF]
}

fn write_cmd(unit: u8, c: u8, h: u8, r: u8, n: u8, eot: u8) -> [u8; 9] {
    [0x45, unit, c, h, r, n, eot, 0x2A, 0xFF]
}

fn one_track_disk() -> TestDisk {
    TestDisk::new(2).track(
        0,
        0,
        vec![sec(0, 0, 0xC1, 2, 0x11), sec(0, 0, 0xC2, 2, 0x22)],
    )
}

// ---- status register ---------------------------------------------------

#[test]
fn msr_walks_idle_command_execution_result_idle() {
    let mut f = fdc(one_track_disk());
    assert_eq!(f.read_msr(), 0x80, "idle");
    f.write_data(0x46);
    assert_eq!(f.read_msr(), 0x90, "busy receiving parameters");
    send(&mut f, &[0, 0, 0, 0xC1, 2, 0xC1, 0x2A, 0xFF]);
    assert_eq!(
        f.read_msr(),
        0x10,
        "first read after the command: not ready"
    );
    assert_eq!(f.read_msr(), 0xF0, "execution, controller -> CPU");
    for _ in 0..512 {
        f.read_data();
    }
    assert_eq!(f.read_msr(), 0xD0, "result phase");
    for _ in 0..7 {
        f.read_data();
    }
    assert_eq!(f.read_msr(), 0x80, "idle again");
}

#[test]
fn write_execution_reports_cpu_to_controller_direction() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    f.read_msr(); // consume the not-ready delay
    assert_eq!(f.read_msr(), 0xB0, "execution, CPU -> controller");
}

#[test]
fn unknown_command_gives_a_single_invalid_result_byte() {
    let mut f = fdc(one_track_disk());
    f.write_data(0xE1); // command 0x01 (flags set): not a uPD765A command
    assert_eq!(f.read_msr(), 0xD0);
    assert_eq!(f.read_data(), 0x80);
    assert_eq!(f.read_msr(), 0x80);
}

#[test]
fn scan_commands_are_reported_invalid() {
    let mut f = fdc(one_track_disk());
    f.write_data(0x51);
    assert_eq!(f.read_data(), 0x80);
}

// ---- Specify / Seek / Recalibrate / Sense Interrupt --------------------

#[test]
fn specify_takes_two_parameters_and_has_no_result() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x03, 0xAF]);
    assert_eq!(f.read_msr(), 0x90);
    f.write_data(0x03);
    assert_eq!(f.read_msr(), 0x80);
}

#[test]
fn seek_then_sense_interrupt_reports_seek_end_and_pcn() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x0F, 0x00, 7]);
    assert_eq!(f.read_msr(), 0x80, "no result phase after Seek");
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x20, 7]);
}

#[test]
fn sense_interrupt_with_nothing_pending_is_invalid() {
    let mut f = fdc(one_track_disk());
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x80]);
}

#[test]
fn sense_interrupt_clears_the_pending_interrupt() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x0F, 0x00, 3]);
    f.write_data(0x08);
    results(&mut f);
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x80]);
}

#[test]
fn recalibrate_returns_the_head_to_cylinder_zero() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x0F, 0x00, 9]);
    f.write_data(0x08);
    results(&mut f);
    send(&mut f, &[0x07, 0x00]);
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x20, 0]);
}

#[test]
fn seek_without_a_disk_reports_not_ready_in_st0() {
    let mut f: Fdc<TestDisk> = Fdc::new();
    send(&mut f, &[0x0F, 0x00, 5]);
    f.write_data(0x08);
    let r = results(&mut f);
    assert_eq!(
        r[0] & 0xC8,
        0x48,
        "abnormal termination + not ready: {r:02X?}"
    );
}

#[test]
fn seek_timing_holds_seek_end_back_until_ticked() {
    let mut f = fdc(one_track_disk());
    f.enable_seek_timing();
    send(&mut f, &[0x0F, 0x00, 5]); // 5 cylinders * 80 T-states
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x80], "still travelling");
    f.tick(399);
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x80], "one T-state short");
    f.tick(1);
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x20, 5]);
}

#[test]
fn seek_timing_is_inert_when_not_enabled() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x0F, 0x00, 5]);
    f.tick(0);
    f.write_data(0x08);
    assert_eq!(results(&mut f), vec![0x20, 5]);
}

// ---- Sense Drive Status ------------------------------------------------

#[test]
fn sense_drive_status_reports_ready_and_track0() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x04, 0x00]);
    assert_eq!(results(&mut f), vec![0x30], "RY + T0");
    send(&mut f, &[0x0F, 0x00, 4]);
    f.write_data(0x08);
    results(&mut f);
    send(&mut f, &[0x04, 0x00]);
    assert_eq!(results(&mut f), vec![0x20], "RY, off track 0");
}

#[test]
fn sense_drive_status_without_a_disk_or_with_motor_off() {
    let mut f: Fdc<TestDisk> = Fdc::new();
    send(&mut f, &[0x04, 0x00]);
    assert_eq!(results(&mut f)[0] & 0x68, 0x48, "write-protect/no RY");
    let mut f = fdc(one_track_disk());
    f.set_motor(false);
    send(&mut f, &[0x04, 0x00]);
    assert_eq!(results(&mut f)[0] & 0x20, 0, "RY clear with motor off");
}

// ---- Read Data ---------------------------------------------------------

#[test]
fn read_data_with_mfm_flag_returns_sector_then_end_of_cylinder_status() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    let data = drain_execution(&mut f);
    assert_eq!(data, vec![0x11; 512]);
    // ST0=abnormal termination, ST1=end of cylinder, ST2=0, then CHRN.
    assert_eq!(results(&mut f), vec![0x40, 0x80, 0x00, 0, 0, 0xC1, 2]);
}

#[test]
fn read_data_ignores_sk_and_mt_bits_on_a_normal_sector() {
    for opcode in [0x06, 0x46, 0x66, 0xC6, 0xE6] {
        let mut f = fdc(one_track_disk());
        let mut cmd = read_cmd(0, 0, 0, 0xC2, 2, 0xC2);
        cmd[0] = opcode;
        send(&mut f, &cmd);
        assert_eq!(
            drain_execution(&mut f),
            vec![0x22; 512],
            "opcode {opcode:#04X}"
        );
        assert_eq!(results(&mut f)[..3], [0x40, 0x80, 0x00]);
    }
}

#[test]
fn read_data_chains_sectors_up_to_eot() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC2));
    let data = drain_execution(&mut f);
    assert_eq!(data.len(), 1024);
    assert_eq!(&data[..512], &[0x11; 512][..]);
    assert_eq!(&data[512..], &[0x22; 512][..]);
    assert_eq!(results(&mut f), vec![0x40, 0x80, 0x00, 0, 0, 0xC2, 2]);
}

#[test]
fn read_data_finds_sectors_out_of_rotational_order() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC2, 2, 0xC2));
    assert_eq!(drain_execution(&mut f), vec![0x22; 512]);
    results(&mut f);
    // Head has now passed 0xC2; asking for 0xC1 wraps past the index hole.
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(drain_execution(&mut f), vec![0x11; 512]);
}

#[test]
fn read_data_for_a_missing_sector_reports_no_data() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0x99, 2, 0x99));
    assert_eq!(drain_execution(&mut f), Vec::<u8>::new());
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x04, 0x04, "no data");
}

#[test]
fn read_data_for_a_sector_on_another_cylinder_flags_wrong_cylinder() {
    let disk = TestDisk::new(2).track(0, 0, vec![sec(5, 0, 1, 2, 0)]);
    let mut f = fdc(disk);
    send(&mut f, &read_cmd(0, 0, 0, 1, 2, 1)); // ask for C=0, track holds C=5
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x04, 0x04);
    assert_eq!(r[2] & 0x10, 0x10, "wrong cylinder");
}

#[test]
fn read_data_uses_the_physical_cylinder_not_the_command_c() {
    // Track 1 physically carries sectors whose ID says C=9 (protection
    // style); the head must be seeked to cylinder 1 to find them.
    let disk = TestDisk::new(2)
        .track(0, 0, vec![sec(0, 0, 1, 2, 0xAA)])
        .track(1, 0, vec![sec(9, 0, 1, 2, 0xBB)]);
    let mut f = fdc(disk);
    send(&mut f, &[0x0F, 0x00, 1]);
    f.write_data(0x08);
    results(&mut f);
    send(&mut f, &read_cmd(0, 9, 0, 1, 2, 1));
    assert_eq!(drain_execution(&mut f), vec![0xBB; 512]);
}

#[test]
fn head_bit_selects_the_side_of_a_double_sided_image() {
    let disk = TestDisk::new(2)
        .track(0, 0, vec![sec(0, 0, 1, 2, 0x10)])
        .track(0, 1, vec![sec(0, 1, 1, 2, 0x20)]);
    let mut f = fdc(disk);
    send(&mut f, &read_cmd(0x04, 0, 1, 1, 2, 1)); // HD bit set
    assert_eq!(drain_execution(&mut f), vec![0x20; 512]);
}

#[test]
fn a_single_sided_image_is_always_read_on_side_zero() {
    let disk = TestDisk::new(1).track(0, 0, vec![sec(0, 0, 1, 2, 0x10)]);
    let mut f = fdc(disk);
    send(&mut f, &read_cmd(0x04, 0, 0, 1, 2, 1)); // HD bit set, one-sided image
    assert_eq!(drain_execution(&mut f), vec![0x10; 512]);
}

#[test]
fn read_data_on_an_unformatted_track_reports_missing_address_mark() {
    let mut f = fdc(TestDisk::new(2).track(1, 0, vec![sec(1, 0, 1, 2, 0)]));
    send(&mut f, &read_cmd(0, 0, 0, 1, 2, 1)); // head is over cylinder 0: nothing there
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x01, 0x01);
}

#[test]
fn commands_report_not_ready_without_a_disk_or_motor_or_for_drive_1() {
    let mut nodisk: Fdc<TestDisk> = Fdc::new();
    send(&mut nodisk, &read_cmd(0, 0, 0, 1, 2, 1));
    assert_eq!(results(&mut nodisk)[0] & 0xC8, 0x48);

    let mut nomotor = fdc(one_track_disk());
    nomotor.set_motor(false);
    send(&mut nomotor, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(results(&mut nomotor)[0] & 0xC8, 0x48);

    let mut drive1 = fdc(one_track_disk());
    send(&mut drive1, &read_cmd(1, 0, 0, 0xC1, 2, 0xC1));
    let r = results(&mut drive1);
    assert_eq!(r[0] & 0xCB, 0x49, "not ready, US0 set: {r:02X?}");
}

#[test]
fn a_deleted_sector_read_normally_stops_with_a_control_mark_but_no_error() {
    let mut deleted = sec(0, 0, 1, 2, 0x77);
    deleted.st2 = 0x40;
    let mut f = fdc(TestDisk::new(2).track(0, 0, vec![deleted]));
    send(&mut f, &read_cmd(0, 0, 0, 1, 2, 1));
    assert_eq!(drain_execution(&mut f), vec![0x77; 512]);
    // Control mark set, but it isn't an abnormal termination.
    assert_eq!(results(&mut f), vec![0x00, 0x00, 0x40, 0, 0, 1, 2]);
}

#[test]
fn sk_skips_a_deleted_sector_and_reads_the_next() {
    let mut deleted = sec(0, 0, 1, 2, 0x77);
    deleted.st2 = 0x40;
    let disk = TestDisk::new(2).track(0, 0, vec![deleted, sec(0, 0, 2, 2, 0x88)]);
    let mut f = fdc(disk);
    let mut cmd = read_cmd(0, 0, 0, 1, 2, 2);
    cmd[0] = 0x66; // MFM + SK
    send(&mut f, &cmd);
    assert_eq!(drain_execution(&mut f), vec![0x88; 512]);
}

#[test]
fn read_deleted_data_reads_a_deleted_sector_with_no_control_mark_error() {
    let mut deleted = sec(0, 0, 1, 2, 0x77);
    deleted.st2 = 0x40;
    let mut f = fdc(TestDisk::new(2).track(0, 0, vec![deleted]));
    let mut cmd = read_cmd(0, 0, 0, 1, 2, 1);
    cmd[0] = 0x4C;
    send(&mut f, &cmd);
    assert_eq!(drain_execution(&mut f), vec![0x77; 512]);
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1], 0x80);
}

#[test]
fn a_sector_with_a_data_error_flag_terminates_abnormally() {
    let mut bad = sec(0, 0, 1, 2, 0x55);
    bad.st1 = 0x20;
    bad.st2 = 0x20;
    let mut f = fdc(TestDisk::new(2).track(0, 0, vec![bad]));
    send(&mut f, &read_cmd(0, 0, 0, 1, 2, 1));
    drain_execution(&mut f);
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x20, 0x20, "data error");
    assert_eq!(r[1] & 0x80, 0, "no end-of-cylinder on a real error");
}

#[test]
fn n_zero_reads_dtl_bytes() {
    let disk = TestDisk::new(2).track(0, 0, vec![sec(0, 0, 1, 0, 0x33)]);
    let mut f = fdc(disk);
    let cmd = [0x46, 0, 0, 0, 1, 0, 1, 0x2A, 0x20]; // N=0, DTL=0x20
    send(&mut f, &cmd);
    assert_eq!(drain_execution(&mut f).len(), 0x20);
}

#[test]
fn weak_sectors_cycle_through_their_stored_copies() {
    let mut weak = sec(0, 0, 1, 1, 0);
    weak.copies = vec![vec![0xAA; 256], vec![0xBB; 256]];
    let mut f = fdc(TestDisk::new(2).track(0, 0, vec![weak]));
    let mut seen = Vec::new();
    for _ in 0..4 {
        send(&mut f, &read_cmd(0, 0, 0, 1, 1, 1));
        seen.push(drain_execution(&mut f)[0]);
        results(&mut f);
    }
    assert_eq!(seen, vec![0xAA, 0xBB, 0xAA, 0xBB]);
}

// ---- Read ID / Read Track ----------------------------------------------

#[test]
fn read_id_returns_successive_sectors() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x4A, 0x00]);
    assert_eq!(results(&mut f), vec![0, 0, 0, 0, 0, 0xC1, 2]);
    send(&mut f, &[0x4A, 0x00]);
    assert_eq!(results(&mut f), vec![0, 0, 0, 0, 0, 0xC2, 2]);
    send(&mut f, &[0x4A, 0x00]);
    assert_eq!(results(&mut f)[5], 0xC1, "wraps past the index hole");
}

#[test]
fn read_id_on_an_unformatted_track_reports_missing_address_mark() {
    let mut f = fdc(TestDisk::new(2));
    send(&mut f, &[0x4A, 0x00]);
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1], 0x01);
}

#[test]
fn read_id_without_a_disk_is_not_ready() {
    let mut f: Fdc<TestDisk> = Fdc::new();
    send(&mut f, &[0x4A, 0x00]);
    assert_eq!(results(&mut f)[0] & 0x48, 0x48);
}

#[test]
fn read_track_returns_the_first_eot_sectors_in_physical_order() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x42, 0, 0, 0, 1, 2, 2, 0x2A, 0xFF]);
    let data = drain_execution(&mut f);
    assert_eq!(data.len(), 1024);
    assert_eq!(data[0], 0x11);
    assert_eq!(data[512], 0x22);
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(
        r[1] & 0x04,
        0x04,
        "sector 1 isn't the first ID (0xC1): no data"
    );
}

// ---- Write Data --------------------------------------------------------

#[test]
fn write_data_stores_the_sector_and_ends_with_end_of_cylinder() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    f.read_msr();
    assert_eq!(f.read_msr() & (DIO | EXM), EXM);
    for i in 0..512u32 {
        f.write_data((i % 251) as u8);
    }
    assert_eq!(results(&mut f), vec![0x40, 0x80, 0x00, 0, 0, 0xC1, 2]);
    let disk = f.disk().unwrap();
    let stored = disk.sector_data(0, 0, 0, 0).unwrap();
    assert_eq!(stored[0], 0);
    assert_eq!(stored[1], 1);
    assert_eq!(stored[500], (500 % 251) as u8);
    // The other sector is untouched.
    assert_eq!(disk.sector_data(0, 0, 1, 0).unwrap(), &[0x22; 512][..]);
}

#[test]
fn write_then_read_back_round_trips() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0xC2, 2, 0xC2));
    for _ in 0..512 {
        f.write_data(0xE7);
    }
    results(&mut f);
    send(&mut f, &read_cmd(0, 0, 0, 0xC2, 2, 0xC2));
    assert_eq!(drain_execution(&mut f), vec![0xE7; 512]);
}

#[test]
fn write_data_chains_sectors_up_to_eot() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0xC1, 2, 0xC2));
    for _ in 0..512 {
        f.write_data(0x01);
    }
    for _ in 0..512 {
        f.write_data(0x02);
    }
    assert_eq!(results(&mut f)[5], 0xC2);
    let d = f.disk().unwrap();
    assert_eq!(d.sector_data(0, 0, 0, 0).unwrap()[0], 0x01);
    assert_eq!(d.sector_data(0, 0, 1, 0).unwrap()[0], 0x02);
}

#[test]
fn write_deleted_data_records_the_deleted_mark() {
    let mut f = fdc(one_track_disk());
    let mut cmd = write_cmd(0, 0, 0, 0xC1, 2, 0xC1);
    cmd[0] = 0x49;
    send(&mut f, &cmd);
    for _ in 0..512 {
        f.write_data(0x5A);
    }
    results(&mut f);
    let info = f.disk().unwrap().sector_info(0, 0, 0).unwrap();
    assert_eq!(info.st2 & 0x40, 0x40);
}

#[test]
fn write_data_to_a_write_protected_disk_reports_not_writable() {
    let mut disk = one_track_disk();
    disk.wp = true;
    let mut f = fdc(disk);
    send(&mut f, &write_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x02, 0x02);
    assert_eq!(
        f.disk().unwrap().sector_data(0, 0, 0, 0).unwrap(),
        &[0x11; 512][..]
    );
}

#[test]
fn write_data_for_a_missing_sector_reports_no_data() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0x77, 2, 0x77));
    let r = results(&mut f);
    assert_eq!(r[0], 0x40);
    assert_eq!(r[1] & 0x04, 0x04);
}

#[test]
fn a_medium_with_default_write_hooks_is_read_only() {
    struct ReadOnly(TestDisk);
    impl DiskMedium for ReadOnly {
        fn sector_count(&self, c: u8, h: u8) -> usize {
            self.0.sector_count(c, h)
        }
        fn sector_info(&self, c: u8, h: u8, i: usize) -> Option<SectorInfo> {
            self.0.sector_info(c, h, i)
        }
        fn sector_data(&self, c: u8, h: u8, i: usize, a: u32) -> Option<&[u8]> {
            self.0.sector_data(c, h, i, a)
        }
    }
    let mut f: Fdc<ReadOnly> = Fdc::new();
    f.insert_disk(ReadOnly(one_track_disk()));
    for &b in &[0x45, 0, 0, 0, 0xC1, 2, 0xC1, 0x2A, 0xFF] {
        f.write_data(b);
    }
    assert_eq!(f.read_msr(), 0xD0);
    assert_eq!(f.read_data() & 0x40, 0x40);
    assert_eq!(f.read_data() & 0x02, 0x02, "not writable");
}

// ---- Format Track ------------------------------------------------------

#[test]
fn format_track_builds_the_requested_sector_table() {
    let mut f = fdc(TestDisk::new(2));
    send(&mut f, &[0x4D, 0x00, 2, 3, 0x4E, 0xE5]); // N=2, SC=3, GPL, filler
    f.read_msr();
    assert_eq!(f.read_msr() & (DIO | EXM), EXM, "controller wants ID bytes");
    for r in [0xC1u8, 0xC2, 0xC3] {
        for byte in [0, 0, r, 2] {
            f.write_data(byte);
        }
    }
    let result = results(&mut f);
    assert_eq!(result[0], 0x00);
    assert_eq!(result[1], 0x00);
    assert_eq!(result[5], 0xC3, "last sector's R");
    assert_eq!(result[6], 2, "N from the command");
    let disk = f.disk().unwrap();
    assert_eq!(disk.sector_count(0, 0), 3);
    assert_eq!(disk.sector_data(0, 0, 1, 0).unwrap(), &[0xE5; 512][..]);
    assert_eq!(disk.formatted.len(), 1);
    assert_eq!(disk.formatted[0].3, 0x4E, "gap 3 passed through");
}

#[test]
fn format_then_read_back_a_freshly_formatted_track() {
    let mut f = fdc(TestDisk::new(2));
    send(&mut f, &[0x4D, 0x00, 1, 2, 0x2A, 0x9C]);
    for byte in [0, 0, 0x41, 1, 0, 0, 0x42, 1] {
        f.write_data(byte);
    }
    results(&mut f);
    send(&mut f, &read_cmd(0, 0, 0, 0x42, 1, 0x42));
    assert_eq!(drain_execution(&mut f), vec![0x9C; 256]);
}

#[test]
fn format_on_a_write_protected_disk_is_refused() {
    let mut disk = TestDisk::new(2);
    disk.wp = true;
    let mut f = fdc(disk);
    send(&mut f, &[0x4D, 0x00, 2, 1, 0x4E, 0xE5]);
    let r = results(&mut f);
    assert_eq!(r[0] & 0x40, 0x40);
    assert_eq!(r[1] & 0x02, 0x02);
}

#[test]
fn format_targets_the_head_bit_side() {
    let mut f = fdc(TestDisk::new(2));
    send(&mut f, &[0x4D, 0x04, 1, 1, 0x2A, 0x11]); // HD bit set
    for byte in [0, 1, 1, 1] {
        f.write_data(byte);
    }
    results(&mut f);
    let disk = f.disk().unwrap();
    assert_eq!(disk.formatted[0].1, 1, "formatted side 1");
}

// ---- Save states -------------------------------------------------------

#[test]
fn state_round_trips_the_cylinder_and_rotational_position() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &[0x0F, 0x00, 0]);
    send(&mut f, &[0x4A, 0x00]);
    results(&mut f); // head now past sector 0xC1
    let mut g = fdc(one_track_disk());
    g.restore_state(f.dump_state());
    send(&mut g, &[0x4A, 0x00]);
    assert_eq!(results(&mut g)[5], 0xC2);
}

#[test]
fn state_round_trips_a_read_in_the_middle_of_its_data_phase() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    f.read_msr();
    for _ in 0..100 {
        f.read_data();
    }
    let state = f.dump_state();
    assert_eq!(state.phase_label(), "execution");

    let mut g = fdc(one_track_disk());
    g.restore_state(state);
    let rest = drain_execution(&mut g);
    assert_eq!(rest.len(), 412, "resumes exactly where it stopped");
    assert_eq!(results(&mut g)[..3], [0x40, 0x80, 0x00]);
}

#[test]
fn state_round_trips_a_write_in_the_middle_of_its_data_phase() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &write_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    for _ in 0..200 {
        f.write_data(0x42);
    }
    let mut g = fdc(one_track_disk());
    g.restore_state(f.dump_state());
    for _ in 0..312 {
        g.write_data(0x42);
    }
    results(&mut g);
    assert_eq!(
        g.disk().unwrap().sector_data(0, 0, 0, 0).unwrap(),
        &[0x42; 512][..]
    );
}

#[test]
fn state_round_trips_a_pending_seek_and_motor() {
    let mut f = fdc(one_track_disk());
    f.set_motor(false);
    let mut g = fdc(one_track_disk());
    g.restore_state(f.dump_state());
    assert!(!g.motor());
}

#[test]
fn state_serialises_with_bincode() {
    let mut f = fdc(one_track_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    let bytes = bincode::serde::encode_to_vec(f.dump_state(), bincode::config::standard()).unwrap();
    let (state, _): (FdcState, usize) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
    let mut g = fdc(one_track_disk());
    g.restore_state(state);
    assert_eq!(drain_execution(&mut g), vec![0x11; 512]);
}

// ---- Disk management ---------------------------------------------------

#[test]
fn eject_and_reinsert_a_disk() {
    let mut f = fdc(one_track_disk());
    let disk = f.eject_disk().unwrap();
    assert!(f.disk().is_none());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(results(&mut f)[0] & 0x08, 0x08, "not ready with no disk");
    f.insert_disk(disk);
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(drain_execution(&mut f), vec![0x11; 512]);
}

// ---- two drives -----------------------------------------------------------

fn second_disk() -> TestDisk {
    TestDisk::new(2).track(
        0,
        0,
        vec![sec(0, 0, 0xC1, 2, 0xA1), sec(0, 0, 0xC2, 2, 0xA2)],
    )
}

#[test]
fn unit_select_addresses_two_independent_drives() {
    let mut f = fdc(one_track_disk());
    f.insert_disk_in(1, second_disk());
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(drain_execution(&mut f), vec![0x11; 512], "drive A");
    results(&mut f);
    send(&mut f, &read_cmd(1, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(drain_execution(&mut f), vec![0xA1; 512], "drive B");
    let r = results(&mut f);
    assert_eq!(r[0] & 0x03, 1, "ST0 names unit 1: {r:02X?}");
    // And back to A: the units do not bleed into each other.
    send(&mut f, &read_cmd(0, 0, 0, 0xC2, 2, 0xC2));
    assert_eq!(drain_execution(&mut f), vec![0x22; 512]);
}

#[test]
fn each_drive_keeps_its_own_head_position() {
    let mut f = fdc(one_track_disk());
    f.insert_disk_in(1, second_disk());
    // Seek A to cylinder 7 and B to cylinder 3; sense interrupt reports each one's PCN.
    send(&mut f, &[0x0F, 0x00, 7]);
    send(&mut f, &[0x08]);
    assert_eq!(results(&mut f), vec![0x20, 7]);
    send(&mut f, &[0x0F, 0x01, 3]);
    send(&mut f, &[0x08]);
    assert_eq!(results(&mut f), vec![0x21, 3]);
    assert_eq!(f.dump_state().cylinder_of(0), 7);
    assert_eq!(f.dump_state().cylinder_of(1), 3);
    // A recalibrate of B does not move A's head.
    send(&mut f, &[0x07, 0x01]);
    send(&mut f, &[0x08]);
    results(&mut f);
    assert_eq!(f.dump_state().cylinder_of(0), 7);
    assert_eq!(f.dump_state().cylinder_of(1), 0);
}

#[test]
fn an_empty_second_drive_and_units_2_and_3_are_not_ready() {
    let mut f = fdc(one_track_disk());
    for unit in [1, 2, 3] {
        send(&mut f, &read_cmd(unit, 0, 0, 0xC1, 2, 0xC1));
        let r = results(&mut f);
        assert_eq!(r[0] & 0xC8, 0x48, "unit {unit} not ready: {r:02X?}");
    }
    // Drive A is still fine afterwards.
    send(&mut f, &read_cmd(0, 0, 0, 0xC1, 2, 0xC1));
    assert_eq!(drain_execution(&mut f), vec![0x11; 512]);
    // Inserting into a unit that does not exist does nothing.
    f.insert_disk_in(2, second_disk());
    assert!(f.disk_in(2).is_none());
}

#[test]
fn ejecting_one_drive_leaves_the_other_and_the_old_api_means_drive_a() {
    let mut f = fdc(one_track_disk());
    f.insert_disk_in(1, second_disk());
    assert!(f.disk().is_some() && f.disk_in(1).is_some());
    assert!(f.eject_disk_from(1).is_some());
    assert!(f.disk_in(1).is_none() && f.disk().is_some());
    assert!(f.eject_disk().is_some());
    assert!(f.disk().is_none());
    // The write-protect and motor line are shared; the motor off makes both not ready.
    f.insert_disk_in(0, one_track_disk());
    f.insert_disk_in(1, second_disk());
    f.set_motor(false);
    for unit in [0, 1] {
        send(&mut f, &read_cmd(unit, 0, 0, 0xC1, 2, 0xC1));
        assert_eq!(results(&mut f)[0] & 0xC8, 0x48, "unit {unit}, motor off");
    }
}

#[test]
fn the_state_carries_both_head_positions() {
    let mut f = fdc(one_track_disk());
    f.insert_disk_in(1, second_disk());
    send(&mut f, &[0x0F, 0x00, 7]);
    send(&mut f, &[0x08]);
    results(&mut f);
    send(&mut f, &[0x0F, 0x01, 3]);
    send(&mut f, &[0x08]);
    results(&mut f);
    let state = f.dump_state();
    let mut g = fdc(one_track_disk());
    g.insert_disk_in(1, second_disk());
    g.restore_state(state);
    assert_eq!(g.dump_state().cylinder_of(0), 7);
    assert_eq!(g.dump_state().cylinder_of(1), 3);
}
