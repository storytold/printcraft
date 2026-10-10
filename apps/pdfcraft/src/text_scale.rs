//! The desktop's text scaling, for Preferences ▸ Interface size ▸ Auto.
//!
//! On Linux and the BSDs, GNOME (and desktops sharing its settings schema) keep it in
//! `org.gnome.desktop.interface text-scaling-factor` (Settings ▸ Accessibility ▸ Large Text sets
//! 1.25). It is read once at launch through `gsettings`, which is absent or fails elsewhere, and
//! anything but a sensible number counts as no scaling. Other systems scale the whole display
//! instead, which already reaches egui as `pixels_per_point`.

/// How long `gsettings` may take before launch goes on without it.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

/// The text scaling factor, or 1.0.
pub fn system() -> f32 {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
    if let Some(output) = gsettings(&["get", "org.gnome.desktop.interface", "text-scaling-factor"]) {
        return parse(&output);
    }
    1.0
}

/// `gsettings` output (`1.25`) as a factor; 1.0 when it isn't a usable one.
#[cfg_attr(not(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")), allow(dead_code))]
fn parse(output: &str) -> f32 {
    output.trim().parse::<f64>().ok().and_then(pdfcraft_ui_egui::ui_scale::valid).unwrap_or(1.0)
}

/// Run `gsettings` and return what it printed, or `None` if it is missing, fails or hangs.
#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
fn gsettings(args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new("gsettings").args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    // A number fits in far less; a misbehaving `gsettings` can't make launch read without end.
    child.stdout.take()?.take(256).read_to_string(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn gsettings_output_parses_or_means_no_scaling() {
        assert_eq!(super::parse("1.25\n"), 1.25);
        assert_eq!(super::parse("1.0"), 1.0);
        for bad in ["", "nan", "inf", "-2", "0", "uint32 0", "100"] {
            assert_eq!(super::parse(bad), 1.0, "{bad}");
        }
    }

    #[test]
    fn reading_the_system_scale_never_fails() {
        let f = super::system();
        assert!(f.is_finite() && (pdfcraft_ui_egui::ui_scale::MIN..=pdfcraft_ui_egui::ui_scale::MAX).contains(&f), "{f}");
    }
}
