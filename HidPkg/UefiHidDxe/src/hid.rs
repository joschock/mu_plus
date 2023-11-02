use core::{any::Any, ffi::c_void};

use crate::{driver_binding::DriverBinding, boot_services::UefiBootServices, hid_io::{HidReportReciever, HidIoFactory, HidIo}};

use alloc::{boxed::Box, vec::Vec};

use r_efi::efi;
#[cfg(not(test))]
use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR};

pub trait HidReceiverFactory {
  fn new_hid_receiver_list(&self, controller: efi::Handle) -> Result<Vec<Box<dyn HidReportReciever>>, efi::Status>;
}

// {fb719b29-fda7-4359-ac68-0d46c31a7a7e}
const PRIVATE_HID_CONTEXT_GUID: efi::Guid = efi::Guid::from_fields(
  0xfb719b29,
  0xfda7,
  0x4359,
  0xac,
  0x68,
  &[0x0d, 0x46, 0xc3, 0x1a, 0x7a, 0x7e]);

pub struct DefaultReceiverFactory {}
impl HidReceiverFactory for DefaultReceiverFactory {
  fn new_hid_receiver_list(&self, _controller: efi::Handle) -> Result<Vec<Box<dyn HidReportReciever>>, efi::Status> {
    Ok(Vec::new())
  }
}

pub struct HidFactory {
  hid_io_factory: Box<dyn HidIoFactory>,
  receiver_factory: Box<dyn HidReceiverFactory>,
  agent: efi::Handle
}

impl HidFactory {
  pub fn new(hid_io_factory: Box<dyn HidIoFactory>, receiver_factory: Box<dyn HidReceiverFactory>, agent: efi::Handle) -> Self {
    HidFactory {hid_io_factory, receiver_factory, agent}
  }
}

impl DriverBinding for HidFactory {
  fn driver_binding_supported(
      &mut self,
      _boot_services: &'static dyn UefiBootServices,
      controller: r_efi::efi::Handle,
    ) -> Result<(), efi::Status>
  {
    self.hid_io_factory.new_hid_io(controller).map(|_|())
  }
  fn driver_binding_start(
      &mut self,
      boot_services: &'static dyn UefiBootServices,
      controller: r_efi::efi::Handle,
    ) -> Result<(), efi::Status>
  {
    let mut hid_io = self.hid_io_factory.new_hid_io(controller)?;

    let hid_splitter = Box::new(HidSplitter {
      receivers: self.receiver_factory.new_hid_receiver_list(controller)?
    });

    hid_io.set_report_receiver(hid_splitter)?;

    let hid_instance = Box::into_raw(Box::new(HidInstance::new(hid_io)));

    let mut handle = controller;
    let status = boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(handle),
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      hid_instance as *mut c_void);
    if status != efi::Status::SUCCESS {
      drop(unsafe {Box::from_raw(hid_instance)});
      return Err(status);
    }
    Ok(())
  }
  fn driver_binding_stop(
      &mut self,
      boot_services: &'static dyn UefiBootServices,
      controller: r_efi::efi::Handle,
    ) -> Result<(), efi::Status>
  {
    let mut hid_instance: *mut HidInstance = core::ptr::null_mut();

    let status = boot_services.open_protocol(
      controller,
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::addr_of_mut!(hid_instance) as *mut *mut c_void,
      self.agent,
      controller,
      efi::OPEN_PROTOCOL_GET_PROTOCOL);
    if status != efi::Status::SUCCESS {
      return Err(status);
    }

    let status = boot_services.uninstall_protocol_interface(
      controller,
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      hid_instance as *mut c_void);
    if status != efi::Status::SUCCESS {
      #[cfg(test)]
      panic!("unexpected failure return: {:x?}", status);
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "hid::driver_binding_stop: unexpected failure return: {:x?}", status);
    }

    drop(unsafe {Box::from_raw(hid_instance)});
    Ok(())
  }
}

struct HidInstance {
  _hid_io: Box<dyn HidIo>
}
impl HidInstance {
  fn new(hid_io: Box<dyn HidIo>) -> Self {
    HidInstance { _hid_io: hid_io }
  }
}

struct HidSplitter {
  receivers: Vec<Box<dyn HidReportReciever>>
}

impl HidReportReciever for HidSplitter {
  fn as_any(&mut self) ->  &mut dyn Any {
      self
  }
  fn receive_report(&mut self, report: &[u8], hid_io: &dyn HidIo) {
    for receiver in &mut self.receivers {
      receiver.receive_report(report, hid_io)
    }
  }
}