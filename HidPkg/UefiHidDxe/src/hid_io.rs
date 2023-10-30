use core::ffi::c_void;

use hidparser::ReportDescriptor;
use r_efi::efi;
#[cfg(not(test))]
use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR};

use crate::boot_services::UefiBootServices;

pub trait HidReportReciever {
  fn receive_report(&mut self, report: &[u8]);
}

pub trait HidIo {
  fn get_report_descriptor(&self) -> Result<ReportDescriptor, efi::Status>;
  fn get_input_report(&self, id: Option<u8>) -> Result<&[u8], efi::Status>;
  fn set_ouput_report(&self, id: Option<u8>, report: &[u8]) -> Result<(), efi::Status>;
  fn set_report_receiver(&self, receiver: &dyn HidReportReciever) -> Result<(), efi::Status>;
}

pub struct UefiHidIo {
  hid_io: &'static mut hid_io::protocol::Protocol,
  boot_services: &'static dyn UefiBootServices,
  controller: efi::Handle,
  agent: efi::Handle
}

impl UefiHidIo {
  pub fn new(boot_services: &'static dyn UefiBootServices, controller: efi::Handle, agent: efi::Handle) -> Result<Self, efi::Status> {
    let mut hid_io_ptr: *mut hid_io::protocol::Protocol = core::ptr::null_mut();

    let status = boot_services.open_protocol(
      controller,
      &hid_io::protocol::GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::addr_of_mut!(hid_io_ptr) as *mut *mut c_void,
      agent,
      controller,
      efi::OPEN_PROTOCOL_BY_DRIVER,
    );
    if status.is_error() {
      return Err(status);
    }

    Ok(Self{
      hid_io: unsafe {hid_io_ptr.as_mut().expect("bad hid_io ptr")},
      boot_services,
      controller,
      agent
    })
  }
}

impl Drop for UefiHidIo {
  fn drop(&mut self) {
    let status = self.boot_services.close_protocol(
      self.controller,
      &hid_io::protocol::GUID as *const efi::Guid as *mut efi::Guid,
      self.agent,
      self.controller);
    #[cfg(not(test))]
    if status.is_error() {
      debugln!(DEBUG_ERROR, "Unexpected error closing hid_io: {:x?}", status);
    }
  }
}

impl HidIo for UefiHidIo {
  fn get_report_descriptor(&self) -> Result<ReportDescriptor, efi::Status> {
    let mut report_descriptor_size: usize = 0;
    match (self.hid_io.get_report_descriptor)(
      self.hid_io,
      core::ptr::addr_of_mut!(report_descriptor_size),
      core::ptr::null_mut()
    ) {
      efi::Status::BUFFER_TOO_SMALL => (),
      efi::Status::SUCCESS => return Err(efi::Status::DEVICE_ERROR),
      err => return Err(err)
    }

    let mut report_descriptor_buffer = vec![0u8; report_descriptor_size];
    let report_descriptor_buffer_ptr = report_descriptor_buffer.as_mut_ptr();

    match (self.hid_io.get_report_descriptor)(
      self.hid_io,
      core::ptr::addr_of_mut!(report_descriptor_size),
      report_descriptor_buffer_ptr as *mut c_void
    ) {
      efi::Status::SUCCESS => (),
      err => return Err(err)
    }

    hidparser::parse_report_descriptor(&report_descriptor_buffer)
      .map_err(|_|efi::Status::DEVICE_ERROR)
  }
  fn get_input_report(&self, id: Option<u8>) -> Result<&[u8], efi::Status> {
    todo!()
  }
  fn set_ouput_report(&self, id: Option<u8>, report: &[u8]) -> Result<(), efi::Status> {
    todo!()
  }
  fn set_report_receiver(&self, receiver: &dyn HidReportReciever) -> Result<(), efi::Status> {
    todo!()
  }
}

#[cfg(test)]
mod test {
  use core::{ffi::c_void, slice::from_raw_parts_mut};

  use super::{UefiHidIo, HidIo};

  use crate::boot_services::MockUefiBootServices;

  use r_efi::efi;

