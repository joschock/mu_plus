use core::ffi::c_void;

use crate::{
  boot_services::UefiBootServices,
  driver_binding::DriverBinding,
  hid_io::{HidIo, HidIoFactory, HidReportReciever},
};

use alloc::{boxed::Box, vec::Vec};

use r_efi::efi;
#[cfg(not(test))]
use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR};

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
pub trait HidReceiverFactory {
  fn new_hid_receiver_list(&self, controller: efi::Handle) -> Result<Vec<Box<dyn HidReportReciever>>, efi::Status>;
}

// {fb719b29-fda7-4359-ac68-0d46c31a7a7e}
const PRIVATE_HID_CONTEXT_GUID: efi::Guid =
  efi::Guid::from_fields(0xfb719b29, 0xfda7, 0x4359, 0xac, 0x68, &[0x0d, 0x46, 0xc3, 0x1a, 0x7a, 0x7e]);

pub struct DefaultReceiverFactory {}
impl HidReceiverFactory for DefaultReceiverFactory {
  fn new_hid_receiver_list(&self, _controller: efi::Handle) -> Result<Vec<Box<dyn HidReportReciever>>, efi::Status> {
    Ok(Vec::new())
  }
}

pub struct HidFactory {
  hid_io_factory: Box<dyn HidIoFactory>,
  receiver_factory: Box<dyn HidReceiverFactory>,
  agent: efi::Handle,
}

impl HidFactory {
  pub fn new(
    hid_io_factory: Box<dyn HidIoFactory>,
    receiver_factory: Box<dyn HidReceiverFactory>,
    agent: efi::Handle,
  ) -> Self {
    HidFactory { hid_io_factory, receiver_factory, agent }
  }
}

impl DriverBinding for HidFactory {
  fn driver_binding_supported(
    &mut self,
    _boot_services: &'static dyn UefiBootServices,
    controller: r_efi::efi::Handle,
  ) -> Result<(), efi::Status> {
    self.hid_io_factory.new_hid_io(controller, true).map(|_| ())
  }
  fn driver_binding_start(
    &mut self,
    boot_services: &'static dyn UefiBootServices,
    controller: r_efi::efi::Handle,
  ) -> Result<(), efi::Status> {
    let mut hid_io = self.hid_io_factory.new_hid_io(controller, true)?;

    let mut hid_splitter = Box::new(HidSplitter { receivers: Vec::new() });

    for mut receiver in self.receiver_factory.new_hid_receiver_list(controller)? {
      if receiver.initialize(controller, hid_io.as_mut()).is_ok() {
        hid_splitter.receivers.push(receiver);
      }
    }

    if hid_splitter.receivers.len() == 0 {
      return Err(efi::Status::UNSUPPORTED);
    }

    hid_io.set_report_receiver(hid_splitter)?;

    let hid_instance = Box::into_raw(Box::new(HidInstance::new(hid_io)));

    let mut handle = controller;
    let status = boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(handle),
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      hid_instance as *mut c_void,
    );
    if status != efi::Status::SUCCESS {
      drop(unsafe { Box::from_raw(hid_instance) });
      return Err(status);
    }
    Ok(())
  }
  fn driver_binding_stop(
    &mut self,
    boot_services: &'static dyn UefiBootServices,
    controller: r_efi::efi::Handle,
  ) -> Result<(), efi::Status> {
    let mut hid_instance: *mut HidInstance = core::ptr::null_mut();

    let status = boot_services.open_protocol(
      controller,
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::addr_of_mut!(hid_instance) as *mut *mut c_void,
      self.agent,
      controller,
      efi::OPEN_PROTOCOL_GET_PROTOCOL,
    );
    if status != efi::Status::SUCCESS {
      return Err(status);
    }

    let status = boot_services.uninstall_protocol_interface(
      controller,
      &PRIVATE_HID_CONTEXT_GUID as *const efi::Guid as *mut efi::Guid,
      hid_instance as *mut c_void,
    );
    if status != efi::Status::SUCCESS {
      #[cfg(test)]
      panic!("unexpected failure return: {:x?}", status);
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "hid::driver_binding_stop: unexpected failure return: {:x?}", status);
    }

    drop(unsafe { Box::from_raw(hid_instance) });
    Ok(())
  }
}

