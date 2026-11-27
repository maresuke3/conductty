//! マウスの操作を、アプリへ送るバイト列にする（xterm / Ghostty と同じ形）

use crate::grid::MouseTracking;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    /// ボタンを押さずに動かしたとき
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub action: MouseAction,
    pub button: MouseButton,
    /// 0 始まりのセルの位置
    pub col: usize,
    pub row: usize,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// モードで送らないことになっている操作なら None
pub fn encode_mouse(tracking: MouseTracking, sgr: bool, ev: &MouseEvent) -> Option<Vec<u8>> {
    let is_wheel = matches!(
        ev.button,
        MouseButton::WheelUp
            | MouseButton::WheelDown
            | MouseButton::WheelLeft
            | MouseButton::WheelRight
    );
    let report = match (tracking, ev.action) {
        (MouseTracking::Off, _) => false,
        (MouseTracking::X10, action) => action == MouseAction::Press && !is_wheel,
        (_, MouseAction::Press) => true,
        // ホイールには離したときの報告が無い
        (_, MouseAction::Release) => !is_wheel,
        (MouseTracking::Normal, MouseAction::Motion) => false,
        (MouseTracking::ButtonEvent, MouseAction::Motion) => ev.button != MouseButton::None,
        (MouseTracking::AnyEvent, MouseAction::Motion) => true,
    };
    if !report {
        return None;
    }

    let mut code: u32 = match ev.button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    // SGR 以外の形では、離したときはどのボタンかを送れず 3 になる
    if ev.action == MouseAction::Release && !sgr {
        code = 3;
    }
    if tracking != MouseTracking::X10 {
        code += 4 * ev.shift as u32 + 8 * ev.alt as u32 + 16 * ev.ctrl as u32;
    }
    if ev.action == MouseAction::Motion {
        code += 32;
    }

    let (x, y) = (ev.col + 1, ev.row + 1);
    if sgr {
        let end = if ev.action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    // 昔の形は 1 バイトに 32 を足して入れるので、223 列・行を超える位置は送れない
    let byte = |v: usize| u8::try_from(v + 32).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        byte(code as usize)?,
        byte(x)?,
        byte(y)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(action: MouseAction, button: MouseButton, col: usize, row: usize) -> MouseEvent {
        MouseEvent {
            action,
            button,
            col,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn encodes_sgr_press_release_motion_and_wheel() {
        use MouseAction::*;
        use MouseButton::*;
        let sgr = |tracking, e: MouseEvent| {
            encode_mouse(tracking, true, &e).map(|b| String::from_utf8(b).unwrap())
        };
        let any = MouseTracking::AnyEvent;
        assert_eq!(
            sgr(any, ev(Press, Left, 0, 0)).as_deref(),
            Some("\x1b[<0;1;1M")
        );
        assert_eq!(
            sgr(any, ev(Release, Right, 9, 4)).as_deref(),
            Some("\x1b[<2;10;5m")
        );
        assert_eq!(
            sgr(any, ev(Motion, Left, 2, 3)).as_deref(),
            Some("\x1b[<32;3;4M")
        );
        assert_eq!(
            sgr(any, ev(Motion, None, 2, 3)).as_deref(),
            Some("\x1b[<35;3;4M")
        );
        assert_eq!(
            sgr(any, ev(Press, WheelUp, 0, 0)).as_deref(),
            Some("\x1b[<64;1;1M")
        );
        assert_eq!(sgr(any, ev(Release, WheelUp, 0, 0)), Option::None);
        let ctrl_shift = MouseEvent {
            shift: true,
            ctrl: true,
            ..ev(Press, Middle, 0, 0)
        };
        assert_eq!(sgr(any, ctrl_shift).as_deref(), Some("\x1b[<21;1;1M"));
    }

    #[test]
    fn tracking_mode_decides_what_is_reported() {
        use MouseAction::*;
        use MouseButton::*;
        let sent = |tracking, e: MouseEvent| encode_mouse(tracking, true, &e).is_some();
        assert!(!sent(MouseTracking::Off, ev(Press, Left, 0, 0)));
        assert!(sent(MouseTracking::X10, ev(Press, Left, 0, 0)));
        assert!(!sent(MouseTracking::X10, ev(Release, Left, 0, 0)));
        assert!(!sent(MouseTracking::X10, ev(Press, WheelUp, 0, 0)));
        assert!(sent(MouseTracking::Normal, ev(Release, Left, 0, 0)));
        assert!(!sent(MouseTracking::Normal, ev(Motion, Left, 0, 0)));
        assert!(sent(MouseTracking::ButtonEvent, ev(Motion, Left, 0, 0)));
        assert!(!sent(MouseTracking::ButtonEvent, ev(Motion, None, 0, 0)));
        assert!(sent(MouseTracking::AnyEvent, ev(Motion, None, 0, 0)));
    }

    #[test]
    fn encodes_legacy_x10_bytes_and_drops_far_positions() {
        use MouseAction::*;
        use MouseButton::*;
        let normal = MouseTracking::Normal;
        assert_eq!(
            encode_mouse(normal, false, &ev(Press, Left, 0, 0)),
            Some(b"\x1b[M\x20\x21\x21".to_vec())
        );
        // 離したときは 3
        assert_eq!(
            encode_mouse(normal, false, &ev(Release, Right, 1, 1)),
            Some(b"\x1b[M\x23\x22\x22".to_vec())
        );
        assert_eq!(
            encode_mouse(normal, false, &ev(Press, Left, 223, 0)),
            Option::None
        );
    }
}
