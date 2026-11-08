#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

pub fn check_accessibility() -> bool {
    unsafe { AXIsProcessTrusted() }
}
