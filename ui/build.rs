fn main() {
    println!("cargo:rerun-if-env-changed=PERIPHERAL_NATIVE_SHA256");
    println!("cargo:rerun-if-env-changed=PERIPHERAL_NATIVE_BUILD");
    // gpui-pre supplies the process manifest (including PerMonitorV2).
    // build.ps1 merges the XAML maxversiontested declaration into that resource.
}
