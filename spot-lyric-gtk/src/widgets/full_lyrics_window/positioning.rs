use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;

use crate::platform::X11Helper;

use super::layout::{
    locked_child_size, side_by_side_window_position, WindowRect, LOCKED_WINDOW_GAP,
};
use super::FullLyricsWindow;

const LOCK_SYNC_INTERVAL_MS: u64 = 16;

impl FullLyricsWindow {
    pub fn present_locked_beside(&self, parent: &impl IsA<gtk::Window>) {
        let parent = parent.as_ref().clone();
        self.set_transient_for(None::<&gtk::Window>);
        self.set_visible(true);
        self.present();
        self.sync_locked_position(&parent);
        self.start_parent_lock_timer(parent);
    }

    pub fn hide_locked(&self) {
        if let Some(id) = self.imp().lock_tick_id.take() {
            id.remove();
        }
        self.imp().last_locked_layout.set(None);
        self.set_visible(false);
    }

    fn start_parent_lock_timer(&self, parent: gtk::Window) {
        if let Some(id) = self.imp().lock_tick_id.take() {
            id.remove();
        }
        self.imp().last_locked_layout.set(None);
        let helper = X11Helper::connect().ok();
        let weak = self.downgrade();
        let id = glib::timeout_add_local(
            std::time::Duration::from_millis(LOCK_SYNC_INTERVAL_MS),
            move || {
                let Some(win) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if !win.is_visible() || !parent.is_visible() {
                    win.set_visible(false);
                    win.imp().lock_tick_id.set(None);
                    return glib::ControlFlow::Break;
                }
                win.sync_locked_position_with_helper(&parent, helper.as_ref());
                glib::ControlFlow::Continue
            },
        );
        self.imp().lock_tick_id.set(Some(id));
    }

    fn sync_locked_position(&self, parent: &gtk::Window) {
        let helper = X11Helper::connect().ok();
        self.sync_locked_position_with_helper(parent, helper.as_ref());
    }

    fn sync_locked_position_with_helper(&self, parent: &gtk::Window, helper: Option<&X11Helper>) {
        let Some(helper) = helper else { return };
        let layout = self.locked_layout(parent, helper);
        self.apply_locked_layout(helper, layout);
    }

    fn locked_layout(
        &self,
        parent: &gtk::Window,
        helper: &X11Helper,
    ) -> Option<(u32, i32, i32, i32, i32)> {
        let parent_xid = window_xid(parent)?;
        let own_xid = window_xid(self)?;
        let geometry = helper.window_geometry(parent_xid).ok()?;
        let monitor = helper.monitor_geometry_for_window(parent_xid).ok()?;
        let parent_rect = WindowRect {
            x: geometry.x,
            y: geometry.y,
            width: geometry.width,
            height: geometry.height,
        };
        let child = locked_child_size(parent_rect);
        let (x, y) = side_by_side_window_position(parent_rect, child, monitor, LOCKED_WINDOW_GAP);
        Some((own_xid, x, y, child.width.max(1), child.height.max(1)))
    }

    fn apply_locked_layout(&self, helper: &X11Helper, layout: Option<(u32, i32, i32, i32, i32)>) {
        let Some((own_xid, x, y, width, height)) = layout else {
            return;
        };
        let geometry = (x, y, width, height);
        if self.imp().last_locked_layout.get() == Some(geometry) {
            return;
        }
        if self.width() != width || self.height() != height {
            self.set_default_size(width, height);
            self.set_size_request(width, height);
        }
        if helper
            .move_resize_window(own_xid, x, y, width, height)
            .is_ok()
        {
            self.imp().last_locked_layout.set(Some(geometry));
        }
    }
}

fn window_xid(window: &impl IsA<gtk::Window>) -> Option<u32> {
    let surface = window.as_ref().surface()?;
    let x11_surface = surface.downcast_ref::<gdk4_x11::X11Surface>()?;
    Some(x11_surface.xid() as u32)
}
