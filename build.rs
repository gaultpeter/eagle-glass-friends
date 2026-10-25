//! Embeds Windows version resources, the application icon and an execution
//! manifest into `eglass.exe`.
//!
//! This is packaging hygiene, not security: a binary with a real
//! FileDescription/ProductName/CompanyName and an `asInvoker` manifest looks
//! like a normal application to SmartScreen, Defender and shell tooling
//! instead of an anonymous blob. It does not sign anything - see
//! `docs/SIGNING.md` for that.
//!
//! Windows resources are only embedded when the *host* is Windows. The build
//! script is a no-op everywhere else so `cargo check` keeps working.

use std::path::PathBuf;

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="EagleGlass.eglass" version="1.0.0.0" processorArchitecture="*"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <!-- Windows 10 and 11 -->
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <!-- Windows 8.1 -->
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/eglass.ico");

    // DPI awareness is deliberately absent: winit sets PerMonitorV2 at
    // startup, and declaring a different mode here would downgrade it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    #[cfg(windows)]
    embed_windows_resources();

    #[cfg(not(windows))]
    println!("cargo:warning=not building on Windows; skipping exe resources");
}

#[cfg(windows)]
fn embed_windows_resources() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = root.join("assets").join("eglass.ico");
    if !icon.is_file() {
        println!("cargo:warning=assets/eglass.ico is missing; run scripts/make-icon.ps1");
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = [0u16; 4];
    for (slot, piece) in parts.iter_mut().zip(
        version
            .split(|c: char| c == '.' || c == '-' || c == '+')
            .take(4),
    ) {
        *slot = piece.parse().unwrap_or(0);
    }
    let packed = (u64::from(parts[0]) << 48)
        | (u64::from(parts[1]) << 32)
        | (u64::from(parts[2]) << 16)
        | u64::from(parts[3]);

    let mut res = winresource::WindowsResource::new();
    res.set_language(0x0409); // en-US
    res.set_icon(icon.to_string_lossy().as_ref());
    res.set_manifest(MANIFEST);
    res.set("FileDescription", "Eagle Glass - IPTV player");
    res.set("ProductName", "Eagle Glass");
    res.set("CompanyName", "Peter Gault");
    res.set("LegalCopyright", "Copyright (C) 2026 Peter Gault. MIT License.");
    res.set("OriginalFilename", "eglass.exe");
    res.set("InternalName", "eglass");
    res.set(
        "Comments",
        "Native Xtream Codes IPTV player (Rust + egui + embedded libVLC).",
    );
    res.set("ProductVersion", version.as_str());
    res.set("FileVersion", version.as_str());
    res.set_version_info(winresource::VersionInfo::FILEVERSION, packed);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
    res.set_version_info(winresource::VersionInfo::FILEOS, 0x0004_0004); // VOS_NT_WINDOWS32
    res.set_version_info(winresource::VersionInfo::FILETYPE, 0x0000_0001); // VFT_APP

    if let Err(err) = res.compile() {
        // A warning keeps `cargo check` usable on machines without rc.exe;
        // release builds are checked for resources by the release workflow.
        println!("cargo:warning=could not embed Windows resources: {err}");
    }
}
