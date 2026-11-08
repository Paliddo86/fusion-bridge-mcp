use fusion_core::{HostPaths, HostProcess};
use fusion_os_mac::MacHost;
fn main() {
    let h = MacHost;
    println!("fusion_executable : {:?}", h.fusion_executable());
    println!("any_app_complete  : {:?}", fusion_os_mac::any_app_complete());
    println!("is_elevated       : {:?}", h.is_elevated());
    println!("fusion_processes  : {:?}", h.fusion_processes());
}