  static MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // USAGE_PAGE (Generic Desktop)
    0x09, 0x06, // USAGE (Keyboard)
    0xa1, 0x01, // COLLECTION (Application)
    0x75, 0x01, //    REPORT_SIZE (1)
    0x95, 0x08, //    REPORT_COUNT (8)
    0x05, 0x07, //    USAGE_PAGE (Key Codes)
    0x19, 0xE0, //    USAGE_MINIMUM (224)
    0x29, 0xE7, //    USAGE_MAXIMUM (231)
    0x15, 0x00, //    LOGICAL_MINIMUM (0)
    0x25, 0x01, //    LOGICAL_MAXIMUM (1)
    0x81, 0x02, //    INPUT (Data, Var, Abs) (Modifier Byte)
    0xc0, // END_COLLECTION
  ];

  fn mock_hid_io() -> hid_io::protocol::Protocol {

    extern "efiapi" fn mock_get_report_descriptor (
      _this: *const hid_io::protocol::Protocol,
      report_descriptor_size: *mut usize,
      report_descriptor_buffer: *mut c_void,
    ) -> efi::Status {
      unsafe {
        if *report_descriptor_size < MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR.len() {
          *report_descriptor_size = MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR.len();
          return efi::Status::BUFFER_TOO_SMALL;
        } else {
          *report_descriptor_size = MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR.len();
          let slice = from_raw_parts_mut(report_descriptor_buffer as *mut u8, *report_descriptor_size);
          slice.copy_from_slice(MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR);
          return efi::Status::SUCCESS
        }
      }
    }
    extern "efiapi" fn mock_get_report (
      _this: *const hid_io::protocol::Protocol,
      _report_id: u8,
      _report_type: hid_io::protocol::HidReportType,
      _report_buffer_size: usize,
      _report_buffer: *mut c_void,
    ) -> efi::Status {
      efi::Status::UNSUPPORTED
    }
    extern "efiapi" fn mock_set_report (
      _this: *const hid_io::protocol::Protocol,
      _report_id: u8,
      _report_type: hid_io::protocol::HidReportType,
      _report_buffer_size: usize,
      _report_buffer: *mut c_void,
    ) -> efi::Status {
      efi::Status::UNSUPPORTED
    }
    extern "efiapi" fn mock_register_report_callback(
      _this: *const hid_io::protocol::Protocol,
      _callback: hid_io::protocol::HidIoReportCallback,
      _context: *mut c_void
    ) -> efi::Status {
      efi::Status::UNSUPPORTED
    }
    extern "efiapi" fn mock_unregister_report_callback(
      _this: *const hid_io::protocol::Protocol,
      _callback: hid_io::protocol::HidIoReportCallback
    ) -> efi::Status {
      efi::Status::UNSUPPORTED
    }

    hid_io::protocol::Protocol {
      get_report_descriptor: mock_get_report_descriptor,
      get_report: mock_get_report,
      set_report: mock_set_report,
      register_report_callback: mock_register_report_callback,
      unregister_report_callback: mock_unregister_report_callback,
    }
  }

  #[test]
  fn new_should_instantiate_new_uefi_hid_io() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
    let controller: efi::Handle = 0x1234 as efi::Handle;
    let agent: efi::Handle = 0x4321 as efi::Handle;

    boot_services.expect_open_protocol()
      .returning(|handle, protocol, interface, agent, controller, attributes|
        {
          assert_eq!(handle, 0x1234 as efi::Handle);
          assert_eq!(unsafe {*protocol}, hid_io::protocol::GUID);
          assert_ne!(interface, core::ptr::null_mut());
          assert_eq!(agent, 0x4321 as efi::Handle);
          assert_eq!(controller, 0x1234 as efi::Handle);
          assert_eq!(attributes, efi::OPEN_PROTOCOL_BY_DRIVER);

          unsafe {*interface = 0x1234 as *mut c_void};
          efi::Status::SUCCESS
        });

    boot_services.expect_close_protocol()
      .returning(|handle, protocol, agent, controller|
        {
          assert_eq!(handle, 0x1234 as efi::Handle);
          assert_eq!(unsafe {*protocol}, hid_io::protocol::GUID);
          assert_eq!(agent, 0x4321 as efi::Handle);
          assert_eq!(controller, 0x1234 as efi::Handle);
          efi::Status::SUCCESS
        });

    let uefi_hid_io = UefiHidIo::new(boot_services, controller, agent).unwrap();
    drop(uefi_hid_io);

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn get_report_descriptor_should_return_report_descriptor() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
    let controller: efi::Handle = 0x1234 as efi::Handle;
    let agent: efi::Handle = 0x4321 as efi::Handle;

    boot_services.expect_open_protocol()
      .returning(|_, _, interface, _, _, _|
        {
          let hid_io = mock_hid_io();
          unsafe {*interface = Box::into_raw(Box::new(hid_io)) as *mut c_void};
          efi::Status::SUCCESS
        });

    boot_services.expect_close_protocol()
      .returning(|handle, protocol, agent, controller|
        {
          efi::Status::SUCCESS
        });

    let uefi_hid_io = UefiHidIo::new(boot_services, controller, agent).unwrap();
    let _descriptor = uefi_hid_io.get_report_descriptor().unwrap();

  }

}
