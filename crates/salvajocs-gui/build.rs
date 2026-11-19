fn main() {
    // Gate on the target, not the host: a build script always compiles for the host. The
    // dependency is not host-gated so that a cross build to Windows embeds the icon too.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        // Version info is taken from the package metadata, whose name is lowercase; Task Manager
        // and the taskbar show FileDescription.
        winresource::WindowsResource::new()
            .set("FileDescription", "Salvajocs")
            .set("ProductName", "Salvajocs")
            .set_icon("assets/salvajocs.ico")
            .compile()
            .expect("failed to embed the Windows icon");
    }
}
