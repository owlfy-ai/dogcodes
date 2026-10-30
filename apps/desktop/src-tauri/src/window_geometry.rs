use tauri::{LogicalPosition, LogicalSize, Manager};

const SCREEN_MARGIN: f64 = 16.0;

fn fitted_sizes(
    desired: LogicalSize<f64>,
    minimum: LogicalSize<f64>,
    frame: LogicalSize<f64>,
    work_area: LogicalSize<f64>,
) -> (LogicalSize<f64>, LogicalSize<f64>) {
    let available = LogicalSize::new(
        (work_area.width - frame.width - 2.0 * SCREEN_MARGIN).max(1.0),
        (work_area.height - frame.height - 2.0 * SCREEN_MARGIN).max(1.0),
    );
    let minimum = LogicalSize::new(
        minimum.width.min(available.width),
        minimum.height.min(available.height),
    );
    let size = LogicalSize::new(
        desired.width.clamp(minimum.width, available.width),
        desired.height.clamp(minimum.height, available.height),
    );
    (size, minimum)
}

/// Fit the hidden main window before showing it. Work area excludes the taskbar;
/// measured non-client bounds include Windows title bars and resize borders.
pub fn fit_to_work_area(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };
    let scale = window.scale_factor()?;
    let inner = window.inner_size()?;
    let outer = window.outer_size()?;
    let frame = LogicalSize::new(
        f64::from(outer.width.saturating_sub(inner.width)) / scale,
        f64::from(outer.height.saturating_sub(inner.height)) / scale,
    );
    let minimum = window
        .app_handle()
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == window.label())
        .map_or(LogicalSize::new(0.0, 0.0), |config| {
            LogicalSize::new(
                config.min_width.unwrap_or(0.0),
                config.min_height.unwrap_or(0.0),
            )
        });
    let work_area = monitor.work_area();
    let work_size = work_area.size.to_logical::<f64>(scale);
    let origin = work_area.position.to_logical::<f64>(scale);
    let (size, minimum) = fitted_sizes(inner.to_logical::<f64>(scale), minimum, frame, work_size);
    // Lower the minimum first, otherwise Windows can reject the fitted size.
    window.set_min_size(Some(minimum))?;
    window.set_size(size)?;
    window.set_position(LogicalPosition::new(
        origin.x + (work_size.width - size.width - frame.width) / 2.0,
        origin.y + (work_size.height - size.height - frame.height) / 2.0,
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_laptop_work_areas_at_different_windows_scales() {
        for (width, height, taskbar, scale) in [
            (1366.0, 768.0, 40.0, 1.0),
            (1920.0, 1080.0, 60.0, 1.5),
            (2760.0, 1720.0, 80.0, 2.0),
            (1366.0, 768.0, 60.0, 1.5),
            (1920.0, 1080.0, 96.0, 2.0),
        ] {
            let work = LogicalSize::new(width / scale, (height - taskbar) / scale);
            let frame = LogicalSize::new(16.0, 40.0);
            let (size, minimum) = fitted_sizes(
                LogicalSize::new(1380.0, 860.0),
                LogicalSize::new(900.0, 480.0),
                frame,
                work,
            );
            assert!(size.width + frame.width + 2.0 * SCREEN_MARGIN <= work.width);
            assert!(size.height + frame.height + 2.0 * SCREEN_MARGIN <= work.height);
            assert!(minimum.width <= size.width && minimum.height <= size.height);
            assert!(minimum.width <= 900.0 && minimum.height <= 480.0);
        }
    }

    #[test]
    fn keeps_preferred_size_on_a_large_display() {
        let desired = LogicalSize::new(1380.0, 860.0);
        let minimum = LogicalSize::new(900.0, 480.0);
        assert_eq!(
            fitted_sizes(
                desired,
                minimum,
                LogicalSize::new(16.0, 40.0),
                LogicalSize::new(2560.0, 1400.0),
            ),
            (desired, minimum)
        );
    }
}
