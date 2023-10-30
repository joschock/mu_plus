use core::{ffi::c_void, fmt::Debug, sync::atomic::AtomicPtr};
#[cfg(test)]
use mockall::automock;
use r_efi::efi;

#[cfg_attr(test, automock)]
pub trait UefiBootServices {
  fn create_event(
    &self,
    r#type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *mut c_void,
    event: *mut efi::Event,
  ) -> efi::Status;

  fn create_event_ex(
    &self,
    r#type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *const c_void,
    event_group: *const efi::Guid,
    event: *mut efi::Event,
  ) -> efi::Status;

  fn close_event(&self, event: efi::Event) -> efi::Status;

  fn signal_event(&self, event: efi::Event) -> efi::Status;

  fn raise_tpl(&self, new_tpl: efi::Tpl) -> efi::Tpl;

  fn restore_tpl(&self, old_tpl: efi::Tpl);

  fn install_protocol_interface(
    &self,
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    interface_type: efi::InterfaceType,
    interface: *mut c_void,
  ) -> efi::Status;

  fn uninstall_protocol_interface(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
  ) -> efi::Status;

  fn open_protocol(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
    attributes: u32,
  ) -> efi::Status;

  fn close_protocol(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
  ) -> efi::Status;

  fn locate_protocol(
    &self,
    protocol: *mut efi::Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
  ) -> efi::Status;
}

#[derive(Debug)]
pub struct StandardUefiBootServices {
  boot_services: AtomicPtr<efi::BootServices>,
}

impl StandardUefiBootServices {
  pub const fn new() -> Self {
    Self { boot_services: AtomicPtr::new(core::ptr::null_mut()) }
  }
  pub fn initialize(&self, boot_services: *mut efi::BootServices) {
    self.boot_services.store(boot_services, core::sync::atomic::Ordering::SeqCst)
  }

  fn boot_services(&self) -> &efi::BootServices {
    let boot_services_ptr = self.boot_services.load(core::sync::atomic::Ordering::SeqCst);
    unsafe { boot_services_ptr.as_ref().expect("invalid boot_services pointer") }
  }
}

unsafe impl Sync for StandardUefiBootServices {}
unsafe impl Send for StandardUefiBootServices {}

impl UefiBootServices for StandardUefiBootServices {
  fn create_event(
    &self,
    r#type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *mut c_void,
    event: *mut efi::Event,
  ) -> efi::Status {
    (self.boot_services().create_event)(r#type, notify_tpl, notify_function, notify_context, event)
  }
  fn create_event_ex(
    &self,
    r#type: u32,
    notify_tpl: efi::Tpl,
    notify_function: Option<efi::EventNotify>,
    notify_context: *const c_void,
    event_group: *const efi::Guid,
    event: *mut efi::Event,
  ) -> efi::Status {
    (self.boot_services().create_event_ex)(r#type, notify_tpl, notify_function, notify_context, event_group, event)
  }
  fn close_event(&self, event: efi::Event) -> efi::Status {
    (self.boot_services().close_event)(event)
  }
  fn signal_event(&self, event: efi::Event) -> efi::Status {
    (self.boot_services().signal_event)(event)
  }
  fn raise_tpl(&self, new_tpl: efi::Tpl) -> efi::Tpl {
    (self.boot_services().raise_tpl)(new_tpl)
  }
  fn restore_tpl(&self, old_tpl: efi::Tpl) {
    (self.boot_services().restore_tpl)(old_tpl)
  }
  fn install_protocol_interface(
    &self,
    handle: *mut efi::Handle,
    protocol: *mut efi::Guid,
    interface_type: efi::InterfaceType,
    interface: *mut c_void,
  ) -> efi::Status {
    (self.boot_services().install_protocol_interface)(handle, protocol, interface_type, interface)
  }
  fn uninstall_protocol_interface(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut c_void,
  ) -> efi::Status {
    (self.boot_services().uninstall_protocol_interface)(handle, protocol, interface)
  }
  fn open_protocol(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    interface: *mut *mut c_void,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
    attributes: u32,
  ) -> efi::Status {
    (self.boot_services().open_protocol)(handle, protocol, interface, agent_handle, controller_handle, attributes)
  }
  fn close_protocol(
    &self,
    handle: efi::Handle,
    protocol: *mut efi::Guid,
    agent_handle: efi::Handle,
    controller_handle: efi::Handle,
  ) -> efi::Status {
    (self.boot_services().close_protocol)(handle, protocol, agent_handle, controller_handle)
  }
  fn locate_protocol(
    &self,
    protocol: *mut efi::Guid,
    registration: *mut c_void,
    interface: *mut *mut c_void,
  ) -> efi::Status {
    (self.boot_services().locate_protocol)(protocol, registration, interface)
  }
}
