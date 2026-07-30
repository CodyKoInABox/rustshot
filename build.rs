fn main() {
    println!("cargo:rerun-if-changed=resources/rustshot.manifest");

    #[cfg(target_os = "windows")]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_manifest_file("resources/rustshot.manifest");
        resource
            .compile()
            .expect("failed to embed the Windows application manifest");
    }
}
