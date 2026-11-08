use fusion_core::{HostPaths, HostProcess};
use fusion_os_mac::MacHost;
fn main() {
    let h = MacHost;
    println!("fusion_processes : {:?}", h.fusion_processes());
    println!("fusion_executable: {:?}", h.fusion_executable());
    println!("addin_dirs       : {:#?}", h.addin_dirs());
    println!("fusion_log_dir   : {:?}", h.fusion_log_dir());
    println!("bridge_state_dir : {:?}", h.bridge_state_dir());
    println!("scratch_dir      : {:?}", h.scratch_dir());
    println!("is_elevated      : {:?}", h.is_elevated());
}
