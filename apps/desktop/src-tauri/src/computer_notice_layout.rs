//! Caption placement is independent of the light at the physical screen edge.
#[cfg(any(test, target_os = "macos"))]
fn below_camera(safe_top: f64) -> f64 {
    if safe_top.is_finite() && safe_top > 0.0 {
        safe_top.ceil() + 8.0
    } else {
        0.0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn caption_top(_: &tauri::Monitor) -> f64 {
    0.0
}

#[cfg(target_os = "macos")]
pub fn caption_top(monitor: &tauri::Monitor) -> f64 {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSScreen;
    let scale = monitor.scale_factor();
    let origin = monitor.position().to_logical::<f64>(scale);
    // A safe fallback if native enumeration cannot identify the monitor.
    let work_inset = f64::from(monitor.work_area().position.y - monitor.position().y) / scale;
    let fallback = work_inset.max(48.0);
    let Some(main_thread) = MainThreadMarker::new() else {
        return below_camera(fallback);
    };
    let screens = NSScreen::screens(main_thread);
    // Cocoa uses bottom-left coordinates; Tauri/CoreGraphics uses top-left.
    let Some(primary) = screens.firstObject() else {
        return below_camera(fallback);
    };
    let primary_frame = primary.frame();
    let primary_top = primary_frame.origin.y + primary_frame.size.height;
    let screen = screens.iter().find(|screen| {
        let frame = screen.frame();
        (frame.origin.x - origin.x).abs() < 1.0
            && (primary_top - frame.origin.y - frame.size.height - origin.y).abs() < 1.0
    });
    let inset = screen.map_or_else(
        || {
            screens
                .iter()
                .map(|screen| screen.safeAreaInsets().top)
                .fold(fallback, f64::max)
        },
        |screen| screen.safeAreaInsets().top,
    );
    below_camera(inset)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captions_clear_camera_housings_and_normal_displays_stay_flush() {
        for (inset, expected) in [
            (0.0, 0.0),
            (32.0, 40.0),
            (37.0, 45.0),
            (37.5, 46.0),
            (f64::NAN, 0.0),
        ] {
            assert!((below_camera(inset) - expected).abs() < f64::EPSILON);
        }
    }
}
