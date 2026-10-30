//! Self-updates through Sparkle (macOS only).
//!
//! `scripts/release.sh` embeds `Sparkle.framework` in `Contents/Frameworks`,
//! and [`start`] loads it at runtime. Builds without the framework (`cargo
//! run`, `scripts/install-local.sh`) leave the updater off, and the UI hides
//! its update controls.
//!
//! Sparkle checks the feed in `Info.plist` (`SUFeedURL`) once a day. When a
//! scheduled check finds an update, Sparkle leaves showing it to Bespoke
//! ("gentle reminders"): [`pending_version`] returns the new version, and the
//! sidebar card and the tray menu offer it. [`check_now`] opens Sparkle's own
//! window, which shows the release notes, downloads, installs and relaunches.
//!
//! Every change emits [`STATE_CHANGED`].

/// Emitted when an update is found or dismissed, or a check finishes.
pub const STATE_CHANGED: &str = "update-state-changed";

#[cfg(target_os = "macos")]
pub use macos::{
    automatically_checks, check_now, is_enabled, last_checked, pending_version,
    set_automatically_checks, start,
};

#[cfg(not(target_os = "macos"))]
pub use other::{
    automatically_checks, check_now, is_enabled, last_checked, pending_version,
    set_automatically_checks, start,
};

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObject};
    use objc2::{define_class, msg_send, ClassType, MainThreadMarker};
    use objc2_foundation::{NSBundle, NSDate, NSString};

    use crate::app_handle::AppHandle;

    static ENABLED: AtomicBool = AtomicBool::new(false);
    /// The version a scheduled check found, until its update session ends.
    static PENDING: Mutex<Option<String>> = Mutex::new(None);

    struct Updater {
        controller: Retained<AnyObject>,
        // Sparkle holds its delegates weakly.
        _delegate: Retained<UpdaterDelegate>,
    }

    thread_local! {
        // Sparkle's objects belong to the main thread.
        static UPDATER: RefCell<Option<Updater>> = const { RefCell::new(None) };
    }

    fn set_pending(version: Option<String>) {
        if let Ok(mut pending) = PENDING.lock() {
            *pending = version;
        }
        notify();
    }

    fn notify() {
        if let Err(e) = AppHandle::global().emit(super::STATE_CHANGED, ()) {
            log::warn!("Failed to emit {}: {e}", super::STATE_CHANGED);
        }
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements, and the class has
        // no ivars.
        #[unsafe(super(NSObject))]
        #[name = "BespokeUpdaterDelegate"]
        struct UpdaterDelegate;

        // SPUStandardUserDriverDelegate and SPUUpdaterDelegate. Sparkle checks
        // each optional method with respondsToSelector:.
        impl UpdaterDelegate {
            #[unsafe(method(supportsGentleScheduledUpdateReminders))]
            fn supports_gentle_reminders(&self) -> bool {
                true
            }

            // Bespoke shows scheduled updates itself, in the sidebar and the
            // tray, instead of Sparkle opening a window unprompted.
            #[unsafe(method(standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:))]
            fn should_handle_scheduled_update(&self, _update: &AnyObject, _immediate: bool) -> bool {
                false
            }

            #[unsafe(method(standardUserDriverWillHandleShowingUpdate:forUpdate:state:))]
            fn will_show_update(&self, _handled: bool, update: &AnyObject, _state: &AnyObject) {
                // SAFETY: `update` is an SUAppcastItem, whose
                // displayVersionString is a non-null NSString.
                let version: Retained<NSString> = unsafe { msg_send![update, displayVersionString] };
                log::info!("Update available: {version}");
                set_pending(Some(version.to_string()));
            }

            // The user installed, skipped or postponed the update, or it failed.
            #[unsafe(method(standardUserDriverWillFinishUpdateSession))]
            fn will_finish_session(&self) {
                set_pending(None);
            }

            #[unsafe(method(updater:didFinishUpdateCycleForUpdateCheck:error:))]
            fn did_finish_cycle(&self, _updater: &AnyObject, _check: isize, _error: Option<&AnyObject>) {
                notify();
            }
        }
    );

    /// Loads Sparkle and starts its schedule. Call once, on the main thread.
    pub fn start() {
        if MainThreadMarker::new().is_none() {
            log::error!("updater::start must run on the main thread");
            return;
        }
        // Dev snapshots show the update card without Sparkle.
        #[cfg(debug_assertions)]
        if let Ok(version) = std::env::var("BESPOKE_DEV_UPDATE") {
            set_pending(Some(version));
        }
        let Some(class) = load_sparkle() else {
            return;
        };
        let delegate: Retained<UpdaterDelegate> =
            unsafe { msg_send![UpdaterDelegate::class(), new] };
        // SAFETY: SPUStandardUpdaterController's designated initializer. With
        // startingUpdater YES it begins the update schedule; it reports a
        // misconfigured bundle in an alert rather than failing here.
        let controller: Retained<AnyObject> = unsafe {
            msg_send![
                msg_send![class, alloc],
                initWithStartingUpdater: true,
                updaterDelegate: &*delegate,
                userDriverDelegate: &*delegate
            ]
        };
        UPDATER.with(|slot| {
            *slot.borrow_mut() = Some(Updater {
                controller,
                _delegate: delegate,
            })
        });
        ENABLED.store(true, Ordering::Release);
        log::info!("Sparkle updater started");
        notify();
    }

    /// Sparkle's controller class, if this bundle embeds Sparkle and names a feed.
    fn load_sparkle() -> Option<&'static AnyClass> {
        let main = NSBundle::mainBundle();
        let feed = main.objectForInfoDictionaryKey(&NSString::from_str("SUFeedURL"));
        let frameworks = main.privateFrameworksPath();
        let (Some(_), Some(frameworks)) = (feed, frameworks) else {
            log::info!("Updates are off: this build has no update feed");
            return None;
        };
        let path = NSString::from_str(&format!("{frameworks}/Sparkle.framework"));
        let Some(bundle) = NSBundle::bundleWithPath(&path) else {
            log::info!("Updates are off: this build doesn't embed Sparkle");
            return None;
        };
        // SAFETY: loading runs Sparkle's initializers, which have no
        // preconditions beyond running in-process.
        if !unsafe { bundle.load() } {
            log::error!("Updates are off: Sparkle.framework failed to load");
            return None;
        }
        let class = AnyClass::get(c"SPUStandardUpdaterController");
        if class.is_none() {
            log::error!("Updates are off: Sparkle has no SPUStandardUpdaterController");
        }
        class
    }

    fn with_updater<R>(f: impl FnOnce(&AnyObject) -> R) -> Option<R> {
        MainThreadMarker::new()?;
        UPDATER.with(|slot| {
            let slot = slot.borrow();
            let updater = slot.as_ref()?;
            // SAFETY: SPUStandardUpdaterController's updater is a non-null SPUUpdater.
            let spu: Retained<AnyObject> = unsafe { msg_send![&*updater.controller, updater] };
            Some(f(&spu))
        })
    }

    /// Whether Sparkle is running. Safe from any thread.
    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::Acquire)
    }

    /// The version a scheduled check found and the user hasn't acted on.
    /// Safe from any thread.
    pub fn pending_version() -> Option<String> {
        PENDING.lock().ok().and_then(|pending| pending.clone())
    }

    /// Opens Sparkle's window: the pending update, or a fresh check.
    pub fn check_now() {
        if MainThreadMarker::new().is_none() {
            return;
        }
        UPDATER.with(|slot| {
            if let Some(updater) = slot.borrow().as_ref() {
                // SAFETY: checkForUpdates: takes an optional sender.
                let _: () = unsafe {
                    msg_send![&*updater.controller, checkForUpdates: Option::<&AnyObject>::None]
                };
            }
        });
    }

    /// Whether Sparkle checks once a day. Sparkle stores this in user defaults.
    pub fn automatically_checks() -> bool {
        // SAFETY: a BOOL property of SPUUpdater.
        with_updater(|spu| unsafe { msg_send![spu, automaticallyChecksForUpdates] })
            .unwrap_or(false)
    }

    pub fn set_automatically_checks(enabled: bool) {
        // SAFETY: a BOOL property of SPUUpdater.
        with_updater(|spu| {
            let _: () = unsafe { msg_send![spu, setAutomaticallyChecksForUpdates: enabled] };
        });
        notify();
    }

    /// When Sparkle last checked the feed.
    pub fn last_checked() -> Option<SystemTime> {
        with_updater(|spu| {
            // SAFETY: a nullable NSDate property of SPUUpdater.
            let date: Option<Retained<NSDate>> = unsafe { msg_send![spu, lastUpdateCheckDate] };
            date
        })
        .flatten()
        .and_then(|date| {
            let seconds = date.timeIntervalSince1970();
            (seconds > 0.).then(|| SystemTime::UNIX_EPOCH + Duration::from_secs_f64(seconds))
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod other {
    use std::time::SystemTime;

    pub fn start() {}

    pub fn is_enabled() -> bool {
        false
    }

    pub fn pending_version() -> Option<String> {
        None
    }

    pub fn check_now() {}

    pub fn automatically_checks() -> bool {
        false
    }

    pub fn set_automatically_checks(_enabled: bool) {}

    pub fn last_checked() -> Option<SystemTime> {
        None
    }
}
