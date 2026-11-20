// The application links libtdjson.so.<version> (the SONAME of the real
// TDLib), so the fake has the same SONAME and replaces it through
// LD_LIBRARY_PATH.
fn main() {
    let version = std::env::var("FAKE_TDJSON_SONAME_VERSION").unwrap_or_else(|_| "1.8.65".into());
    println!("cargo:rerun-if-env-changed=FAKE_TDJSON_SONAME_VERSION");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libtdjson.so.{version}");
}