struct HidInstance {
  _hid_io: Box<dyn HidIo>,
}
impl HidInstance {
  fn new(hid_io: Box<dyn HidIo>) -> Self {
    HidInstance { _hid_io: hid_io }
  }
}

struct HidSplitter {
  receivers: Vec<Box<dyn HidReportReciever>>,
}

impl HidReportReciever for HidSplitter {
  fn initialize(&mut self, _controller: efi::Handle, _hid_io: &dyn HidIo) -> Result<(), efi::Status> {
    panic!("initialize not expected for HidSplitter")
  }
  fn receive_report(&mut self, report: &[u8], hid_io: &dyn HidIo) {
    for receiver in &mut self.receivers {
      receiver.receive_report(report, hid_io)
    }
  }
}

#[cfg(test)]
mod test {
  use core::ffi::c_void;

  use r_efi::efi;

  use crate::{
    boot_services::MockUefiBootServices,
    driver_binding::DriverBinding,
    hid_io::{MockHidIo, MockHidIoFactory, MockHidReportReciever},
  };

  use super::{HidFactory, MockHidReceiverFactory};

  #[test]
  fn driver_binding_supported_should_indicate_support() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_ref().unwrap() };

    {
      let mut hid_io_factory = Box::new(MockHidIoFactory::new());
      //handle 0x3 should return success.
      hid_io_factory
        .expect_new_hid_io()
        .withf_st(|controller, _| *controller == 0x3 as efi::Handle)
        .returning(|_, _| Ok(Box::new(MockHidIo::new())));
      //default for any other handles
      hid_io_factory.expect_new_hid_io().returning(|_, _| Err(efi::Status::UNSUPPORTED));

      let receiver_factory = Box::new(MockHidReceiverFactory::new());
      let agent = 0x1 as efi::Handle;
      let mut hid_factory = HidFactory::new(hid_io_factory, receiver_factory, agent);

      let controller = 0x2 as efi::Handle;
      assert_eq!(hid_factory.driver_binding_supported(boot_services, controller), Err(efi::Status::UNSUPPORTED));

      let controller = 0x3 as efi::Handle;
      assert!(hid_factory.driver_binding_supported(boot_services, controller).is_ok());
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn driver_binding_start_should_not_start_when_not_supported() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_ref().unwrap() };
    {
      let mut hid_io_factory = Box::new(MockHidIoFactory::new());
      hid_io_factory
        .expect_new_hid_io()
        .withf_st(|controller, _| *controller == 0x3 as efi::Handle)
        .returning(|_, _| Ok(Box::new(MockHidIo::new())));
      hid_io_factory
        .expect_new_hid_io()
        .withf_st(|controller, _| *controller == 0x4 as efi::Handle)
        .returning(|_, _| Ok(Box::new(MockHidIo::new())));
      //default for any other handles
      hid_io_factory.expect_new_hid_io().returning(|_, _| Err(efi::Status::UNSUPPORTED));

      let mut receiver_factory = Box::new(MockHidReceiverFactory::new());
      receiver_factory
        .expect_new_hid_receiver_list()
        .withf_st(|controller| *controller == 0x4 as efi::Handle)
        .returning(|_| Ok(Vec::new()));
      receiver_factory.expect_new_hid_receiver_list().returning(|_| Err(efi::Status::UNSUPPORTED));

      let agent = 0x1 as efi::Handle;
      let mut hid_factory = HidFactory::new(hid_io_factory, receiver_factory, agent);

      // test: no hid_io on the handle.
      let controller = 0x02 as efi::Handle;
      assert_eq!(hid_factory.driver_binding_start(boot_services, controller), Err(efi::Status::UNSUPPORTED));

      // test: hid_io present, but failed to retrive receivers.
      let controller = 0x03 as efi::Handle;
      assert_eq!(hid_factory.driver_binding_start(boot_services, controller), Err(efi::Status::UNSUPPORTED));

      // test: hid_io present, empty receiver list.
      let controller = 0x04 as efi::Handle;
      assert_eq!(hid_factory.driver_binding_start(boot_services, controller), Err(efi::Status::UNSUPPORTED));

      let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
      boot_services.checkpoint();

      //test: hid_io present, receiver present, receiver init indicates no support.
      let mut hid_io_factory = Box::new(MockHidIoFactory::new());
      hid_io_factory.expect_new_hid_io().returning(|_, _| Ok(Box::new(MockHidIo::new())));

      let mut receiver_factory = Box::new(MockHidReceiverFactory::new());
      receiver_factory.expect_new_hid_receiver_list().returning(|_| {
        let mut hid_receiver = MockHidReportReciever::new();
        hid_receiver.expect_initialize().returning(|_, _| Err(efi::Status::UNSUPPORTED));
        Ok(vec![Box::new(hid_receiver)])
      });

      let mut hid_factory = HidFactory::new(hid_io_factory, receiver_factory, agent);
      let controller = 0x02 as efi::Handle;
      assert_eq!(hid_factory.driver_binding_start(boot_services, controller), Err(efi::Status::UNSUPPORTED));

      let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };
      boot_services.checkpoint();
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn driver_binding_start_should_start_when_supported() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      let agent = 0x1 as efi::Handle;

      let mut hid_io_factory = Box::new(MockHidIoFactory::new());
      hid_io_factory.expect_new_hid_io().returning(|_, _| {
        let mut hid_io = MockHidIo::new();
        hid_io.expect_set_report_receiver().returning(|_| Ok(()));
        Ok(Box::new(hid_io))
      });

      let mut receiver_factory = Box::new(MockHidReceiverFactory::new());
      receiver_factory.expect_new_hid_receiver_list().returning(|_| {
        let mut hid_receiver = MockHidReportReciever::new();
        hid_receiver.expect_initialize().returning(|_, _| Ok(()));
        Ok(vec![Box::new(hid_receiver)])
      });

      boot_services.expect_install_protocol_interface().returning(|_, _, _, _| efi::Status::SUCCESS);

      let mut hid_factory = HidFactory::new(hid_io_factory, receiver_factory, agent);
      let controller = 0x02 as efi::Handle;
      hid_factory.driver_binding_start(boot_services, controller).unwrap();

      //test note: this will leak a HidInstance.
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn driver_binding_start_should_stop_after_start() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      let agent = 0x1 as efi::Handle;

      let mut hid_io_factory = Box::new(MockHidIoFactory::new());
      hid_io_factory.expect_new_hid_io().returning(|_, _| {
        let mut hid_io = MockHidIo::new();
        hid_io.expect_set_report_receiver().returning(|_| Ok(()));
        Ok(Box::new(hid_io))
      });

      let mut receiver_factory = Box::new(MockHidReceiverFactory::new());
      receiver_factory.expect_new_hid_receiver_list().returning(|_| {
        let mut hid_receiver = MockHidReportReciever::new();
        hid_receiver.expect_initialize().returning(|_, _| Ok(()));
        Ok(vec![Box::new(hid_receiver)])
      });

      static mut HID_INSTANCE_PTR: *mut c_void = core::ptr::null_mut();
      boot_services.expect_install_protocol_interface().returning(|_, _, _, instance| {
        unsafe { HID_INSTANCE_PTR = instance };
        efi::Status::SUCCESS
      });

      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = HID_INSTANCE_PTR };
        efi::Status::SUCCESS
      });

      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);

      let mut hid_factory = HidFactory::new(hid_io_factory, receiver_factory, agent);
      let controller = 0x02 as efi::Handle;
      hid_factory.driver_binding_start(boot_services, controller).unwrap();

      assert_ne!(unsafe { HID_INSTANCE_PTR }, core::ptr::null_mut());

      hid_factory.driver_binding_stop(boot_services, controller).unwrap();
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }
}
