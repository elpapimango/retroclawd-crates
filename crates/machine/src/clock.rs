use std::time::{Duration, Instant};

use crate::Machine;

/// Paces `Machine::step` calls by running in fixed-size clock-cycle budgets
/// ("ticks") and sleeping off whatever real time each tick finishes early by.
/// The pacing rate (`ticks_per_sec`, the frame/refresh rate) is supplied by
/// the caller at construction — in practice, every machine's own `run()`
/// passes `self.frame_hz()`. The CPU speed (`clock_hz()` in MHz) comes from
/// the machine itself — so a ZX81 always runs at its own speed and a Spectrum
/// at its own speed, regardless of the caller's pacing budget.
///
/// "Cycles" here is deliberately generic, not the Z80-specific "T-state" --
/// this crate is CPU-agnostic, so a future 6502/68000 machine's `step()`
/// returns its own chip's cycle count, whatever that chip calls it
/// internally. `crates/z80` itself still uses "T-states" throughout, since
/// that's the correct, precise term for that CPU specifically.
pub struct Clock {
    ticks_per_sec: u64,
}

pub struct RunStats {
    pub instructions: u64,
    pub cycles: u64,
    pub elapsed: Duration,
}

impl Clock {
    pub fn new(ticks_per_sec: u64) -> Self {
        Self { ticks_per_sec }
    }

