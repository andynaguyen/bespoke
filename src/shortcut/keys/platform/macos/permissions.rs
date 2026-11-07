#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Check if the current process has accessibility permissions
pub fn check_accessibility() -> bool {
    unsafe { AXIsProcessTrusted() }
}
