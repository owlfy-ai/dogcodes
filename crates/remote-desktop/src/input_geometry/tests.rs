use super::*;
use crate::to_global;
fn display() -> Display {
    Display {
        id: 7,
        name: "original".into(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 2.0,
        primary: true,
    }
}
fn point(x: u16, y: u16) -> Input {
    Input::Move { x, y }
}
#[test]
fn a_batch_uses_one_fresh_moved_rotated_scaled_snapshot_before_any_injection() {
    let stale = display();
    let mut queries = 0;
    let mut mapped = vec![];
    batch(
        &stale,
        vec![point(0, 0), point(u16::MAX, u16::MAX)],
        |d| {
            queries += 1;
            Bounds {
                x: -1080,
                y: 120,
                width: 1080,
                height: 1920,
            }
            .apply(d)
        },
        |d, e| {
            if let Input::Move { x, y } = e {
                mapped.push(to_global(d, x, y));
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(queries, 1);
    assert_eq!(mapped, vec![(-1080, 120), (-1, 2039)]);
}
#[test]
fn native_input_units_do_not_depend_on_encoded_resolution_or_old_scale() {
    let base = display();
    let physical = Bounds {
        x: 3840,
        y: 0,
        width: 3840,
        height: 2160,
    }
    .apply(&base)
    .unwrap();
    assert_eq!(to_global(&physical, u16::MAX, u16::MAX), (7679, 2159));
    let logical = Bounds {
        x: -1728,
        y: 0,
        width: 1728,
        height: 1117,
    }
    .apply(&base)
    .unwrap();
    assert_eq!(to_global(&logical, u16::MAX, u16::MAX), (-1, 1116));
}
#[test]
fn unavailable_or_removed_display_never_falls_back_to_capture_bounds_or_partly_injects() {
    let mut sent = 0;
    let result = batch(
        &display(),
        vec![
            Input::Text {
                text: "before pointer".into(),
            },
            point(1, 2),
        ],
        |_| Err("removed".into()),
        |_, _| {
            sent += 1;
            Ok(())
        },
    );
    assert_eq!(result.unwrap_err(), "removed");
    assert_eq!(sent, 0);
}
#[test]
fn releases_and_keyboard_remain_available_without_querying_a_removed_display() {
    let events = vec![
        Input::Key {
            key: "shift".into(),
            direction: "release".into(),
        },
        Input::Button {
            button: "left".into(),
            direction: "release".into(),
        },
        Input::Text {
            text: "text".into(),
        },
    ];
    let mut sent = 0;
    batch(
        &display(),
        events,
        |_| panic!("non-positional input queried a removed screen"),
        |_, _| {
            sent += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(sent, 3);
}
#[test]
fn degenerate_or_overflowing_bounds_are_rejected() {
    for b in [
        Bounds {
            x: 0,
            y: 0,
            width: 0,
            height: 1,
        },
        Bounds {
            x: 0,
            y: 0,
            width: 1,
            height: 0,
        },
        Bounds {
            x: i32::MAX,
            y: 0,
            width: 2,
            height: 1,
        },
    ] {
        assert!(b.apply(&display()).is_err());
    }
}
#[test]
fn a_geometry_change_between_move_and_button_preserves_the_original_normalized_point() {
    let old = display();
    let mut pointer = Pointer::default();
    pointer.remember(&old, u16::MAX, u16::MAX);
    assert_eq!(pointer.changed(&old), None);
    let new = Bounds {
        x: -1080,
        y: 0,
        width: 1080,
        height: 1920,
    }
    .apply(&old)
    .unwrap();
    let (x, y) = pointer.changed(&new).unwrap();
    assert_eq!(to_global(&new, x, y), (-1, 1919));
    pointer.remember(&new, x, y);
    assert_eq!(pointer.changed(&new), None);
    let other = Display { id: 8, ..new };
    assert_eq!(
        pointer.changed(&other),
        None,
        "must not move a different selected screen"
    );
}
#[test]
fn native_injection_errors_stop_the_remaining_ordered_batch() {
    let mut sent = 0;
    let result = batch(
        &display(),
        vec![point(0, 0), point(1, 1)],
        |d| Ok(d.clone()),
        |_, _| {
            sent += 1;
            Err("uncertain injection".into())
        },
    );
    assert!(result.is_err());
    assert_eq!(sent, 1);
}

#[test]
fn another_displays_snapshot_cannot_be_used_for_this_input_batch() {
    let result = batch(
        &display(),
        vec![point(0, 0)],
        |d| Ok(Display { id: 8, ..d.clone() }),
        |_, _| panic!("different display input escaped"),
    );
    assert!(result.unwrap_err().contains("显示器身份"));
}

#[test]
fn a_late_geometry_answer_cannot_inject_after_the_original_owner_has_retired() {
    use std::cell::Cell;
    let alive = Cell::new(true);
    let mut sent = 0;
    let result = batch_guarded(
        &display(),
        vec![point(1, 2)],
        |d| {
            alive.set(false);
            Ok(d.clone())
        },
        || alive.get(),
        |_, _| {
            sent += 1;
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(sent, 0);
}
#[test]
fn owner_retirement_during_a_batch_stops_subsequent_actions() {
    use std::cell::Cell;
    let alive = Cell::new(true);
    let mut sent = 0;
    let result = batch_guarded(
        &display(),
        vec![point(1, 2), point(3, 4)],
        |d| Ok(d.clone()),
        || alive.get(),
        |_, _| {
            sent += 1;
            alive.set(false);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert_eq!(sent, 1);
}
