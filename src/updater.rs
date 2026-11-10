//! Sparkle checks the feed in `Info.plist` (`SUFeedURL`) once a day. When a
//! scheduled check finds an update, Sparkle leaves showing it to Bespoke
//! ("gentle reminders"): [`pending_version`] returns the new version, and the
//! sidebar card and the tray menu offer it. [`check_now`] opens Sparkle's own
//! window, which shows the release notes, downloads, installs and relaunches.

pub const STATE_CHANGED: &str = "update-state-changed";

pub use macos::{
    automatically_checks, check_now, is_enabled, last_checked, pending_version,
    set_automatically_checks, start,
};

mod macos {
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime};

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObject};
    use objc2::{ClassType, MainThreadMarker, define_class, msg_send};
    use objc2_foundation::{NSBundle, NSDate, NSString};

    use crate::app_handle::AppHandle;

    static ENABLED: AtomicBool = AtomicBool::new(false);
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

    pub fn start() {
        if MainThreadMarker::new().is_none() {
            log::error!("updater::start must run on the main thread");
            return;
        }
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

    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::Acquire)
    }

    pub fn pending_version() -> Option<String> {
        PENDING.lock().ok().and_then(|pending| pending.clone())
    }

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
