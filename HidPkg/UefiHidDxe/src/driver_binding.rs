use alloc::boxed::Box;
use core::ffi::c_void;

#[cfg(test)]
use mockall::automock;
use r_efi::{efi, protocols};

use crate::boot_services::UefiBootServices;

#[cfg_attr(test, automock)]
pub trait DriverBinding {
  fn driver_binding_supported(&self, boot_services: &dyn UefiBootServices, controller: efi::Handle) -> efi::Status;
  fn driver_binding_start(&mut self, boot_services: &dyn UefiBootServices, controller: efi::Handle) -> efi::Status;
  fn driver_binding_stop(&mut self, boot_services: &dyn UefiBootServices, controller: efi::Handle) -> efi::Status;
}

#[repr(C)]
pub struct UefiDriverBinding {
  uefi_binding: protocols::driver_binding::Protocol,
  boot_services: &'static dyn UefiBootServices,
  binding: Box<dyn DriverBinding>,
}

impl UefiDriverBinding {
  pub fn new(
    boot_services: &'static dyn UefiBootServices,
    binding: Box<dyn DriverBinding>,
    handle: efi::Handle,
  ) -> Self {
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
    let boot_services = &unsafe { uefi_driver_binding_mgr_ptr.as_ref().unwrap() }.boot_services;
    let status = boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(handle),
      &protocols::driver_binding::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      uefi_driver_binding_mgr_ptr as *mut c_void,
    );
    if status.is_error() {
      Err(status)
    } else {
      Ok(uefi_driver_binding_mgr_ptr)
    }
  }
  pub fn uninstall(uefi_binding: *mut UefiDriverBinding) -> Result<Self, efi::Status> {
    let ptr = uefi_binding;
    let binding = unsafe { Box::from_raw(uefi_binding) };
    let status = binding.boot_services.uninstall_protocol_interface(
      binding.uefi_binding.driver_binding_handle,
      &protocols::driver_binding::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      ptr as *mut c_void,
    );
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
    let uefi_binding = unsafe { (this as *mut UefiDriverBinding).as_mut() }.expect("bad this pointer");
    uefi_binding.binding.driver_binding_supported(uefi_binding.boot_services, controller)
  }
  extern "efiapi" fn driver_binding_start(
    this: *mut protocols::driver_binding::Protocol,
    controller: efi::Handle,
    _remaining_device_path: *mut protocols::device_path::Protocol,
  ) -> efi::Status {
    let uefi_binding = unsafe { (this as *mut UefiDriverBinding).as_mut() }.expect("bad this pointer");
    uefi_binding.binding.driver_binding_start(uefi_binding.boot_services, controller)
  }
  pub extern "efiapi" fn driver_binding_stop(
    this: *mut protocols::driver_binding::Protocol,
    controller: efi::Handle,
    _num_children: usize,
    _child_handle_buffer: *mut efi::Handle,
  ) -> efi::Status {
    let uefi_binding = unsafe { (this as *mut UefiDriverBinding).as_mut() }.expect("bad this pointer");
    uefi_binding.binding.driver_binding_stop(uefi_binding.boot_services, controller)
  }
}

#[cfg(test)]
mod test {

  use super::{MockDriverBinding, UefiDriverBinding};
  use crate::boot_services::MockUefiBootServices;
  use r_efi::{efi, protocols};

