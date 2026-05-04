use crate::bridge::Command;
use adw::prelude::*;
use gtk::prelude::*;
use tokio::sync::mpsc;

pub fn show_paste_dialog(parent: &gtk::Widget, cmd_tx: mpsc::UnboundedSender<Command>) {
    let dialog = adw::Dialog::builder()
        .content_width(520)
        .content_height(420)
        .title("粘贴 Cookie")
        .build();

    let header = adw::HeaderBar::new();
    header.set_show_title(true);

    let text_buffer = gtk::TextBuffer::new(None);
    let text_view = gtk::TextView::builder()
        .buffer(&text_buffer)
        .wrap_mode(gtk::WrapMode::Char)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    let scroll = gtk::ScrolledWindow::builder()
        .child(&text_view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let save_btn = gtk::Button::builder()
        .label("保存")
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();

    text_buffer.connect_changed(gtk::glib::clone!(@weak save_btn => move |buf| {
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
        save_btn.set_sensitive(!text.is_empty());
    }));

    let dialog_clone = dialog.clone();
    save_btn.connect_clicked(move |_| {
        let text = text_buffer.text(&text_buffer.start_iter(), &text_buffer.end_iter(), false);
        let _ = cmd_tx.send(Command::ImportCookieString(text.to_string()));
        dialog_clone.close();
    });

    header.pack_end(&save_btn);

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();

    let label = gtk::Label::builder()
        .label("将浏览器开发者工具中的整段 cookie 字符串粘贴到此处（必须包含 sp_dc 字段）")
        .wrap(true)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    vbox.append(&header);
    vbox.append(&label);
    vbox.append(&scroll);

    dialog.set_child(Some(&vbox));
    dialog.present(Some(parent));
}
