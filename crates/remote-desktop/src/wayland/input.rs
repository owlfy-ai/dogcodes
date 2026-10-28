//! Plan and validate an entire input batch before the first OS effect.
use crate::{Input, Result};
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Event {
    Move { stream: u32, x: f64, y: f64 },
    Button(i32, u32),
    Key(i32, u32),
    Scroll(u32, i32),
}
pub(super) fn plan(stream: u32, size: (u32, u32), events: Vec<Input>) -> Result<Vec<Event>> {
    let mut out = Vec::new();
    let state = |direction: &str| match direction {
        "press" => Ok(&[1][..]),
        "release" => Ok(&[0][..]),
        "click" => Ok(&[1, 0][..]),
        _ => Err("输入方向无效".to_string()),
    };
    for event in events {
        match event {
            Input::Move { x, y } => out.push(Event::Move {
                stream,
                x: f64::from(x) * f64::from(size.0.saturating_sub(1)) / 65535.,
                y: f64::from(y) * f64::from(size.1.saturating_sub(1)) / 65535.,
            }),
            Input::Button { button, direction } => {
                let code = match button.as_str() {
                    "left" => 0x110,
                    "right" => 0x111,
                    "middle" => 0x112,
                    _ => return Err("鼠标按钮无效".into()),
                };
                for s in state(&direction)? {
                    out.push(Event::Button(code, *s));
                }
            }
            Input::Key { key, direction } => {
                let code = keysym(&key)?;
                for s in state(&direction)? {
                    out.push(Event::Key(code, *s));
                }
            }
            Input::Text { text } => {
                if text.len() > 64 * 1024 {
                    return Err("键入文字过长".into());
                }
                for c in text.chars() {
                    let code = unicode(c);
                    out.push(Event::Key(code, 1));
                    out.push(Event::Key(code, 0));
                }
            }
            Input::Scroll { x, y } => {
                if x != 0 {
                    out.push(Event::Scroll(1, i32::from(x)));
                }
                if y != 0 {
                    out.push(Event::Scroll(0, i32::from(y)));
                }
            }
        }
        if out.len() > 32768 {
            return Err("单次键鼠操作过多".into());
        }
    }
    Ok(out)
}
pub(super) fn unicode(c: char) -> i32 {
    match c {
        '\n' | '\r' => return 0xff0d,
        '\t' => return 0xff09,
        '\u{8}' => return 0xff08,
        _ => {}
    }
    let n = u32::from(c);
    (if n <= 255 { n } else { 0x01000000 | n }) as i32
}
pub(super) fn keysym(key: &str) -> Result<i32> {
    let value = match key {
        "ctrl" => 0xffe3,
        "alt" => 0xffe9,
        "shift" => 0xffe1,
        "meta" => 0xffeb,
        "enter" => 0xff0d,
        "tab" => 0xff09,
        "escape" => 0xff1b,
        "backspace" => 0xff08,
        "delete" => 0xffff,
        "left" => 0xff51,
        "up" => 0xff52,
        "right" => 0xff53,
        "down" => 0xff54,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageUp" => 0xff55,
        "pageDown" => 0xff56,
        "space" => 32,
        "capsLock" => 0xffe5,
        _ => {
            if let Some(n) = key
                .strip_prefix('f')
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|n| (1..=12).contains(n))
            {
                0xffbd + n
            } else {
                let mut chars = key.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => unicode(c),
                    _ => return Err("不支持的按键".into()),
                }
            }
        }
    };
    Ok(value)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_newlines_tabs_and_non_latin_symbols_are_distinct_from_shortcuts() {
        assert_eq!(
            plan(
                7,
                (800, 600),
                vec![Input::Text {
                    text: "a\n\t中".into()
                }]
            )
            .unwrap(),
            vec![
                Event::Key(97, 1),
                Event::Key(97, 0),
                Event::Key(0xff0d, 1),
                Event::Key(0xff0d, 0),
                Event::Key(0xff09, 1),
                Event::Key(0xff09, 0),
                Event::Key(0x01004e2d, 1),
                Event::Key(0x01004e2d, 0)
            ]
        );
    }
    #[test]
    fn coordinates_are_stream_local_logical_not_physical_or_global() {
        assert_eq!(
            plan(77, (1728, 1117), vec![Input::Move { x: 65535, y: 32768 }]).unwrap(),
            vec![Event::Move {
                stream: 77,
                x: 1727.,
                y: 32768. * 1116. / 65535.
            }]
        );
    }
    #[test]
    fn unicode_shortcuts_and_scroll_use_portal_symbols_and_evdev_buttons() {
        let events = vec![
            Input::Text {
                text: "中🟢".into(),
            },
            Input::Key {
                key: "ctrl".into(),
                direction: "press".into(),
            },
            Input::Button {
                button: "right".into(),
                direction: "click".into(),
            },
            Input::Scroll { x: -2, y: 3 },
        ];
        assert_eq!(
            plan(7, (800, 600), events).unwrap(),
            vec![
                Event::Key(0x01004e2d, 1),
                Event::Key(0x01004e2d, 0),
                Event::Key(0x0101f7e2, 1),
                Event::Key(0x0101f7e2, 0),
                Event::Key(0xffe3, 1),
                Event::Button(0x111, 1),
                Event::Button(0x111, 0),
                Event::Scroll(1, -2),
                Event::Scroll(0, 3)
            ]
        );
        assert!(
            plan(
                7,
                (800, 600),
                vec![
                    Input::Move { x: 1, y: 1 },
                    Input::Key {
                        key: "unknown".into(),
                        direction: "click".into()
                    }
                ]
            )
            .is_err()
        );
    }
}
