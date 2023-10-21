#![cfg_attr(target_os = "uefi", no_std)]

extern crate alloc;

pub mod driver_binding;
mod hid;
mod key_queue;
mod keyboard;
mod pointer;

use r_efi::efi;

pub static mut BOOT_SERVICES: *mut efi::BootServices = core::ptr::null_mut();
pub static mut RUNTIME_SERVICES: *mut efi::RuntimeServices = core::ptr::null_mut();
