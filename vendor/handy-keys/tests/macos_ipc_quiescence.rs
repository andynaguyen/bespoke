//! handy-keys ≤ 0.3.2 polled CGEventTapIsEnabled every 100ms from the event
//! tap run loop. Each call is a synchronous mach RPC to WindowServer; on
//! macOS 26 those RPCs leaked kernel IPC vouchers until the machine kernel-
//! panicked with "Cannot grow ipc space beyond IVAC_ENTRIES_MAX"
//! (cjpais/Handy#1827). The fix made tap recovery event-driven, so an idle
//! listener must send (nearly) zero mach messages.

#![cfg(target_os = "macos")]

use std::sync::Mutex;
use std::time::Duration;

use handy_keys::KeyboardListener;

static SERIAL: Mutex<()> = Mutex::new(());

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct TaskEventsInfo {
    faults: i32,
    pageins: i32,
    cow_faults: i32,
    messages_sent: i32,
    messages_received: i32,
    syscalls_mach: i32,
    syscalls_unix: i32,
    csw: i32,
}

const TASK_EVENTS_INFO: u32 = 2;

extern "C" {
    static mach_task_self_: u32;
    fn task_info(task: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
}

fn messages_sent() -> i64 {
    let mut info = TaskEventsInfo::default();
    let mut count = (std::mem::size_of::<TaskEventsInfo>() / 4) as u32;
    let kr = unsafe {
        task_info(
            mach_task_self_,
            TASK_EVENTS_INFO,
            &mut info as *mut TaskEventsInfo as *mut i32,
            &mut count,
        )
    };
    assert_eq!(kr, 0, "task_info(TASK_EVENTS_INFO) failed: {kr}");
    info.messages_sent as i64
}

#[test]
fn idle_listener_sends_no_recurring_ipc() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    let listener = match KeyboardListener::new() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SKIPPED: cannot create listener ({e})");
            return;
        }
    };

    std::thread::sleep(Duration::from_millis(500));

    let mut best = i64::MAX;
    for window in 1..=3 {
        let before = messages_sent();
        std::thread::sleep(Duration::from_secs(5));
        let delta = messages_sent() - before;
        best = best.min(delta);
        println!("window {window}: {delta} mach messages sent during 5s idle");
        if best < 25 {
            break;
        }
    }

    assert!(
        best < 25,
        "idle listener sent ≥{best} mach messages in every 5s window — \
         recurring IPC (e.g. a CGEventTapIsEnabled watchdog) has crept back \
         in; that pattern kernel-panics macOS (cjpais/Handy#1827)"
    );

    drop(listener);
}

#[test]
fn shutdown_is_prompt() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

    for _ in 0..5 {
        let listener = match KeyboardListener::new() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("SKIPPED: cannot create listener ({e})");
                return;
            }
        };
        let start = std::time::Instant::now();
        drop(listener);
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "listener drop took {elapsed:?} — tap thread failed to stop promptly"
        );
    }
}
