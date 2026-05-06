use adw::subclass::prelude::*;
use gtk::prelude::*;

use super::FullLyricsWindow;

impl FullLyricsWindow {
    pub(super) fn setup_window(&self) {
        self.set_title(Some("完整歌词"));
        self.set_default_size(420, 720);
        self.set_size_request(360, 420);
        self.set_decorated(false);
        self.set_resizable(false);
        self.add_css_class("full-lyrics-window");
    }

    pub(super) fn build_ui(&self) {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["full-lyrics-root"])
            .build();
        root.append(&self.build_header());
        root.append(&self.build_stack());
        self.set_child(Some(&root));
    }

    fn build_header(&self) -> adw::HeaderBar {
        let header = adw::HeaderBar::new();
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
        let title_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        let title = short_label("完整歌词", "full-lyrics-title");
        let subtitle = short_label("播放歌曲后显示完整歌词", "full-lyrics-subtitle");
        title_box.append(&title);
        title_box.append(&subtitle);
        header.set_title_widget(Some(&title_box));
        header.pack_end(&self.close_button());
        *self.imp().title_label.borrow_mut() = Some(title);
        *self.imp().subtitle_label.borrow_mut() = Some(subtitle);
        header
    }

    fn close_button(&self) -> gtk::Button {
        let button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("关闭完整歌词")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let weak = self.downgrade();
        button.connect_clicked(move |_| {
            if let Some(win) = weak.upgrade() {
                win.hide_locked();
            }
        });
        button
    }

    fn build_stack(&self) -> gtk::Stack {
        let stack = gtk::Stack::builder().vexpand(true).hexpand(true).build();
        let empty = empty_label("暂无歌词\n播放歌曲或点击手动匹配预览后显示完整歌词");
        let lines_box = lyrics_box();
        let scrolled = scrolled_lines(&lines_box);
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&scrolled, Some("lyrics"));
        stack.set_visible_child_name("empty");
        self.store_stack_widgets(&stack, empty, scrolled, lines_box);
        stack
    }

    fn store_stack_widgets(
        &self,
        stack: &gtk::Stack,
        empty: gtk::Label,
        scrolled: gtk::ScrolledWindow,
        lines_box: gtk::Box,
    ) {
        let imp = self.imp();
        *imp.stack.borrow_mut() = Some(stack.clone());
        *imp.empty_label.borrow_mut() = Some(empty);
        *imp.scrolled.borrow_mut() = Some(scrolled);
        *imp.lines_box.borrow_mut() = Some(lines_box);
    }
}

fn short_label(text: &str, css_class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes([css_class])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build()
}

fn empty_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(["full-lyrics-empty"])
        .justify(gtk::Justification::Center)
        .wrap(true)
        .vexpand(true)
        .hexpand(true)
        .build()
}

fn lyrics_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(14)
        .margin_end(14)
        .css_classes(["full-lyrics-list"])
        .build()
}

fn scrolled_lines(lines_box: &gtk::Box) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .child(lines_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(true)
        .build()
}