    /// Runs until `running` returns false, checked between instructions.
    pub fn run(
        &self,
        machine: &mut dyn Machine,
        mut running: impl FnMut(&dyn Machine) -> bool,
    ) -> RunStats {
        let hz = machine.clock_hz();
        // `.max(1)` keeps the inner budget loop reachable for a machine whose
        // clock rate is below `ticks_per_sec`; without it the budget would be
        // 0 and `run` would sleep forever without ever stepping the machine.
        let cycles_per_tick = (hz / self.ticks_per_sec).max(1);
        let tick_duration = Duration::from_secs_f64(1.0 / self.ticks_per_sec as f64);

        let mut instructions = 0u64;
        let mut cycles = 0u64;
        let start = Instant::now();

        while running(machine) {
            let tick_start = Instant::now();
            let mut budget = cycles_per_tick;

            // Once per tick, matching `frame_hz()` -- e.g. a ZX Spectrum
            // ULA's real 50Hz `INT` line. See `Machine::request_interrupt`'s
            // doc comment.
            machine.request_interrupt();

            while budget > 0 && running(machine) {
                let cost = machine.step() as u64;
                instructions += 1;
                cycles += cost;
                budget = budget.saturating_sub(cost);
            }

            let elapsed = tick_start.elapsed();
            if elapsed < tick_duration {
                std::thread::sleep(tick_duration - elapsed);
            }
        }

        RunStats {
            instructions,
            cycles,
            elapsed: start.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeMachine {
        hz: u64,
        steps_remaining: u32,
        cost_per_step: u32,
    }

    impl Machine for FakeMachine {
        fn clock_hz(&self) -> u64 {
            self.hz
        }
        fn frame_hz(&self) -> u64 {
            60
        }
        fn run(&mut self, running: &mut dyn FnMut(&dyn Machine) -> bool) -> RunStats {
            Clock::new(self.frame_hz()).run(self, running)
        }
        fn step(&mut self) -> u32 {
            self.steps_remaining -= 1;
            self.cost_per_step
        }
        fn halted(&self) -> bool {
            self.steps_remaining == 0
        }
        fn pc(&self) -> u16 {
            0
        }
        fn registers(&self) -> Vec<(&'static str, u64)> {
            Vec::new()
        }
        fn read_memory(&self, _addr: u16) -> u8 {
            0
        }
        fn load(&mut self, _addr: u16, bytes: &[u8]) -> usize {
            bytes.len()
        }
        fn set_pc(&mut self, _addr: u16) {}
        fn reset(&mut self) {}
        fn save_state(&self) -> Vec<u8> {
            Vec::new()
        }
        fn load_state(&mut self, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn run_executes_until_halted_and_reports_totals() {
        let mut machine = FakeMachine {
            hz: 1_000_000,
            steps_remaining: 5,
            cost_per_step: 4,
        };
        let clock = Clock::new(60);

        let stats = clock.run(&mut machine, |m| !m.halted());

        assert_eq!(stats.instructions, 5);
        assert_eq!(stats.cycles, 20);
    }

    #[test]
    fn run_still_makes_progress_below_one_cycle_per_tick() {
        // hz < ticks_per_sec would make the per-tick budget 0, so `run` would
        // sleep forever without ever stepping. Two steps at one cycle of
        // budget per tick means this finishes in ~2 ticks (~33ms).
        let mut machine = FakeMachine {
            hz: 30,
            steps_remaining: 2,
            cost_per_step: 4,
        };
        let clock = Clock::new(60);

        let stats = clock.run(&mut machine, |m| !m.halted());

        assert_eq!(stats.instructions, 2);
        assert!(machine.halted());
    }

    #[test]
    fn different_tick_rates_change_how_many_steps_run_per_tick() {
        use std::cell::Cell;

        // hz=100, cost_per_step=10: at ticks_per_sec=5, budget=100/5=20
        // cycles/tick -- fits two 10-cycle steps per tick, so all 4
        // steps finish within 2 ticks. At ticks_per_sec=100, budget=1
        // cycle/tick -- less than one step's cost, so only one step fits
        // per tick, needing 4 ticks. `running` is called once per step
        // attempt plus once per tick boundary, so counting its calls
        // reveals how many ticks each rate actually took.
        let calls = Cell::new(0u32);

        let mut slow = FakeMachine {
            hz: 100,
            steps_remaining: 4,
            cost_per_step: 10,
        };
        let slow_stats = Clock::new(5).run(&mut slow, |m| {
            calls.set(calls.get() + 1);
            !m.halted()
        });
        let calls_at_slow_rate = calls.get();

        calls.set(0);
        let mut fast = FakeMachine {
            hz: 100,
            steps_remaining: 4,
            cost_per_step: 10,
        };
        let fast_stats = Clock::new(100).run(&mut fast, |m| {
            calls.set(calls.get() + 1);
            !m.halted()
        });

        assert_eq!(slow_stats.instructions, 4);
        assert_eq!(fast_stats.instructions, 4);
        // Same total work, different tick-boundary structure -- proves the
        // rate passed to `Clock::new` drives the budget math.
        assert_eq!(calls_at_slow_rate, 7);
        assert_eq!(calls.get(), 9);
    }

    struct CountingOverride {
        hz: u64,
        steps_remaining: u32,
        override_calls: u32,
    }

    impl Machine for CountingOverride {
        fn clock_hz(&self) -> u64 {
            self.hz
        }
        fn frame_hz(&self) -> u64 {
            60
        }
        fn run(&mut self, running: &mut dyn FnMut(&dyn Machine) -> bool) -> RunStats {
            // A deliberately non-default body, standing in for a future
            // machine with its own timing model -- proves override, not
            // just default dispatch, works through a trait object.
            self.override_calls += 1;
            Clock::new(self.frame_hz()).run(self, running)
        }
        fn step(&mut self) -> u32 {
            self.steps_remaining -= 1;
            4
        }
        fn halted(&self) -> bool {
            self.steps_remaining == 0
        }
        fn pc(&self) -> u16 {
            0
        }
        fn registers(&self) -> Vec<(&'static str, u64)> {
            Vec::new()
        }
        fn read_memory(&self, _addr: u16) -> u8 {
            0
        }
        fn load(&mut self, _addr: u16, bytes: &[u8]) -> usize {
            bytes.len()
        }
        fn set_pc(&mut self, _addr: u16) {}
        fn reset(&mut self) {}
        fn save_state(&self) -> Vec<u8> {
            Vec::new()
        }
        fn load_state(&mut self, _bytes: &[u8]) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn run_dispatches_through_a_boxed_trait_object_and_can_be_overridden() {
        let mut boxed: Box<dyn Machine> = Box::new(CountingOverride {
            hz: 1_000_000,
            steps_remaining: 3,
            override_calls: 0,
        });

        let stats = boxed.run(&mut |m| !m.halted());

        assert_eq!(stats.instructions, 3);
        assert!(boxed.halted());
    }

    #[test]
    fn render_frame_defaults_to_none_when_not_overridden() {
        let machine = FakeMachine {
            hz: 1_000_000,
            steps_remaining: 0,
            cost_per_step: 4,
        };

        assert_eq!(machine.render_frame(), None);
    }
}
