//! Minimal X11 helpers used by the desktop lyrics overlay.
//!
//! Responsibilities:
//!   - Mark a window as a desktop overlay: `_NET_WM_STATE_ABOVE`,
//!     `_NET_WM_STATE_SKIP_TASKBAR`, `_NET_WM_STATE_SKIP_PAGER`,
//!     `_NET_WM_STATE_STICKY`, `_NET_WM_WINDOW_TYPE_UTILITY`,
//!     `_NET_WM_DESKTOP = 0xFFFFFFFF` (all desktops), `WM_CLASS`, `_NET_WM_PID`.
//!   - Toggle click-through via the SHAPE extension's input region.
//!   - Move the window to absolute screen coordinates.
//!
//! GTK4 dropped `gtk_window_set_keep_above`, so on X11 we must do this
//! ourselves via raw EWMH messages.

use anyhow::{anyhow, Result};
use std::sync::Arc;
use x11rb::connection::Connection as _;
use x11rb::connection::RequestConnection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _, SK};
use x11rb::protocol::xproto::{
    self, AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, EventMask,
    PropMode, Rectangle,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const NET_WM_DESKTOP_ALL: u32 = 0xFFFF_FFFF;

/// Cached atom set so we resolve names once per process.
struct Atoms {
    net_wm_state: u32,
    net_wm_state_above: u32,
    net_wm_state_skip_taskbar: u32,
    net_wm_state_skip_pager: u32,
    net_wm_state_sticky: u32,
    net_wm_window_type: u32,
    net_wm_window_type_utility: u32,
    net_wm_desktop: u32,
    net_moveresize_window: u32,
    net_wm_pid: u32,
    wm_class: u32,
    utf8_string: u32,
    cardinal: u32,
    atom: u32,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self> {
        fn one(conn: &RustConnection, name: &str) -> Result<u32> {
            let cookie = conn.intern_atom(false, name.as_bytes())?;
            Ok(cookie.reply()?.atom)
        }
        Ok(Self {
            net_wm_state: one(conn, "_NET_WM_STATE")?,
            net_wm_state_above: one(conn, "_NET_WM_STATE_ABOVE")?,
            net_wm_state_skip_taskbar: one(conn, "_NET_WM_STATE_SKIP_TASKBAR")?,
            net_wm_state_skip_pager: one(conn, "_NET_WM_STATE_SKIP_PAGER")?,
            net_wm_state_sticky: one(conn, "_NET_WM_STATE_STICKY")?,
            net_wm_window_type: one(conn, "_NET_WM_WINDOW_TYPE")?,
            net_wm_window_type_utility: one(conn, "_NET_WM_WINDOW_TYPE_UTILITY")?,
            net_wm_desktop: one(conn, "_NET_WM_DESKTOP")?,
            net_moveresize_window: one(conn, "_NET_MOVERESIZE_WINDOW")?,
            net_wm_pid: one(conn, "_NET_WM_PID")?,
            wm_class: u32::from(AtomEnum::WM_CLASS),
            utf8_string: one(conn, "UTF8_STRING")?,
            cardinal: u32::from(AtomEnum::CARDINAL),
            atom: u32::from(AtomEnum::ATOM),
        })
    }
}

#[derive(Clone)]
pub struct X11Helper {
    conn: Arc<RustConnection>,
    root: u32,
    atoms: Arc<Atoms>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl X11Helper {
    /// Connect to the running X server (via `$DISPLAY`).
    pub fn connect() -> Result<Self> {
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|error| anyhow!("X11 connect failed: {error}"))?;
        let root = conn.setup().roots[screen_num].root;
        let atoms = Atoms::intern(&conn)?;

        // Verify SHAPE extension presence so set_input_passthrough cannot
        // fail silently later.
        let shape_present = conn
            .extension_information(shape::X11_EXTENSION_NAME)?
            .is_some();
        if !shape_present {
            tracing::warn!("X11 SHAPE extension is missing — click-through will not work");
        }

        Ok(Self {
            conn: Arc::new(conn),
            root,
            atoms: Arc::new(atoms),
        })
    }

    /// Mark the window as an always-on-top desktop overlay across all desktops
    /// and skip taskbar/pager. Idempotent — safe to call after every realize.
    pub fn make_overlay(&self, xid: u32, app_id: &str, instance: &str) -> Result<()> {
        let atoms = &self.atoms;
        let conn = &*self.conn;

        // _NET_WM_WINDOW_TYPE = _NET_WM_WINDOW_TYPE_UTILITY
        conn.change_property32(
            PropMode::REPLACE,
            xid,
            atoms.net_wm_window_type,
            atoms.atom,
            &[atoms.net_wm_window_type_utility],
        )?;

        // _NET_WM_STATE = ABOVE | SKIP_TASKBAR | SKIP_PAGER | STICKY
        conn.change_property32(
            PropMode::REPLACE,
            xid,
            atoms.net_wm_state,
            atoms.atom,
            &[
                atoms.net_wm_state_above,
                atoms.net_wm_state_skip_taskbar,
                atoms.net_wm_state_skip_pager,
                atoms.net_wm_state_sticky,
            ],
        )?;

        // EWMH spec: also send a ClientMessage so already-mapped windows
        // get the WM to honor the new state.
        for atom in [
            atoms.net_wm_state_above,
            atoms.net_wm_state_skip_taskbar,
            atoms.net_wm_state_skip_pager,
            atoms.net_wm_state_sticky,
        ] {
            self.send_state_change(xid, atom)?;
        }

        // _NET_WM_DESKTOP = 0xFFFFFFFF (show on all desktops)
        conn.change_property32(
            PropMode::REPLACE,
            xid,
            atoms.net_wm_desktop,
            atoms.cardinal,
            &[NET_WM_DESKTOP_ALL],
        )?;

        // _NET_WM_PID
        let pid = std::process::id();
        conn.change_property32(
            PropMode::REPLACE,
            xid,
            atoms.net_wm_pid,
            atoms.cardinal,
            &[pid],
        )?;

        // WM_CLASS = "instance\0class\0" (STRING list)
        let mut wm_class = Vec::new();
        wm_class.extend_from_slice(instance.as_bytes());
        wm_class.push(0);
        wm_class.extend_from_slice(app_id.as_bytes());
        wm_class.push(0);
        conn.change_property8(
            xproto::PropMode::REPLACE,
            xid,
            atoms.wm_class,
            u32::from(xproto::AtomEnum::STRING),
            &wm_class,
        )?;

        conn.flush()?;
        Ok(())
    }

    /// Toggle click-through. `passthrough = true` empties the input region,
    /// which causes the X server to forward all pointer events to the window
    /// behind ours; `false` restores normal hit testing.
    pub fn set_input_passthrough(&self, xid: u32, passthrough: bool) -> Result<()> {
        let conn = &*self.conn;
        let outer = self.outer_window(xid).unwrap_or(xid);

        self.set_input_passthrough_for_window(xid, passthrough)?;
        if outer != xid {
            // Cinnamon/Muffin reparents managed windows.  Restoring the input
            // shape only on the GTK client window can leave the WM frame with
            // an empty input region, so apply the same state to the outermost
            // frame window as well.
            self.set_input_passthrough_for_window(outer, passthrough)?;
        }
        conn.flush()?;
        Ok(())
    }

    fn set_input_passthrough_for_window(&self, xid: u32, passthrough: bool) -> Result<()> {
        let conn = &*self.conn;
        if passthrough {
            // Empty rectangle list → input region is empty → window cannot
            // receive any pointer event.
            conn.shape_rectangles(
                shape::SO::SET,
                SK::INPUT,
                xproto::ClipOrdering::UNSORTED,
                xid,
                0,
                0,
                &[],
            )?;
        } else {
            // Some compositors/window managers do not reliably restore the
            // previous input region from a NONE mask after it was emptied.
            // Set an explicit full-window input rectangle so the overlay
            // receives pointer events again as soon as the user unlocks it.
            let geometry = conn.get_geometry(xid)?.reply()?;
            let rect = Rectangle {
                x: 0,
                y: 0,
                width: geometry.width.max(1),
                height: geometry.height.max(1),
            };
            conn.shape_rectangles(
                shape::SO::SET,
                SK::INPUT,
                xproto::ClipOrdering::UNSORTED,
                xid,
                0,
                0,
                &[rect],
            )?;
        }
        Ok(())
    }

    /// Move window to absolute screen coordinates.
    pub fn move_window(&self, xid: u32, x: i32, y: i32) -> Result<()> {
        self.move_resize_window_inner(xid, x, y, None)
    }

    /// Move and resize a managed window to an absolute screen rectangle.
    pub fn move_resize_window(
        &self,
        xid: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Result<()> {
        let width = width.max(1) as u32;
        let height = height.max(1) as u32;
        self.move_resize_window_inner(xid, x, y, Some((width, height)))
    }

    fn move_resize_window_inner(
        &self,
        xid: u32,
        x: i32,
        y: i32,
        size: Option<(u32, u32)>,
    ) -> Result<()> {
        const STATIC_GRAVITY: u32 = 10;
        const HAS_X: u32 = 1 << 8;
        const HAS_Y: u32 = 1 << 9;
        const HAS_WIDTH: u32 = 1 << 10;
        const HAS_HEIGHT: u32 = 1 << 11;
        const SOURCE_APPLICATION: u32 = 1 << 12;

        let mut flags = STATIC_GRAVITY | HAS_X | HAS_Y | SOURCE_APPLICATION;
        let (width, height) = size.unwrap_or((0, 0));
        if size.is_some() {
            flags |= HAS_WIDTH | HAS_HEIGHT;
        }

        let event = ClientMessageEvent::new(
            32,
            xid,
            self.atoms.net_moveresize_window,
            [flags, x as u32, y as u32, width, height],
        );
        let conn = &*self.conn;
        conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
            event,
        )?;

        // Muffin can ignore direct ConfigureWindow requests sent to the GTK
        // client window after it has reparented the window into a WM frame.
        // Configure the outermost child of the root window as a synchronous
        // fallback so locked companion windows stay visually attached.
        let target = self.outer_window(xid).unwrap_or(xid);
        let mut aux = ConfigureWindowAux::new().x(x).y(y);
        if let Some((width, height)) = size {
            aux = aux.width(width).height(height);
        }
        conn.configure_window(target, &aux)?;
        conn.flush()?;
        Ok(())
    }

    /// Current top-left client-window position in root coordinates.
    pub fn window_position(&self, xid: u32) -> Result<(i32, i32)> {
        let geometry = self.window_geometry(xid)?;
        Ok((geometry.x, geometry.y))
    }

    /// Current outer-frame geometry in root coordinates.
    pub fn window_geometry(&self, xid: u32) -> Result<WindowGeometry> {
        let target = self.outer_window(xid).unwrap_or(xid);
        let geometry = self.conn.get_geometry(target)?.reply()?;
        let reply = self
            .conn
            .translate_coordinates(target, self.root, 0, 0)?
            .reply()?;
        if !reply.same_screen {
            return Err(anyhow!(
                "window is not on the same X11 screen as the root window"
            ));
        }
        Ok(WindowGeometry {
            x: reply.dst_x as i32,
            y: reply.dst_y as i32,
            width: geometry.width as i32,
            height: geometry.height as i32,
        })
    }

    /// Best-effort primary monitor geometry.
    /// Falls back to the root window dimensions when RandR is unavailable.
    pub fn primary_monitor_geometry(&self) -> Result<MonitorGeometry> {
        self.randr_primary_monitor_geometry()
            .or_else(|_| self.root_monitor_geometry())
    }

    /// Best-effort monitor geometry for a window.
    /// Falls back to the primary/root monitor when RandR cannot resolve one.
    pub fn monitor_geometry_for_window(&self, xid: u32) -> Result<MonitorGeometry> {
        let geometry = self.window_geometry(xid)?;
        self.randr_monitor_geometries()
            .ok()
            .and_then(|monitors| best_monitor_for_rect(&monitors, geometry))
            .or_else(|| self.primary_monitor_geometry().ok())
            .ok_or_else(|| anyhow!("no monitor geometry available"))
    }

    fn randr_primary_monitor_geometry(&self) -> Result<MonitorGeometry> {
        let monitors = self.randr_monitor_geometries()?;
        if monitors.is_empty() {
            return Err(anyhow!("RandR did not report an active monitor"));
        }
        Ok(monitors[0])
    }

    fn randr_monitor_geometries(&self) -> Result<Vec<MonitorGeometry>> {
        let conn = &*self.conn;
        let _ = conn.randr_query_version(1, 5)?.reply()?;
        let resources = conn
            .randr_get_screen_resources_current(self.root)?
            .reply()?;
        let primary = conn.randr_get_output_primary(self.root)?.reply()?.output;

        let mut outputs = Vec::new();
        if primary != x11rb::NONE {
            outputs.push(primary);
        }
        outputs.extend(
            resources
                .outputs
                .iter()
                .copied()
                .filter(|output| *output != primary),
        );

        let mut monitors = Vec::new();
        for output in outputs {
            let output_info = conn
                .randr_get_output_info(output, resources.config_timestamp)?
                .reply()?;
            if output_info.connection != randr::Connection::CONNECTED
                || output_info.crtc == x11rb::NONE
            {
                continue;
            }

            let crtc = conn
                .randr_get_crtc_info(output_info.crtc, resources.config_timestamp)?
                .reply()?;
            if crtc.width == 0 || crtc.height == 0 {
                continue;
            }

            monitors.push(MonitorGeometry {
                x: crtc.x as i32,
                y: crtc.y as i32,
                width: crtc.width as i32,
                height: crtc.height as i32,
            });
        }

        if monitors.is_empty() {
            Err(anyhow!("RandR did not report an active monitor"))
        } else {
            Ok(monitors)
        }
    }

    fn root_monitor_geometry(&self) -> Result<MonitorGeometry> {
        let geometry = self.conn.get_geometry(self.root)?.reply()?;
        Ok(MonitorGeometry {
            x: 0,
            y: 0,
            width: geometry.width as i32,
            height: geometry.height as i32,
        })
    }

    /// Return the outermost window that still belongs to this toplevel.
    ///
    /// Reparenting window managers such as Cinnamon's Muffin wrap the GTK
    /// client XID in a WM-owned frame window.  The frame is the child of the
    /// root window and is the window that actually moves on screen.
    fn outer_window(&self, xid: u32) -> Result<u32> {
        let conn = &*self.conn;
        let mut current = xid;

        for _ in 0..16 {
            let tree = conn.query_tree(current)?.reply()?;
            if tree.parent == x11rb::NONE || tree.parent == self.root {
                return Ok(current);
            }
            current = tree.parent;
        }

        Err(anyhow!("X11 window parent chain is unexpectedly deep"))
    }

    fn send_state_change(&self, xid: u32, state_atom: u32) -> Result<()> {
        const _NET_WM_STATE_ADD: u32 = 1;
        const SOURCE_APPLICATION: u32 = 1;

        let event = ClientMessageEvent::new(
            32,
            xid,
            self.atoms.net_wm_state,
            [_NET_WM_STATE_ADD, state_atom, 0, SOURCE_APPLICATION, 0],
        );

        let conn = &*self.conn;
        conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT,
            event,
        )?;
        Ok(())
    }
}

