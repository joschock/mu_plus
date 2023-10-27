#![cfg_attr(target_os = "uefi", no_std)]

extern crate alloc;

pub mod driver_binding;
mod hid;
//mod key_queue;
//mod keyboard;
mod pointer;

use r_efi::efi;
use rust_boot_services::StandardUefiBootServices;

pub static BOOT_SERVICES: StandardUefiBootServices = StandardUefiBootServices::new();
pub static mut RUNTIME_SERVICES: *mut efi::RuntimeServices = core::ptr::null_mut();

#[cfg(not(test))]
#[macro_export]
macro_rules! supported_handlers {
    () => {
        {
            let mut vec: Vec<Box<dyn HidInputHandler>> = Vec::new();
            vec.push(Box::new(crate::pointer::PointerHandler::new()));
            vec
        }
    };
}
#[cfg(test)]
#[macro_export]
macro_rules! supported_handlers {
    () => {
        {
            let vec: Vec<Box<dyn HidInputHandler>> = Vec::new();
            vec
        }
    };
}