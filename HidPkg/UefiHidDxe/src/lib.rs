#![cfg_attr(target_os = "uefi", no_std)]

extern crate alloc;

pub mod boot_services;
pub mod driver_binding;
pub mod hid;
pub mod hid_io;

use boot_services::StandardUefiBootServices;
use r_efi::efi;

pub static BOOT_SERVICES: StandardUefiBootServices = StandardUefiBootServices::new();
pub static mut RUNTIME_SERVICES: *mut efi::RuntimeServices = core::ptr::null_mut();
