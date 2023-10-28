#![cfg_attr(target_os = "uefi", no_std)]

extern crate alloc;

pub mod driver_binding;
pub mod hid;

use r_efi::efi;
use rust_boot_services::StandardUefiBootServices;

pub static BOOT_SERVICES: StandardUefiBootServices = StandardUefiBootServices::new();
pub static mut RUNTIME_SERVICES: *mut efi::RuntimeServices = core::ptr::null_mut();
