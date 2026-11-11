use std::process::Command;

pub fn is_clamshell() -> Result<bool, String> {
    let output = Command::new("ioreg")
        .args(["-r", "-k", "AppleClamshellState", "-d", "4"])
        .output()
        .map_err(|e| format!("Failed to execute ioreg: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "ioreg command failed with status: {}",
            output.status
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    Ok(stdout.contains("\"AppleClamshellState\" = Yes"))
}

pub fn is_laptop() -> Result<bool, String> {
    let output = Command::new("pmset")
        .arg("-g")
        .arg("batt")
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    Ok(stdout.contains("InternalBattery"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Smoke tests: a typo in the command or a bad `ioreg` argument makes
    // these fail, and clamshell microphone selection would fall back silently.
    #[test]
    fn ioreg_runs_with_our_arguments() {
        assert!(is_clamshell().is_ok());
    }

    #[test]
    fn pmset_runs_with_our_arguments() {
        assert!(is_laptop().is_ok());
    }
}
