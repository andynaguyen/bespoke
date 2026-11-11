use std::process::Command;

use log::debug;
use parking_lot::Mutex;

const LOWEST_UNMUTED_VOLUME: u32 = 1;

static LOWERING: Mutex<Lowering> = Mutex::new(Lowering {
    holds: 0,
    undo: None,
    closed: false,
});

/// Keeps the output down for one recording. Recordings can overlap, and dropping the last hold
/// puts the output back.
#[must_use]
pub struct Hold(());

impl Drop for Hold {
    fn drop(&mut self) {
        LOWERING.lock().release(read_output, run);
    }
}

/// Turns the output down to `level` of its current volume, from 0.0 for silence to 1.0 for no
/// change. Returns no hold at 1.0 or once the app is quitting.
pub fn lower(level: f32) -> Option<Hold> {
    let percent = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
    LOWERING
        .lock()
        .acquire(percent, read_output, run)
        .then_some(Hold(()))
}

pub fn restore_and_stop_lowering() {
    LOWERING.lock().close(read_output, run);
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Output {
    volume: Option<u8>,
    muted: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Mute(bool),
    Volume(u8),
}

#[derive(Debug, Clone, Copy)]
struct Undo {
    change: Change,
    lowered: Output,
}

impl Undo {
    fn output_unchanged_since_lowering(&self, now: Output) -> bool {
        match self.change {
            Change::Mute(_) => now.muted == self.lowered.muted,
            Change::Volume(_) => now.volume == self.lowered.volume,
        }
    }
}

#[derive(Debug, Default)]
struct Lowering {
    holds: usize,
    undo: Option<Undo>,
    closed: bool,
}

impl Lowering {
    fn acquire(
        &mut self,
        percent: u8,
        read: impl Fn() -> Output,
        apply: impl FnOnce(Change),
    ) -> bool {
        if percent >= 100 || self.closed {
            return false;
        }
        self.holds += 1;
        if self.holds == 1
            && let Some((change, undo)) = plan(percent, read())
        {
            apply(change);
            self.undo = Some(Undo {
                change: undo,
                lowered: read(),
            });
        }
        true
    }

    fn release(&mut self, read: impl FnOnce() -> Output, apply: impl FnOnce(Change)) {
        self.holds = self.holds.saturating_sub(1);
        if self.holds == 0 {
            self.put_back(read, apply);
        }
    }

    fn close(&mut self, read: impl FnOnce() -> Output, apply: impl FnOnce(Change)) {
        self.closed = true;
        self.put_back(read, apply);
    }

    fn put_back(&mut self, read: impl FnOnce() -> Output, apply: impl FnOnce(Change)) {
        if let Some(undo) = self.undo.take()
            && undo.output_unchanged_since_lowering(read())
        {
            apply(undo.change);
        }
    }
}

fn plan(percent: u8, before: Output) -> Option<(Change, Change)> {
    if before.muted == Some(true) {
        return None;
    }
    if percent == 0 {
        return Some((Change::Mute(true), Change::Mute(false)));
    }
    let volume = before.volume.filter(|volume| *volume > 0)?;
    let lowered =
        ((u32::from(volume) * u32::from(percent) + 50) / 100).max(LOWEST_UNMUTED_VOLUME) as u8;
    Some((Change::Volume(lowered), Change::Volume(volume)))
}

fn read_output() -> Output {
    let script = [
        "-e",
        "set s to get volume settings",
        "-e",
        r#"return (output volume of s as text) & "," & (output muted of s as text)"#,
    ];
    match Command::new("osascript").args(script).output() {
        Ok(out) if out.status.success() => parse_output(&String::from_utf8_lossy(&out.stdout)),
        _ => Output::default(),
    }
}

fn parse_output(text: &str) -> Output {
    let (volume, muted) = text.trim().split_once(',').unwrap_or_default();
    Output {
        volume: volume.parse().ok(),
        muted: muted.parse().ok(),
    }
}

fn run(change: Change) {
    debug!("Changing the system output: {change:?}");
    let script = match change {
        Change::Mute(muted) => format!("set volume output muted {muted}"),
        Change::Volume(volume) => format!("set volume output volume {volume}"),
    };
    let _ = Command::new("osascript").args(["-e", &script]).output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const AUDIBLE: Output = Output {
        volume: Some(60),
        muted: Some(false),
    };

    #[test]
    fn zero_percent_mutes_and_restoring_unmutes() {
        assert_eq!(
            plan(0, AUDIBLE),
            Some((Change::Mute(true), Change::Mute(false)))
        );
    }

    #[test]
    fn a_partial_level_scales_the_current_volume() {
        assert_eq!(
            plan(25, AUDIBLE),
            Some((Change::Volume(15), Change::Volume(60)))
        );
    }

    #[test]
    fn a_low_level_never_reaches_zero() {
        let quiet = Output {
            volume: Some(2),
            muted: Some(false),
        };
        assert_eq!(plan(1, quiet), Some((Change::Volume(1), Change::Volume(2))));
    }

    #[test]
    fn muted_output_stays_as_it_is() {
        let muted = Output {
            volume: Some(60),
            muted: Some(true),
        };
        assert_eq!(plan(0, muted), None);
        assert_eq!(plan(50, muted), None);
    }

    #[test]
    fn a_device_without_a_volume_can_only_be_muted() {
        let fixed = Output {
            volume: None,
            muted: Some(false),
        };
        assert_eq!(plan(50, fixed), None);
        assert!(plan(0, fixed).is_some());
    }

    #[test]
    fn parses_what_osascript_prints() {
        assert_eq!(parse_output("60,false\n"), AUDIBLE);
        assert_eq!(
            parse_output("missing value,true\n"),
            Output {
                volume: None,
                muted: Some(true)
            }
        );
        assert_eq!(parse_output(""), Output::default());
    }

    #[derive(Default)]
    struct Fake {
        output: Cell<Output>,
        log: RefCell<Vec<Change>>,
    }

    impl Fake {
        fn audible() -> Self {
            let fake = Self::default();
            fake.output.set(AUDIBLE);
            fake
        }

        fn set_volume(&self, volume: u8) {
            self.output.set(Output {
                volume: Some(volume),
                ..self.output.get()
            });
        }

        fn apply(&self, change: Change) {
            self.log.borrow_mut().push(change);
            match change {
                Change::Volume(volume) => self.set_volume(volume),
                Change::Mute(muted) => self.output.set(Output {
                    muted: Some(muted),
                    ..self.output.get()
                }),
            }
        }

        fn acquire(&self, lowering: &mut Lowering, percent: u8) -> bool {
            lowering.acquire(percent, || self.output.get(), |c| self.apply(c))
        }

        fn release(&self, lowering: &mut Lowering) {
            lowering.release(|| self.output.get(), |c| self.apply(c));
        }

        fn changes(&self) -> Vec<Change> {
            self.log.borrow().clone()
        }
    }

    #[test]
    fn full_level_never_reads_or_changes_the_output() {
        let mut lowering = Lowering::default();
        let held = lowering.acquire(
            100,
            || panic!("read the output"),
            |_| panic!("changed the output"),
        );
        assert!(!held);
        assert_eq!(lowering.holds, 0);
    }

    #[test]
    fn the_output_stays_down_until_the_last_hold_ends() {
        let fake = Fake::audible();
        let mut lowering = Lowering::default();
        assert!(fake.acquire(&mut lowering, 25));
        assert!(fake.acquire(&mut lowering, 25));
        assert_eq!(fake.changes(), vec![Change::Volume(15)]);

        fake.release(&mut lowering);
        assert_eq!(fake.changes(), vec![Change::Volume(15)]);

        fake.release(&mut lowering);
        assert_eq!(fake.changes(), vec![Change::Volume(15), Change::Volume(60)]);
    }

    #[test]
    fn muting_then_releasing_unmutes() {
        let fake = Fake::audible();
        let mut lowering = Lowering::default();
        fake.acquire(&mut lowering, 0);
        fake.release(&mut lowering);
        assert_eq!(
            fake.changes(),
            vec![Change::Mute(true), Change::Mute(false)]
        );
    }

    #[test]
    fn a_volume_changed_during_the_recording_is_left_alone() {
        let fake = Fake::audible();
        let mut lowering = Lowering::default();
        fake.acquire(&mut lowering, 25);
        fake.set_volume(40);
        fake.release(&mut lowering);
        assert_eq!(fake.changes(), vec![Change::Volume(15)]);
        assert_eq!(fake.output.get().volume, Some(40));
    }

    #[test]
    fn unmuting_by_hand_during_the_recording_is_left_alone() {
        let fake = Fake::audible();
        let mut lowering = Lowering::default();
        fake.acquire(&mut lowering, 0);
        fake.output.set(AUDIBLE);
        fake.release(&mut lowering);
        assert_eq!(fake.changes(), vec![Change::Mute(true)]);
    }

    #[test]
    fn closing_restores_and_refuses_later_holds() {
        let fake = Fake::audible();
        let mut lowering = Lowering::default();
        fake.acquire(&mut lowering, 25);
        lowering.close(|| fake.output.get(), |c| fake.apply(c));
        assert_eq!(fake.changes(), vec![Change::Volume(15), Change::Volume(60)]);

        assert!(!fake.acquire(&mut lowering, 25));
        fake.release(&mut lowering);
        assert_eq!(fake.changes(), vec![Change::Volume(15), Change::Volume(60)]);
    }
}
