use crate::bridge::Command;
use crate::dbus::types::LyricsCandidate;
use crate::utils::format_duration_ms;
use adw::prelude::*;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tokio::sync::mpsc as tokio_mpsc;

thread_local! {
    static ACTIVE_DIALOG: RefCell<Option<Rc<LyricsMatchDialogInner>>> = RefCell::new(None);
}

struct LyricsMatchDialogInner {
    dialog: adw::Dialog,
    list_box: gtk::ListBox,
    track_uri: String,
    cmd_tx: tokio_mpsc::UnboundedSender<Command>,
}

impl LyricsMatchDialogInner {
    fn update_results(&self, candidates: Vec<LyricsCandidate>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        for candidate in candidates {
            let row = adw::ActionRow::builder()
                .title(&candidate.title)
                .subtitle(&format!(
                    "{} · {} · {}",
                    candidate.artists.join(", "),
                    candidate.album,
                    format_duration_ms(candidate.duration_ms.unwrap_or(0))
                ))
                .build();

            let buttons = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .valign(gtk::Align::Center)
                .build();

            let preview_btn = gtk::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text("预览")
                .css_classes(["flat"])
                .build();

            let preview_tx = self.cmd_tx.clone();
            let cid = candidate.candidate_id.clone();
            preview_btn.connect_clicked(move |_| {
                let _ = preview_tx.send(Command::PreviewLyricsMatch {
                    candidate_id: cid.clone(),
                });
            });

            let save_btn = gtk::Button::builder()
                .icon_name("document-save-symbolic")
                .tooltip_text("保存")
                .css_classes(["flat"])
                .build();

            let save_tx = self.cmd_tx.clone();
            let cid2 = candidate.candidate_id.clone();
            let uri = self.track_uri.clone();
            let dialog_clone = self.dialog.clone();
            save_btn.connect_clicked(move |_| {
                let _ = save_tx.send(Command::SaveLyricsMatch {
                    track_uri: uri.clone(),
                    candidate_id: cid2.clone(),
                });
                dialog_clone.close();
            });

            buttons.append(&preview_btn);
            buttons.append(&save_btn);
            row.add_suffix(&buttons);

            self.list_box.append(&row);
        }
    }
}

pub fn show(
    parent: &gtk::Widget,
    track_label: &str,
    track_uri: &str,
    cmd_tx: tokio_mpsc::UnboundedSender<Command>,
) {
    let dialog = adw::Dialog::builder()
        .content_width(500)
        .content_height(620)
        .title("手动匹配歌词")
        .build();

    let header = adw::HeaderBar::new();
    header.set_show_title(true);

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();

    vbox.append(&header);

    let info_label = gtk::Label::builder()
        .label(&format!("当前曲目: {}", track_label))
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .halign(gtk::Align::Start)
        .build();
    vbox.append(&info_label);

    let search_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .build();

    let search_entry = gtk::Entry::builder()
        .text(track_label)
        .hexpand(true)
        .placeholder_text("搜索 NetEase / QQ 链接或关键词")
        .build();

    let search_btn = gtk::Button::builder()
        .label("搜索")
        .css_classes(["suggested-action"])
        .build();

    search_box.append(&search_entry);
    search_box.append(&search_btn);
    vbox.append(&search_box);

    let list_box = gtk::ListBox::builder()
        .css_classes(["boxed-list"])
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .selection_mode(gtk::SelectionMode::None)
        .build();

    let scroll = gtk::ScrolledWindow::builder()
        .child(&list_box)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    vbox.append(&scroll);
    dialog.set_child(Some(&vbox));

    let inner = Rc::new(LyricsMatchDialogInner {
        dialog: dialog.clone(),
        list_box: list_box.clone(),
        track_uri: track_uri.to_string(),
        cmd_tx: cmd_tx.clone(),
    });

    ACTIVE_DIALOG.with(|cell| {
        *cell.borrow_mut() = Some(inner.clone());
    });

    dialog.connect_closed(|_| {
        ACTIVE_DIALOG.with(|cell| {
            *cell.borrow_mut() = None;
        });
    });

    let tx_clone = cmd_tx.clone();
    search_btn.connect_clicked(move |_| {
        let query = search_entry.text().to_string();
        if !query.is_empty() {
            let _ = tx_clone.send(Command::SearchLyricsMatches { query });
        }
    });

    dialog.present(Some(parent));

    // Auto-search on open
    let _ = cmd_tx.send(Command::SearchLyricsMatches {
        query: track_label.to_string(),
    });
}

pub fn dispatch_results(candidates: Vec<LyricsCandidate>) {
    ACTIVE_DIALOG.with(|cell| {
        if let Some(inner) = cell.borrow().as_ref() {
            inner.update_results(candidates);
        }
    });
}
