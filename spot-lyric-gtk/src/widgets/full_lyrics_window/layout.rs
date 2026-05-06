use crate::platform::MonitorGeometry;

pub const LOCKED_WINDOW_GAP: i32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub width: i32,
    pub height: i32,
}

pub fn active_lyric_index(line_start_times: &[i64], position_ms: i64) -> Option<usize> {
    line_start_times
        .iter()
        .rposition(|start_time| *start_time <= position_ms)
}

pub fn locked_child_size(parent: WindowRect) -> WindowSize {
    WindowSize {
        width: parent.width.max(1),
        height: parent.height.max(1),
    }
}

pub fn side_by_side_window_position(
    parent: WindowRect,
    child: WindowSize,
    monitor: MonitorGeometry,
    gap: i32,
) -> (i32, i32) {
    let gap = gap.max(0);
    let min_x = monitor.x + gap;
    let max_x = (monitor.x + monitor.width - child.width - gap).max(min_x);
    let right_x = parent.x + parent.width + gap;
    let left_x = parent.x - child.width - gap;
    let x = if right_x <= max_x {
        right_x
    } else {
        left_x.clamp(min_x, max_x)
    };
    (x, clamped_y(parent.y, child.height, monitor, gap))
}

fn clamped_y(parent_y: i32, child_height: i32, monitor: MonitorGeometry, gap: i32) -> i32 {
    let min_y = monitor.y + gap;
    let max_y = (monitor.y + monitor.height - child_height - gap).max(min_y);
    parent_y.clamp(min_y, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_index_tracks_last_started_line() {
        let starts = [1_000, 2_500, 4_000];

        assert_eq!(active_lyric_index(&starts, 999), None);
        assert_eq!(active_lyric_index(&starts, 1_000), Some(0));
        assert_eq!(active_lyric_index(&starts, 3_000), Some(1));
        assert_eq!(active_lyric_index(&starts, 9_000), Some(2));
    }

    #[test]
    fn locked_child_size_matches_parent() {
        let parent = WindowRect {
            x: 120,
            y: 80,
            width: 620,
            height: 720,
        };

        assert_eq!(
            locked_child_size(parent),
            WindowSize {
                width: 620,
                height: 720,
            }
        );
    }

    #[test]
    fn side_by_side_position_prefers_right_of_parent() {
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
        };
        let parent = WindowRect {
            x: 120,
            y: 80,
            width: 620,
            height: 720,
        };
        let child = WindowSize {
            width: 420,
            height: 720,
        };

        assert_eq!(
            side_by_side_window_position(parent, child, monitor, 16),
            (756, 80)
        );
    }

    #[test]
    fn locked_side_by_side_position_has_no_gap() {
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 1_920,
            height: 1_080,
        };
        let parent = WindowRect {
            x: 120,
            y: 80,
            width: 620,
            height: 720,
        };
        let child = locked_child_size(parent);

        assert_eq!(
            side_by_side_window_position(parent, child, monitor, LOCKED_WINDOW_GAP),
            (740, 80)
        );
    }

    #[test]
    fn side_by_side_position_falls_back_left_and_clamps_y() {
        let monitor = MonitorGeometry {
            x: 0,
            y: 0,
            width: 1_200,
            height: 900,
        };
        let parent = WindowRect {
            x: 700,
            y: 760,
            width: 480,
            height: 720,
        };
        let child = WindowSize {
            width: 360,
            height: 500,
        };

        assert_eq!(
            side_by_side_window_position(parent, child, monitor, 12),
            (328, 388)
        );
    }
}
