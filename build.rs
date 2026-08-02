fn main() {
    println!("cargo:rerun-if-changed=resources/rustshot.manifest");
    println!("cargo:rerun-if-changed=resources/settings.rc");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    let package_version = std::env::var("CARGO_PKG_VERSION")
        .expect("Cargo always provides the package version to build scripts");
    let numeric_version = package_version
        .split_once('-')
        .map_or(package_version.as_str(), |(version, _)| version);
    let expected_manifest_version = format!("version=\"{numeric_version}.0\"");
    assert!(
        include_str!("resources/rustshot.manifest").contains(&expected_manifest_version),
        "resources/rustshot.manifest must contain {expected_manifest_version}"
    );

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_manifest_file("resources/rustshot.manifest");
        resource.append_rc_content(include_str!("resources/settings.rc"));
        resource
            .compile()
            .expect("failed to embed the Windows application manifest");
    }
}
