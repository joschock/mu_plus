//! HID I/O support for HID input driver.
//!
//! This module manages interactions with the lower-layer drivers that produce the HidIo protocol.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

mod uefi_interface {
  use core::{ffi::c_void, slice::from_raw_parts};
  use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR};
  use rust_boot_services::UefiBootServices;
  use r_efi::efi;
  use alloc::{boxed::Box, vec, vec::Vec};
  use super::HidHandlers;

  pub type ReportCallback = fn(handlers: &mut HidHandlers, report: &[u8]);

  struct CallbackContext {
    callback: ReportCallback,
    handlers: *mut HidHandlers,
  }

  pub struct UefiHidIo<'a> {
    hid_io: *const hid_io::protocol::Protocol,
    boot_services: &'a dyn UefiBootServices,
    controller: efi::Handle,
    agent: efi::Handle,
    callback_context: Option<*mut CallbackContext>
  }

  impl<'a> UefiHidIo<'a> {
    pub fn new(boot_services: &'a impl UefiBootServices, controller: efi::Handle, agent: efi::Handle) -> Result<UefiHidIo<'a>, efi::Status> {
      // retrieve the HidIo instance for the given controller.
      let mut hid_io: *mut hid_io::protocol::Protocol = core::ptr::null_mut();
      let status = boot_services.open_protocol(
        controller,
        &hid_io::protocol::GUID as *const efi::Guid as *mut efi::Guid,
        core::ptr::addr_of_mut!(hid_io) as *mut *mut c_void,
        agent,
        controller,
        efi::OPEN_PROTOCOL_BY_DRIVER,
      );
      if status.is_error() {
        return Err(status);
      }
      Ok(UefiHidIo {hid_io, boot_services, controller, agent, callback_context: None})
    }

    pub fn get_report_descriptor(&self) -> Result<Vec<u8>, efi::Status> {
      let hid_io = unsafe {self.hid_io.as_ref().expect("bad hid io pointer")};
      let mut report_descriptor_size = 0;

      match (hid_io.get_report_descriptor)(
        hid_io, core::ptr::addr_of_mut!(report_descriptor_size),
        core::ptr::null_mut())
      {
        efi::Status::BUFFER_TOO_SMALL => (),
        _ => return Err(efi::Status::DEVICE_ERROR)
      };

      let mut report_descriptor_buffer = vec![0u8; report_descriptor_size];
      let report_descriptor_buffer_ptr = report_descriptor_buffer.as_mut_ptr();

      match (hid_io.get_report_descriptor)(
        hid_io, core::ptr::addr_of_mut!(report_descriptor_size),
        report_descriptor_buffer_ptr as *mut c_void)
      {
        efi::Status::SUCCESS => (),
        err => return Err(err)
      };

      Ok(report_descriptor_buffer)
    }

    pub fn _send_output_report(&self, report_id: Option<u8>, report: &[u8]) -> Result<(), efi::Status> {
      let hid_io = unsafe {self.hid_io.as_ref().expect("bad hid io pointer")};

      match (hid_io.set_report)(
        self.hid_io,
        report_id.unwrap_or(0),
        hid_io::protocol::HidReportType::OutputReport,
        report.len(),
        report.as_ptr() as *mut c_void)
      {
        efi::Status::SUCCESS => Ok(()),
        err => Err(err)
      }
    }

    pub fn initiate_reports(&mut self, callback: ReportCallback, handlers: Box<HidHandlers>) ->Result<(), efi::Status> {
      let hid_io = unsafe {self.hid_io.as_ref().expect("bad hid io pointer")};
      let callback_context = Box::into_raw(Box::new(CallbackContext {
        callback,
        handlers: Box::into_raw(handlers)
      }));

      self.callback_context = Some(callback_context);

      match (hid_io.register_report_callback)(self.hid_io, Self::on_input_report, callback_context as *mut c_void) {
        efi::Status::SUCCESS => Ok(()),
        err => {
          let _ = self.terminate_reports();
          Err(err)
        },
      }
    }

    pub fn terminate_reports(&mut self) -> Result<Box<HidHandlers>, efi::Status> {
      let callback_context = self.callback_context.take().ok_or(efi::Status::NOT_STARTED)?;
      let hid_io = unsafe {self.hid_io.as_ref().expect("bad hid io pointer")};

      match (hid_io.unregister_report_callback)(self.hid_io, Self::on_input_report) {
        efi::Status::NOT_STARTED | efi::Status::SUCCESS => (), //not started case may occur on init failure.
        err => return Err(err),
      };

      let callback_context = unsafe {Box::from_raw(callback_context)};
      Ok(unsafe {Box::from_raw(callback_context.handlers)})
    }

    extern "efiapi" fn on_input_report(report_buffer_size: u16, report_buffer: *mut c_void, context: *mut c_void) {
      unsafe {
        let report = from_raw_parts(report_buffer as *mut u8, report_buffer_size as usize);
        let callback_context = (context as *mut CallbackContext).as_mut().expect("bad callback context ptr");
        let context = callback_context.handlers.as_mut().expect("bad context pointer");
        (callback_context.callback)(context, report);
      };
    }
  }

  impl Drop for UefiHidIo<'_> {
    fn drop(&mut self) {
      if self.callback_context.is_some() {
        let _ = self.terminate_reports();
      }
      self.boot_services.close_protocol(
        self.controller,
        &hid_io::protocol::GUID as *const efi::Guid as *mut efi::Guid,
        self.agent,
        self.controller);
    }
  }

  // private GUID used to save and retrieve a UefiHidIo context {3ae107d3-7249-4f45-8b99-32735a13999b}
  const PRIVATE_CONTEXT_GUID: efi::Guid = efi::Guid::from_fields(
      0x3ae107d3,
      0x7249,
      0x4f45,
      0x8b,
      0x99,
      &[0x32, 0x73, 0x5a, 0x13, 0x99, 0x9b]);

  pub fn save_hid_io_context(boot_services: &impl UefiBootServices, controller: efi::Handle, context: UefiHidIo) -> Result<(), efi::Status> {
    let context_ptr = Box::into_raw(Box::new(context));

    match boot_services.install_protocol_interface(
      core::ptr::addr_of!(controller) as *mut efi::Handle,
      &PRIVATE_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      context_ptr as *mut c_void)
    {
      efi::Status::SUCCESS => Ok(()),
      err => Err(err)
    }
  }

  pub fn _retrieve_hid_io_context(boot_services: &impl UefiBootServices, controller: efi::Handle) -> Result<UefiHidIo, efi::Status> {
    let mut context_ptr: *mut UefiHidIo<'_> = core::ptr::null_mut();

    match boot_services.open_protocol(
      controller,
      &PRIVATE_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::addr_of_mut!(context_ptr) as *mut *mut c_void,
      core::ptr::null_mut(),
      controller,
    efi::OPEN_PROTOCOL_GET_PROTOCOL)
    {
      efi::Status::SUCCESS =>  Ok(*unsafe{Box::from_raw(context_ptr)}),
      err => Err(err)
    }
  }

  pub fn remove_hid_io_context(boot_services: &impl UefiBootServices, controller: efi::Handle) -> Result<UefiHidIo, efi::Status> {
    let mut context_ptr: *mut UefiHidIo<'_> = core::ptr::null_mut();

    match boot_services.open_protocol(
      controller,
      &PRIVATE_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::addr_of_mut!(context_ptr) as *mut *mut c_void,
      core::ptr::null_mut(),
      controller,
    efi::OPEN_PROTOCOL_GET_PROTOCOL)
    {
      efi::Status::SUCCESS =>  (),
      err => return Err(err)
    };


    match boot_services.uninstall_protocol_interface(
      controller,
      &PRIVATE_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      context_ptr as *mut c_void)
    {
      efi::Status::SUCCESS => (),
      err => debugln!(DEBUG_ERROR, "Unexpected status while removing context: {:x?}", err),
    }

    Ok(*unsafe{Box::from_raw(context_ptr)})
  }
}

