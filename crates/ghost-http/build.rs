//! Embeds the Windows version resource and application manifest into this
//! binary. Antivirus heuristics and SmartScreen score an executable with no
//! publisher, product or version metadata as more suspicious than one that
//! states who built it and what it is - and Rust binaries carry none by
//! default. Windows only; a no-op for every other target, including the Linux
//! cross-build from a Windows host.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/ghost.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", "Northtek (FrostByte LLC)")
        .set("ProductName", "Ghost")
        .set("FileDescription", "Ghost desktop automation HTTP server")
        .set("InternalName", "ghost-http")
        .set("OriginalFilename", "ghost-http.exe")
        .set("LegalCopyright", "Copyright (c) Northtek. MIT License.")
        .set("Comments", "https://github.com/NORTHTEKDevs/ghost")
        .set_manifest(&manifest(&version));
    if std::path::Path::new("../../assets/ghost.ico").exists() {
        res.set_icon("../../assets/ghost.ico");
    }
    res.compile().expect("embed Windows version resource and manifest");
}

/// asInvoker (never asks for elevation), declared Windows 10/11 support, the
/// same per-monitor-v2 DPI awareness the code sets at runtime, long paths on.
fn manifest(version: &str) -> String {
    let four_part = {
        let mut parts: Vec<&str> = version.split('.').take(3).collect();
        while parts.len() < 3 {
            parts.push("0");
        }
        format!("{}.0", parts.join("."))
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Northtek.Ghost.ghost-http" version="{four_part}"/>
  <description>Ghost desktop automation HTTP server</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#
    )
}
