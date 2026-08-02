fn main() {
    println!("cargo:rerun-if-changed=resources/rustshot.manifest");
    println!("cargo:rerun-if-changed=resources/settings.rc");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_manifest_file("resources/rustshot.manifest");
        resource.append_rc_content(include_str!("resources/settings.rc"));
        resource
            .compile()
            .expect("failed to embed the Windows application manifest");
    }
}
