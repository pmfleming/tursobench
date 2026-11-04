//! Verify the actual native display selected by eframe during desktop checks.
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

#[cfg(not(any(feature = "wayland", feature = "x11")))]
compile_error!("Enable a desktop backend: wayland (default) or x11");

pub fn configure_backend(_builder: &mut eframe::EventLoopBuilder<eframe::UserEvent>) {
    // AccessKit's defaults enable both winit backends transitively. Keep our
    // selected backend authoritative without removing accessibility support.
    #[cfg(all(target_os = "linux", feature = "wayland", not(feature = "x11")))]
    {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        _builder.with_wayland();
    }
    #[cfg(all(target_os = "linux", feature = "x11", not(feature = "wayland")))]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        _builder.with_x11();
    }
}

pub fn verify_backend(
    cc: &eframe::CreationContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Ok(expected) = std::env::var("TURSOBENCH_EXPECT_BACKEND") else {
        return Ok(());
    };
    let actual = match cc.display_handle()?.as_raw() {
        RawDisplayHandle::Wayland(_) => "wayland",
        RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => "x11",
        _ => "other",
    };
    if expected != actual {
        return Err(std::io::Error::other(format!(
            "Expected desktop backend {expected}, got {actual}"
        ))
        .into());
    }
    eprintln!("DESKTOP BACKEND VERIFIED: {actual}");
    Ok(())
}
