use tracing_panic::panic_hook;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, layer::SubscriberExt};

use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_TASKMODAL, MessageBoxW};
use windows::core::PCWSTR;

pub fn show_error_message_box(message: String, title: &str) {
    let mut message_utf16: Vec<u16> = message.encode_utf16().collect();
    message_utf16.push(0);
    let mut title_utf16: Vec<u16> = title.encode_utf16().collect();
    title_utf16.push(0);

    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message_utf16.as_ptr()),
            PCWSTR(title_utf16.as_ptr()),
            MB_ICONERROR | MB_OK | MB_TASKMODAL,
        );
    }
}

pub fn custom_panic_hook(panic_info: &std::panic::PanicHookInfo) {
    let message;
    let reason = panic_info.payload().downcast_ref::<&str>();

    if let Some(location) = panic_info.location() {
        message = format!(
            "A panic occurred at {}:{}\nReason: {}",
            location.file(),
            location.line(),
            reason.map_or("Unknown", |v| v),
        );
    } else {
        message = format!(
            "A panic occurred\nReason: {}",
            reason.map_or("Unknown", |v| v)
        );
    }

    show_error_message_box(message, "Debug Text View Error");
    panic_hook(panic_info);
    std::process::abort();
}

pub fn setup_logging() {
    // unsafe {
    //     windows::Win32::System::Console::AttachConsole(
    //         windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
    //     )
    //     .unwrap();
    // }
    let filter = tracing_subscriber::filter::EnvFilter::from_default_env()
        .add_directive(tracing_subscriber::filter::LevelFilter::DEBUG.into());

    let stdout_log = tracing_subscriber::fmt::layer().pretty();

    tracing_subscriber::registry()
        .with(stdout_log.with_filter(filter))
        .init();
}
