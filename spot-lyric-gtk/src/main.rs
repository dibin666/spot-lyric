mod application;
mod backend_runtime;
mod bridge;
mod config;
mod dbus;
mod dialogs;
mod platform;
mod tray;
mod utils;
mod widgets;

use gtk::gio;
use gtk::prelude::*;

fn main() -> glib::ExitCode {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("\n=== SPOT-LYRIC-GTK PANIC ===");
        eprintln!("{info}");
        eprintln!("Backtrace:\n{}", std::backtrace::Backtrace::force_capture());
        eprintln!("=== END PANIC ===\n");
    }));

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "spot_lyric_gtk=info".into()),
        )
        .init();

    ensure_development_gsettings_schema_dir();

    gio::resources_register_include!("spot_lyric.gresource")
        .expect("Failed to register GResource bundle");

    let backend_runtime = backend_runtime::BackendRuntime::default();
    let app = application::SpotLyricApplication::new(backend_runtime.clone());
    let exit_code = app.run();
    backend_runtime.shutdown();
    exit_code
}

fn ensure_development_gsettings_schema_dir() {
    if std::env::var_os("GSETTINGS_SCHEMA_DIR").is_some() {
        return;
    }

    if let Some(schema_dir) = option_env!("SPOT_LYRIC_GSETTINGS_SCHEMA_DIR") {
        std::env::set_var("GSETTINGS_SCHEMA_DIR", schema_dir);
    }
}

#[cfg(test)]
mod tests {
    fn schema_default(schema: &str, key_name: &str) -> Option<String> {
        let key_start = schema.find(&format!("<key name=\"{key_name}\""))?;
        let after_key = &schema[key_start..];
        let default_start = after_key.find("<default>")? + "<default>".len();
        let after_default = &after_key[default_start..];
        let default_end = after_default.find("</default>")?;
        Some(after_default[..default_end].trim().to_string())
    }

    #[test]
    fn desktop_lyrics_are_draggable_by_default() {
        let schema = include_str!("../data/cn.spotlyric.Gtk.gschema.xml");

        assert_eq!(
            schema_default(schema, "desktop-lyrics-locked").as_deref(),
            Some("false")
        );
    }
}
