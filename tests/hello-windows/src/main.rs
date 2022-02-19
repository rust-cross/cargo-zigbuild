use winapi::um::sysinfoapi::GetVersion as WinApiGetVersion;
use windows_sys::Win32::System::SystemInformation::GetVersion as WindowsSysGetVersion;

fn main() {
    let v1 = unsafe { WinApiGetVersion() };
    println!("version from winapi: {}", v1);

    let v2 = unsafe { WindowsSysGetVersion() };
    println!("version from windows-sys: {}", v2);

    assert_eq!(v1, v2);
}