  #[test]
  fn new_should_instantiate_new_uefi_driver_binding() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_ref().unwrap() };

    let binding = MockDriverBinding::new();
    let handle = 0x1234 as efi::Handle;
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);

    assert!(driver_binding.uefi_binding.supported == UefiDriverBinding::driver_binding_supported);
    assert!(driver_binding.uefi_binding.start == UefiDriverBinding::driver_binding_start);
    assert!(driver_binding.uefi_binding.stop == UefiDriverBinding::driver_binding_stop);
    assert_eq!(driver_binding.uefi_binding.version, 1);
    assert_eq!(driver_binding.uefi_binding.image_handle, handle);
    assert_eq!(driver_binding.uefi_binding.driver_binding_handle, handle);

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn install_should_install_the_driver_binding() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    //expect a call to install_protocol_interface
    boot_services
      .expect_install_protocol_interface()
      .withf(|handle, protocol, interface_type, interface| {
        assert_ne!(*handle, core::ptr::null_mut());
        assert_eq!(unsafe { **protocol }, protocols::driver_binding::PROTOCOL_GUID);
        assert_eq!(*interface_type, efi::NATIVE_INTERFACE);
        assert_ne!(*interface, core::ptr::null_mut());
        true
      })
      .returning(|_, _, _, _| efi::Status::SUCCESS);

    let handle = 0x1234 as efi::Handle;

    let binding = MockDriverBinding::new();
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);
    driver_binding.install().unwrap();

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn install_should_report_failures_to_install_the_driver_binding() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    //expect a call to install_protocol_interface
    boot_services.expect_install_protocol_interface().returning(|_, _, _, _| efi::Status::OUT_OF_RESOURCES);

    let handle = 0x1234 as efi::Handle;

    let binding = MockDriverBinding::new();
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);
    assert_eq!(driver_binding.install(), Err(efi::Status::OUT_OF_RESOURCES));

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn uninstall_should_uninstall_the_driver_binding() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    //expect a call to install_protocol_interface
    boot_services
      .expect_install_protocol_interface()
      .times(1)
      .withf(|handle, protocol, interface_type, interface| {
        assert_ne!(*handle, core::ptr::null_mut());
        assert_eq!(unsafe { **protocol }, protocols::driver_binding::PROTOCOL_GUID);
        assert_eq!(*interface_type, efi::NATIVE_INTERFACE);
        assert_ne!(*interface, core::ptr::null_mut());
        true
      })
      .returning(|_, _, _, _| efi::Status::SUCCESS);

    let handle = 0x1234 as efi::Handle;

    let binding = MockDriverBinding::new();
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);
    let binding_ptr = driver_binding.install().unwrap();

    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
    boot_services.checkpoint();
    boot_services
      .expect_uninstall_protocol_interface()
      .times(1)
      .withf(|handle, protocol, interface| {
        assert_ne!(*handle, core::ptr::null_mut());
        assert_eq!(unsafe { **protocol }, protocols::driver_binding::PROTOCOL_GUID);
        assert_ne!(*interface, core::ptr::null_mut());
        true
      })
      .returning(|_, _, _| efi::Status::SUCCESS);

    let driver_binding = UefiDriverBinding::uninstall(binding_ptr).unwrap();

    assert!(driver_binding.uefi_binding.supported == UefiDriverBinding::driver_binding_supported);
    assert!(driver_binding.uefi_binding.start == UefiDriverBinding::driver_binding_start);
    assert!(driver_binding.uefi_binding.stop == UefiDriverBinding::driver_binding_stop);
    assert_eq!(driver_binding.uefi_binding.version, 1);
    assert_eq!(driver_binding.uefi_binding.image_handle, handle);
    assert_eq!(driver_binding.uefi_binding.driver_binding_handle, handle);

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn uninstall_should_report_failures_to_uninstall_the_driver() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    //expect a call to install_protocol_interface
    boot_services.expect_install_protocol_interface().times(1).returning(|_, _, _, _| efi::Status::SUCCESS);

    let handle = 0x1234 as efi::Handle;

    let binding = MockDriverBinding::new();
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);
    let binding_ptr = driver_binding.install().unwrap();

    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
    boot_services.checkpoint();
    boot_services.expect_uninstall_protocol_interface().times(1).returning(|_, _, _| efi::Status::INVALID_PARAMETER);

    assert_eq!(UefiDriverBinding::uninstall(binding_ptr).err(), Some(efi::Status::INVALID_PARAMETER));

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn driver_binding_should_call_driver_binding_routines() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    //expect a call to install_protocol_interface
    boot_services.expect_install_protocol_interface().returning(|_, _, _, _| efi::Status::SUCCESS);

    let mut binding = MockDriverBinding::new();

    binding.expect_driver_binding_supported().returning(|_, _| efi::Status::SUCCESS);

    binding.expect_driver_binding_start().returning(|_, _| efi::Status::SUCCESS);

    binding.expect_driver_binding_stop().returning(|_, _| efi::Status::SUCCESS);

    let handle = 0x1234 as efi::Handle;
    let driver_binding = UefiDriverBinding::new(boot_services, Box::new(binding), handle);
    let binding_ptr = driver_binding.install().unwrap();

    let driver_binding_ref = unsafe { binding_ptr.as_ref().unwrap() };
    let this_ptr = binding_ptr as *mut protocols::driver_binding::Protocol;

    let controller_handle = 0x4321 as efi::Handle;
    assert_eq!(
      (driver_binding_ref.uefi_binding.supported)(this_ptr, controller_handle, core::ptr::null_mut()),
      efi::Status::SUCCESS
    );
    assert_eq!(
      (driver_binding_ref.uefi_binding.start)(this_ptr, controller_handle, core::ptr::null_mut()),
      efi::Status::SUCCESS
    );
    assert_eq!(
      (driver_binding_ref.uefi_binding.stop)(this_ptr, controller_handle, 0, core::ptr::null_mut()),
      efi::Status::SUCCESS
    );
  }
}
