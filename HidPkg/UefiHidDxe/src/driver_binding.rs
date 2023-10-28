use core::ffi::c_void;
use alloc::boxed::Box;

use r_efi::{efi, protocols};
use rust_boot_services::UefiBootServices;

pub trait DriverBinding {
  fn driver_binding_supported(
    &self,
    boot_services: &dyn UefiBootServices,
    controller: efi::Handle,
  ) -> efi::Status;
  fn driver_binding_start(
    &mut self,
    boot_services: &dyn UefiBootServices,
    controller: efi::Handle,
  ) -> efi::Status;
  fn driver_binding_stop(
    &mut self,
    boot_services: &dyn UefiBootServices,
    controller: efi::Handle,
  ) -> efi::Status;
}

#[repr(C)]
pub struct UefiDriverBinding {
  uefi_binding: protocols::driver_binding::Protocol,
  boot_services: &'static dyn UefiBootServices,
  binding: Box<dyn DriverBinding>,
}

impl UefiDriverBinding {
  pub fn new(boot_services: &'static dyn UefiBootServices, binding: Box<dyn DriverBinding>, handle: efi::Handle) -> Self {
    let uefi_binding = protocols::driver_binding::Protocol {
      supported: Self::driver_binding_supported,
      start: Self::driver_binding_start,
      stop: Self::driver_binding_stop,
      version: 1,
      image_handle: handle,
      driver_binding_handle: handle,
    };
    Self { uefi_binding, boot_services, binding }
  }

  pub fn install(self) -> Result<*mut UefiDriverBinding, efi::Status> {
    let mut handle = self.uefi_binding.driver_binding_handle;
    let uefi_driver_binding_mgr_ptr = Box::into_raw(Box::new(self));
    let boot_services = &unsafe {uefi_driver_binding_mgr_ptr.as_ref().unwrap()}.boot_services;
    let status = boot_services.install_protocol_interface(
        core::ptr::addr_of_mut!(handle),
        &protocols::driver_binding::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        efi::NATIVE_INTERFACE,
        uefi_driver_binding_mgr_ptr as *mut c_void);
    if status.is_error() {
      Err(status)
    } else {
      Ok(uefi_driver_binding_mgr_ptr)
    }
  }
  pub fn uninstall(uefi_binding: *mut UefiDriverBinding) -> Result<Self, efi::Status> {
    let ptr = uefi_binding;
    let binding = unsafe {Box::from_raw(uefi_binding)};
    let status = binding.boot_services.uninstall_protocol_interface(
      binding.uefi_binding.driver_binding_handle,
      &protocols::driver_binding::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      ptr as *mut c_void);
    if status.is_error() {
      Err(status)
    } else {
      Ok(*binding)
    }
  }
  extern "efiapi" fn driver_binding_supported(
    this: *mut protocols::driver_binding::Protocol,
    controller: efi::Handle,
    _remaining_device_path: *mut protocols::device_path::Protocol,
  ) -> efi::Status {
    let uefi_binding = unsafe {(this as *mut UefiDriverBinding).as_mut()}.expect("bad this pointer");
    uefi_binding.binding.driver_binding_supported(uefi_binding.boot_services, controller)
  }
  extern "efiapi" fn driver_binding_start(
    this: *mut protocols::driver_binding::Protocol,
    controller: efi::Handle,
    _remaining_device_path: *mut protocols::device_path::Protocol,
  ) -> efi::Status {
    let uefi_binding = unsafe {(this as *mut UefiDriverBinding).as_mut()}.expect("bad this pointer");
    uefi_binding.binding.driver_binding_start(uefi_binding.boot_services, controller)
  }
  pub extern "efiapi" fn driver_binding_stop(
    this: *mut protocols::driver_binding::Protocol,
    controller: efi::Handle,
    _num_children: usize,
    _child_handle_buffer: *mut efi::Handle,
  ) -> efi::Status {
    let uefi_binding = unsafe {(this as *mut UefiDriverBinding).as_mut()}.expect("bad this pointer");
    uefi_binding.binding.driver_binding_stop(uefi_binding.boot_services, controller)
  }
}
