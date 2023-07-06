use std::ffi::CStr;

use libz_ng_sys::zlibVersion;

fn main() {
    let ver = unsafe { zlibVersion() };
    let ver_cstr = unsafe { CStr::from_ptr(ver) };
    let version = ver_cstr.to_str().unwrap();
    assert!(!version.is_empty());
}