fn best_monitor_for_rect(
    monitors: &[MonitorGeometry],
    rect: WindowGeometry,
) -> Option<MonitorGeometry> {
    let rect_center = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    monitors.iter().copied().max_by_key(|monitor| {
        let contains_center = contains_point(*monitor, rect_center) as i64;
        (
            overlap_area(*monitor, rect),
            contains_center,
            monitor.width as i64 * monitor.height as i64,
        )
    })
}

fn contains_point(monitor: MonitorGeometry, point: (i32, i32)) -> bool {
    point.0 >= monitor.x
        && point.0 < monitor.x + monitor.width
        && point.1 >= monitor.y
        && point.1 < monitor.y + monitor.height
}

fn overlap_area(monitor: MonitorGeometry, rect: WindowGeometry) -> i64 {
    let left = monitor.x.max(rect.x);
    let top = monitor.y.max(rect.y);
    let right = (monitor.x + monitor.width).min(rect.x + rect.width);
    let bottom = (monitor.y + monitor.height).min(rect.y + rect.height);
    i64::from((right - left).max(0)) * i64::from((bottom - top).max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn best_monitor_for_rect_uses_window_screen() {
        let monitors = [
            MonitorGeometry {
                x: 0,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
            MonitorGeometry {
                x: 1_920,
                y: 0,
                width: 1_920,
                height: 1_080,
            },
        ];
        let rect = WindowGeometry {
            x: 1_950,
            y: 284,
            width: 742,
            height: 842,
        };

        assert_eq!(best_monitor_for_rect(&monitors, rect), Some(monitors[1]));
    }
}