use alloc::{boxed::Box, vec::Vec};

use hidparser::ReportDescriptor;
use r_efi::efi;
use rust_boot_services::UefiBootServices;


pub trait HidInputHandler {
  fn initialize(&mut self, boot_services: &dyn UefiBootServices, controller: efi::Handle, descriptor: &ReportDescriptor) -> Result<(), efi::Status>;
  fn process_input_report(&mut self, report: &[u8]);
  fn deinitialize(self, boot_services: &dyn UefiBootServices) -> Result<(), efi::Status>;
}

pub struct HidHandlers {
  handlers: Vec<*mut dyn HidInputHandler>
}

pub fn initialize(boot_services: &impl UefiBootServices, controller: efi::Handle, agent: efi::Handle, handlers: Vec<Box<dyn HidInputHandler>>) -> Result<(), efi::Status> {
  let mut uefi_hid_io = uefi_interface::UefiHidIo::new(boot_services, controller, agent)?;

  let report_descriptor_buffer = uefi_hid_io.get_report_descriptor()?;
  let report_descriptor = hidparser::parse_report_descriptor(&report_descriptor_buffer)
    .map_err(|_|efi::Status::DEVICE_ERROR)?;

  //FFI note: handlers are boxed into raw pointers here because they are used to recover context as part of the FFI interfaces.
  let mut hid_handlers = Box::new(HidHandlers {handlers: Vec::new()});

  for handler in handlers {
    hid_handlers.handlers.push(Box::into_raw(handler))
  }

  for handler in &hid_handlers.handlers {
    unsafe {handler.as_mut().expect("bad pointer")}.initialize(boot_services, controller, &report_descriptor)?;
  }

  uefi_hid_io.initiate_reports(report_callback, hid_handlers)?;

  uefi_interface::save_hid_io_context(boot_services, controller, uefi_hid_io)
}

fn report_callback(handlers: &mut HidHandlers, report: &[u8]) {
  for handler in &handlers.handlers {
    unsafe {handler.as_mut().expect("bad pointer")}.process_input_report(report)
  }
}

pub fn destroy(boot_services: &impl UefiBootServices, controller: efi::Handle) -> Result<(), efi::Status> {
  drop(uefi_interface::remove_hid_io_context(boot_services, controller)?);
  Ok(())
}
