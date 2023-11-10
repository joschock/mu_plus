use core::ffi::c_void;

use alloc::{
  boxed::Box,
  collections::{BTreeMap, BTreeSet},
  vec,
  vec::Vec,
};
use hidparser::{
  report_data_types::{ReportId, Usage},
  ArrayField, ReportDescriptor, ReportField, VariableField,
};
use r_efi::{efi, hii, protocols};

use crate::{
  boot_services::UefiBootServices,
  hid_io::{HidIo, HidIoFactory, HidReportReciever, UefiHidIoFactory},
  key_queue::{self, OrdKeyData},
};

#[cfg(not(test))]
use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR, DEBUG_WARN};

// usages supported by this module
const KEYBOARD_MODIFIER_USAGE_MIN: u32 = 0x000700E0;
const KEYBOARD_MODIFIER_USAGE_MAX: u32 = 0x000700E7;
const KEYBOARD_USAGE_MIN: u32 = 0x00070001;
const KEYBOARD_USAGE_MAX: u32 = 0x00070065;
const LED_USAGE_MIN: u32 = 0x00080001;
const LED_USAGE_MAX: u32 = 0x00080005;

// maps a given field to a routine that handles input from it.
#[derive(Debug, Clone)]
struct ReportFieldWithHandler<T> {
  field: T,
  report_handler: fn(handler: &mut KeyboardHidHandler, field: T, report: &[u8]),
}

// maps a given field to a routine that builds output reports from it.
#[derive(Debug, Clone)]
struct ReportFieldBuilder<T> {
  field: T,
  field_builder: fn(&mut KeyboardHidHandler, field: T, report: &mut [u8]),
}

// Defines an input report and the fields of interest in it.
#[derive(Debug, Default, Clone)]
struct KeyboardReportData {
  report_id: Option<ReportId>,
  report_size: usize,
  relevant_variable_fields: Vec<ReportFieldWithHandler<VariableField>>,
  relevant_array_fields: Vec<ReportFieldWithHandler<ArrayField>>,
}

// Defines an output report and the fields of interest in it.
#[derive(Debug, Default, Clone)]
struct KeyboardOutputReportBuilder {
  report_id: Option<ReportId>,
  report_size: usize,
  relevant_variable_fields: Vec<ReportFieldBuilder<VariableField>>,
}

// FFI contexts
// Safety: a pointer to KeyboardHidHandler is included in the contexts so that it can be reclaimed in the simple_text_in
// API implementations. Care must be taken to ensure that rust invariants are respected when accessing the
// KeyboardHidHandler. In particular, the design must ensure mutual exclusion on the KeyboardHidHandler between
// callbacks running at different TPL; this is accomplished by ensuring all access to the structure is at TPL_NOTIFY
// once initialization is complete - for this reason the context structure includes a direct reference to boot_services
// so that TPL can be enforced without access to the *mut KeyboardHidHandler.
//
// In addition, the simple_text_in/simple_text_in_ex protocol element needs to be the first element in the structure so
// that the full structure can be recovered by simple casting for simple_text_in FFI interfaces that only receive a
// pointer to the simple_text_in protocol structure.
#[repr(C)]
struct SimpleTextInContext {
  simple_text_in: protocols::simple_text_input::Protocol,
  boot_services: &'static dyn UefiBootServices,
  keyboard_handler: *mut KeyboardHidHandler,
}

#[repr(C)]
struct SimpleTextInExContext {
  simple_text_in_ex: protocols::simple_text_input_ex::Protocol,
  boot_services: &'static dyn UefiBootServices,
  keyboard_handler: *mut KeyboardHidHandler,
}

#[repr(C)]
struct LayoutChangeContext {
  boot_services: &'static dyn UefiBootServices,
  keyboard_handler: *mut KeyboardHidHandler,
}

pub struct KeyboardHidHandler {
  boot_services: &'static dyn UefiBootServices,
  agent: efi::Handle,
  controller: Option<efi::Handle>,
  input_reports: BTreeMap<Option<ReportId>, KeyboardReportData>,
  output_builders: Vec<KeyboardOutputReportBuilder>,
  report_id_present: bool,
  last_keys: BTreeSet<Usage>,
  current_keys: BTreeSet<Usage>,
  led_state: BTreeSet<Usage>,
  key_queue: key_queue::KeyQueue,
  notification_callbacks: BTreeMap<usize, (OrdKeyData, protocols::simple_text_input_ex::KeyNotifyFunction)>,
  next_notify_handle: usize,
  key_notify_event: efi::Event,
  layout_change_event: efi::Event,
  layout_context: *mut LayoutChangeContext,
}

impl KeyboardHidHandler {
  pub fn new(boot_services: &'static dyn UefiBootServices, agent: efi::Handle) -> Self {
    Self {
      boot_services,
      agent,
      controller: None,
      input_reports: BTreeMap::new(),
      output_builders: Vec::new(),
      report_id_present: false,
      last_keys: BTreeSet::new(),
      current_keys: BTreeSet::new(),
      led_state: BTreeSet::new(),
      key_queue: Default::default(),
      notification_callbacks: BTreeMap::new(),
      next_notify_handle: 0,
      key_notify_event: core::ptr::null_mut(),
      layout_change_event: core::ptr::null_mut(),
      layout_context: core::ptr::null_mut(),
    }
  }
  fn process_descriptor(&mut self, descriptor: ReportDescriptor) -> Result<(), efi::Status> {
    let multiple_reports =
      descriptor.input_reports.len() > 1 || descriptor.output_reports.len() > 1 || descriptor.features.len() > 1;

    for report in &descriptor.input_reports {
      let mut report_data = KeyboardReportData { report_id: report.report_id, ..Default::default() };

      self.report_id_present = report.report_id.is_some();

      if multiple_reports && !self.report_id_present {
        //Invalid to have None ReportId if multiple reports present.
        Err(efi::Status::DEVICE_ERROR)?;
      }

      report_data.report_size = report.size_in_bits.div_ceil(8);

      for field in &report.fields {
        match field {
          //Variable fields (typically used for modifier Usages)
          ReportField::Variable(field) => {
            match field.usage.into() {
              KEYBOARD_MODIFIER_USAGE_MIN..=KEYBOARD_MODIFIER_USAGE_MAX => {
                report_data.relevant_variable_fields.push(ReportFieldWithHandler::<VariableField> {
                  field: field.clone(),
                  report_handler: Self::handle_variable_key,
                })
              }
              _ => (), // other usages irrelevant.
            }
          }
          //Array fields (typically used for key strokes)
          ReportField::Array(field) => {
            for usage_list in &field.usage_list {
              if usage_list.contains(Usage::from(KEYBOARD_USAGE_MIN))
                || usage_list.contains(Usage::from(KEYBOARD_USAGE_MAX))
              {
                report_data.relevant_array_fields.push(ReportFieldWithHandler::<ArrayField> {
                  field: field.clone(),
                  report_handler: Self::handle_array_key,
                });
                break;
              }
            }
          }
          ReportField::Padding(_) => (), // padding irrelevant.
        }
      }
      if report_data.relevant_variable_fields.len() > 0 || report_data.relevant_array_fields.len() > 0 {
        self.input_reports.insert(report_data.report_id, report_data);
      }
    }

    for report in &descriptor.output_reports {
      let mut report_builder = KeyboardOutputReportBuilder { report_id: report.report_id, ..Default::default() };

      self.report_id_present = report.report_id.is_some();

      if multiple_reports && !self.report_id_present {
        //invalid to have None ReportId if multiple reports present.
        Err(efi::Status::DEVICE_ERROR)?;
      }

      report_builder.report_size = report.size_in_bits / 8;
      if (report.size_in_bits % 8) != 0 {
        report_builder.report_size += 1;
      }

      for field in &report.fields {
        match field {
        //Variable fields in output reports (typically used for LEDs).
        ReportField::Variable(field) => {
          match field.usage.into() {
            LED_USAGE_MIN..=LED_USAGE_MAX => {
              report_builder.relevant_variable_fields.push(
                ReportFieldBuilder {
                  field: field.clone(),
                  field_builder: Self::build_led_report
                }
              )
            },
            _=> (), //other usages irrelevant.
          }
        },
        ReportField::Array(_) | // No support for array field report outputs; could be added if required.
        ReportField::Padding(_) => (), // padding fields irrelevant.
      }
      }
      if report_builder.relevant_variable_fields.len() > 0 {
        self.output_builders.push(report_builder);
      }
    }

    if self.input_reports.len() > 0 || self.output_builders.len() > 0 {
      Ok(())
    } else {
      Err(efi::Status::UNSUPPORTED)
    }
  }

  // helper routine to handle variable keyboard input report fields
  fn handle_variable_key(&mut self, field: VariableField, report: &[u8]) {
    match field.field_value(report) {
      Some(x) if x != 0 => {
        self.current_keys.insert(field.usage);
      }
      None | Some(_) => (),
    }
  }

  // helper routine to handle array keyboard input report fields
  fn handle_array_key(&mut self, field: ArrayField, report: &[u8]) {
    match field.field_value(report) {
      Some(index) if index != 0 => {
        let mut index = (index as u32 - u32::from(field.logical_minimum)) as usize;
        let usage = field.usage_list.iter().find_map(|x| {
          let range_size = (x.end() - x.start()) as usize;
          if index <= range_size {
            x.range().nth(index)
          } else {
            index = index - range_size as usize;
            None
          }
        });
        if let Some(usage) = usage {
          self.current_keys.insert(Usage::from(usage));
        }
      }
      None | Some(_) => (),
    }
  }

  //helper routine that updates the fields in the given report buffer for the given field (called for each field for
  //every LED usage that was discovered in the output report descriptor).
  fn build_led_report(&mut self, field: VariableField, report: &mut [u8]) {
    let status = field.set_field_value(self.led_state.contains(&field.usage).into(), report);
    if status.is_err() {
      #[cfg(not(test))]
      debugln!(DEBUG_WARN, "keyobard::build_led_report: failed to set field value: {:?}", status);
    }
  }

  fn generate_led_output_reports(&mut self) -> Vec<(Option<ReportId>, Vec<u8>)> {
    let mut output_vec = Vec::new();
    let current_leds: BTreeSet<Usage> = self.key_queue.get_active_leds().iter().cloned().collect();
    if current_leds != self.led_state {
      self.led_state = current_leds;
      for output_builder in self.output_builders.clone() {
        let mut report_buffer = vec![0u8; output_builder.report_size];
        for field_builder in &output_builder.relevant_variable_fields {
          (field_builder.field_builder)(self, field_builder.field.clone(), report_buffer.as_mut_slice());
        }
        output_vec.push((output_builder.report_id, report_buffer));
      }
    }
    output_vec
  }

  fn install_simple_text_in(&mut self, controller: efi::Handle) -> Result<(), efi::Status> {
    //Create simple_text_in context
    let simple_text_in_ctx = SimpleTextInContext {
      simple_text_in: protocols::simple_text_input::Protocol {
        reset: simple_text_in_reset,
        read_key_stroke: simple_text_in_read_key_stroke,
        wait_for_key: core::ptr::null_mut(),
      },
      boot_services: self.boot_services,
      keyboard_handler: self as *mut KeyboardHidHandler,
    };

    let simple_text_in_ptr = Box::into_raw(Box::new(simple_text_in_ctx));

    //create event for wait_for_key
    let mut wait_for_key_event: efi::Event = core::ptr::null_mut();
    let status = self.boot_services.create_event(
      efi::EVT_NOTIFY_WAIT,
      efi::TPL_NOTIFY,
      Some(simple_text_in_wait_for_key),
      simple_text_in_ptr as *mut c_void,
      core::ptr::addr_of_mut!(wait_for_key_event),
    );
    if status.is_error() {
      drop(unsafe { Box::from_raw(simple_text_in_ptr) });
      return Err(status);
    }

    unsafe { (*simple_text_in_ptr).simple_text_in.wait_for_key = wait_for_key_event };

    //install the simple_text_in protocol
    let mut controller = controller;
    let status = self.boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(controller),
      &protocols::simple_text_input::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      simple_text_in_ptr as *mut c_void,
    );

    if status.is_error() {
      let _ = self.boot_services.close_event(wait_for_key_event);
      drop(unsafe { Box::from_raw(simple_text_in_ptr) });
      return Err(status);
    }

    self.controller = Some(controller);
    Ok(())
  }

  fn uninstall_simple_text_in(&mut self) -> Result<(), efi::Status> {
    if let Some(controller) = self.controller {
      //Controller is set - that means initialize() was called, and there is potential state exposed thru FFI that needs
      //to be cleaned up.
      let mut simple_text_in_ptr: *mut SimpleTextInContext = core::ptr::null_mut();
      let status = self.boot_services.open_protocol(
        controller,
        &protocols::simple_text_input::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        core::ptr::addr_of_mut!(simple_text_in_ptr) as *mut *mut c_void,
        self.agent,
        controller,
        efi::OPEN_PROTOCOL_GET_PROTOCOL,
      );
      if status.is_error() {
        //No protocol is actually installed on this controller, so nothing to clean up.
        return Ok(());
      }

      //Attempt to uninstall the simple_text_in interface - this should disconnect any drivers using it and release
      //the interface.
      let status = self.boot_services.uninstall_protocol_interface(
        controller,
        &protocols::simple_text_input::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        simple_text_in_ptr as *mut c_void,
      );
      if status.is_error() {
        //An error here means some other driver might be holding on to the simple_text_in_ptr.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on absolute_pointer_ptr to continue to resolve
        //and return error based on observing keyboard_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to uninstall simple_text_in interface, status: {:x?}", status);

        unsafe {
          (*simple_text_in_ptr).keyboard_handler = core::ptr::null_mut();
        }
        //return without tearing down the context.
        return Err(status);
      }

      let wait_for_key_event: efi::Handle = unsafe { (*simple_text_in_ptr).simple_text_in.wait_for_key };
      let status = self.boot_services.close_event(wait_for_key_event);
      if status.is_error() {
        //An error here means the event was not closed, so in theory the notification_callback on it could still be
        //fired.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on simple_text_in_ptr to continue to resolve
        //and return error based on observing keyboard_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to close simple_text_in_ptr.wait_for_key event, status: {:x?}", status);
        unsafe {
          (*simple_text_in_ptr).keyboard_handler = core::ptr::null_mut();
        }
        return Err(status);
      }
      // None of the parts of simple_text_in_ptr simple_text_in_ptr are in use, so it is safe to reclaim it.
      drop(unsafe { Box::from_raw(simple_text_in_ptr) });
    }
    Ok(())
  }

  fn install_simple_text_in_ex(&mut self, controller: efi::Handle) -> Result<(), efi::Status> {
    //Create simple_text_in context
    let simple_text_in_ex_ctx = SimpleTextInExContext {
      simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
        reset: simple_text_in_ex_reset,
        read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
        wait_for_key_ex: core::ptr::null_mut(),
        set_state: simple_text_in_ex_set_state,
        register_key_notify: simple_text_in_ex_register_key_notify,
        unregister_key_notify: simple_text_in_ex_unregister_key_notify,
      },
      boot_services: self.boot_services,
      keyboard_handler: self as *mut KeyboardHidHandler,
    };

    let simple_text_in_ex_ptr = Box::into_raw(Box::new(simple_text_in_ex_ctx));

    //create event for wait_for_key
    let mut wait_for_key_event: efi::Event = core::ptr::null_mut();
    let status = self.boot_services.create_event(
      efi::EVT_NOTIFY_WAIT,
      efi::TPL_NOTIFY,
      Some(simple_text_in_ex_wait_for_key),
      simple_text_in_ex_ptr as *mut c_void,
      core::ptr::addr_of_mut!(wait_for_key_event),
    );
    if status.is_error() {
      drop(unsafe { Box::from_raw(simple_text_in_ex_ptr) });
      return Err(status);
    }

    unsafe { (*simple_text_in_ex_ptr).simple_text_in_ex.wait_for_key_ex = wait_for_key_event };

    //Key notifies are required to dispatch at TPL_CALLBACK per UEFI spec 2.10 section 12.2.5. The keyboard handler
    //interfaces run at TPL_NOTIFY and issue a boot_sevices.signal_event() on this event to pend key notifies to be
    //serviced at TPL_CALLBACK.
    let mut key_notify_event: efi::Event = core::ptr::null_mut();
    let status = self.boot_services.create_event(
      efi::EVT_NOTIFY_SIGNAL,
      efi::TPL_CALLBACK,
      Some(process_key_notifies),
      simple_text_in_ex_ptr as *mut c_void,
      core::ptr::addr_of_mut!(key_notify_event),
    );
    if status.is_error() {
      let _ = self.boot_services.close_event(wait_for_key_event);
      drop(unsafe { Box::from_raw(simple_text_in_ex_ptr) });
    }

    self.key_notify_event = key_notify_event;

    //install the simple_text_in_ex protocol
    let mut controller = controller;
    let status = self.boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(controller),
      &protocols::simple_text_input_ex::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      simple_text_in_ex_ptr as *mut c_void,
    );

    if status.is_error() {
      let _ = self.boot_services.close_event(wait_for_key_event);
      let _ = self.boot_services.close_event(key_notify_event);
      drop(unsafe { Box::from_raw(simple_text_in_ex_ptr) });
      return Err(status);
    }

    self.controller = Some(controller);
    Ok(())
  }

  fn uninstall_simple_text_in_ex(&mut self) -> Result<(), efi::Status> {
    if let Some(controller) = self.controller {
      //Controller is set - that means initialize() was called, and there is potential state exposed thru FFI that needs
      //to be cleaned up.
      let mut simple_text_in_ex_ptr: *mut SimpleTextInExContext = core::ptr::null_mut();
      let status = self.boot_services.open_protocol(
        controller,
        &protocols::simple_text_input_ex::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        core::ptr::addr_of_mut!(simple_text_in_ex_ptr) as *mut *mut c_void,
        self.agent,
        controller,
        efi::OPEN_PROTOCOL_GET_PROTOCOL,
      );
      if status.is_error() {
        //No protocol is actually installed on this controller, so nothing to clean up.
        return Ok(());
      }

      //Attempt to uninstall the simple_text_in interface - this should disconnect any drivers using it and release
      //the interface.
      let status = self.boot_services.uninstall_protocol_interface(
        controller,
        &protocols::simple_text_input_ex::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        simple_text_in_ex_ptr as *mut c_void,
      );
      if status.is_error() {
        //An error here means some other driver might be holding on to the simple_text_in_ptr.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on absolute_pointer_ptr to continue to resolve
        //and return error based on observing keyboard_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to uninstall simple_text_in interface, status: {:x?}", status);

        unsafe {
          (*simple_text_in_ex_ptr).keyboard_handler = core::ptr::null_mut();
        }
        //return without tearing down the context.
        return Err(status);
      }

      let wait_for_key_event: efi::Handle = unsafe { (*simple_text_in_ex_ptr).simple_text_in_ex.wait_for_key_ex };
      let status = self.boot_services.close_event(wait_for_key_event);
      if status.is_error() {
        //An error here means the event was not closed, so in theory the notification_callback on it could still be
        //fired.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on simple_text_in_ptr to continue to resolve
        //and return error based on observing keyboard_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to close simple_text_in_ptr.wait_for_key event, status: {:x?}", status);
        unsafe {
          (*simple_text_in_ex_ptr).keyboard_handler = core::ptr::null_mut();
        }
        return Err(status);
      }

      let key_notify_event: efi::Handle = self.key_notify_event;
      let status = self.boot_services.close_event(key_notify_event);
      if status.is_error() {
        //An error here means the event was not closed, so in theory the notification_callback on it could still be
        //fired.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on simple_text_in_ptr to continue to resolve
        //and return error based on observing keyboard_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to close key_notify_event event, status: {:x?}", status);
        unsafe {
          (*simple_text_in_ex_ptr).keyboard_handler = core::ptr::null_mut();
        }
        return Err(status);
      }

      // None of the parts of simple_text_in_ptr simple_text_in_ptr are in use, so it is safe to reclaim it.
      drop(unsafe { Box::from_raw(simple_text_in_ex_ptr) });
    }
    Ok(())
  }

  fn install_protocol_interfaces(&mut self, controller: efi::Handle) -> Result<(), efi::Status> {
    self.install_simple_text_in(controller)?;
    self.install_simple_text_in_ex(controller)?;
    // after this point, access to self must be guarded by raising TPL to NOTIFY.
    Ok(())
  }

  fn install_layout_change_event(&mut self) -> Result<(), efi::Status> {
    let context = LayoutChangeContext { boot_services: self.boot_services, keyboard_handler: self as *mut Self };
    let context_ptr = Box::into_raw(Box::new(context));

    let mut layout_change_event: efi::Event = core::ptr::null_mut();
    let status = self.boot_services.create_event_ex(
      efi::EVT_NOTIFY_SIGNAL,
      efi::TPL_NOTIFY,
      Some(on_layout_update),
      context_ptr as *mut c_void,
      &protocols::hii_database::SET_KEYBOARD_LAYOUT_EVENT_GUID,
      core::ptr::addr_of_mut!(layout_change_event),
    );
    if status.is_error() {
      Err(status)?;
    }

    self.layout_change_event = layout_change_event;
    self.layout_context = context_ptr;

    Ok(())
  }

  fn uninstall_layout_change_event(&mut self) -> Result<(), efi::Status> {
    if !self.layout_change_event.is_null() {
      let layout_change_event: efi::Handle = self.layout_change_event;
      let status = self.boot_services.close_event(layout_change_event);
      if status.is_error() {
        //An error here means the event was not closed, so in theory the notification_callback on it could still be fired.
        //Mark the instance invalid by setting the keyboard_handler raw pointer to null, but leak the LayoutContext
        //instance. Leaking context allows context usage in the callback should it fire.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to close layout_change_event event, status: {:x?}", status);
        unsafe {
          (*self.layout_context).keyboard_handler = core::ptr::null_mut();
        }
        return Err(status);
      }
      // safe to drop layout change context.
      drop(unsafe { Box::from_raw(self.layout_context) });
      self.layout_context = core::ptr::null_mut();
      self.layout_change_event = core::ptr::null_mut();
    }
    Ok(())
  }

  fn install_default_layout(&mut self) -> Result<(), efi::Status> {
    let mut hii_database_protocol_ptr: *mut protocols::hii_database::Protocol = core::ptr::null_mut();

    let status = self.boot_services.locate_protocol(
      &protocols::hii_database::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::null_mut(),
      core::ptr::addr_of_mut!(hii_database_protocol_ptr) as *mut *mut c_void,
    );
    if status.is_error() {
      #[cfg(not(test))]
      debugln!(
        DEBUG_ERROR,
        "keyboard::install_default_layout: Could not locate hii_database protocol to install keyboard layout: {:x?}",
        status
      );
      Err(status)?;
    }

    let hii_database_protocol =
      unsafe { hii_database_protocol_ptr.as_mut().expect("Bad pointer returned from successful locate protocol.") };

    let mut hii_handle: hii::Handle = core::ptr::null_mut();
    let status = (hii_database_protocol.new_package_list)(
      hii_database_protocol_ptr,
      hii_keyboard_layout::get_default_keyboard_pkg_list_buffer().as_ptr() as *const hii::PackageListHeader,
      core::ptr::null_mut(),
      core::ptr::addr_of_mut!(hii_handle),
    );

    if status.is_error() {
      #[cfg(not(test))]
      debugln!(
        DEBUG_ERROR,
        "keyboard::install_default_layout: Failed to install keyboard layout package: {:x?}",
        status
      );
      Err(status)?;
    }

    let status = (hii_database_protocol.set_keyboard_layout)(
      hii_database_protocol_ptr,
      &hii_keyboard_layout::DEFAULT_KEYBOARD_LAYOUT_GUID as *const efi::Guid as *mut efi::Guid,
    );

    if status.is_error() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "keyboard::install_default_layout: Failed to set keyboard layout: {:x?}", status);
      Err(status)?;
    }

    Ok(())
  }

  fn initialize_keyboard_layout(&mut self) -> Result<(), efi::Status> {
    self.install_layout_change_event()?;

    //signal event to pick up any existing layout
    self.boot_services.signal_event(self.layout_change_event);

    //install a default layout if no layout is installed.
    if self.key_queue.get_layout().is_none() {
      self.install_default_layout()?;
    }
    Ok(())
  }
  fn reset(&mut self, extended_verification: bool) -> Result<(), efi::Status> {
    self.last_keys.clear();
    self.current_keys.clear();
    self.key_queue.reset(extended_verification.into());
    if extended_verification {
      self.update_leds()?;
    }
    Ok(())
  }
  fn update_leds(&mut self) -> Result<(), efi::Status> {
    let controller = self.controller.expect("keyboard handler not initialized");
    // For simplicity, UefiHidIoFactory is used directly here to get a hid_io instance to send an LED report.
    // This avoids having to manage a hid_io or hid_io_factory trait object reference on self.
    let hid_factory = UefiHidIoFactory::new(self.boot_services, self.agent);
    let hid_io = hid_factory.new_hid_io(controller, false);
    if let Ok(hid_io) = hid_io {
      for (id, output_report) in self.generate_led_output_reports() {
        let result = hid_io.set_output_report(id.map(|x| u32::from(x) as u8), &output_report);
        if let Err(result) = result {
          #[cfg(not(test))]
          debugln!(DEBUG_ERROR, "unexpected error sending output report: {:?}", result);
          return Err(result);
        }
      }
    } else {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "update_leds: failed to get an instance of hid_io for LED reset.");
      return Err(efi::Status::DEVICE_ERROR);
    }
    Ok(())
  }
}

impl HidReportReciever for KeyboardHidHandler {
  fn initialize(&mut self, controller: efi::Handle, hid_io: &dyn HidIo) -> Result<(), efi::Status> {
    let descriptor = hid_io.get_report_descriptor()?;
    self.process_descriptor(descriptor)?;

    self.install_protocol_interfaces(controller)?;

    self.initialize_keyboard_layout()?;

    Ok(())
  }

  fn receive_report(&mut self, report: &[u8], hid_io: &dyn HidIo) {
    let old_tpl = self.boot_services.raise_tpl(efi::TPL_NOTIFY);

    let mut output_reports = Vec::new();
    'report_processing: {
      if report.len() == 0 {
        break 'report_processing;
      }
      // determine whether report includes report id byte and adjust the buffer as needed.
      let (report_id, report) = match self.report_id_present {
        true => (Some(ReportId::from(&report[0..1])), &report[1..]),
        false => (None, &report[0..]),
      };

      if report.len() == 0 {
        break 'report_processing;
      }

      if let Some(report_data) = self.input_reports.get(&report_id).cloned() {
        if report.len() != report_data.report_size {
          break 'report_processing;
        }

        //reset currently active keys to empty set.
        self.current_keys.clear();

        // hand the report data to the handler for each relevant field for field-specific processing.
        for field in report_data.relevant_variable_fields {
          (field.report_handler)(self, field.field, report);
        }

        for field in report_data.relevant_array_fields {
          (field.report_handler)(self, field.field, report);
        }

        //check if any key state has changed.
        if self.last_keys != self.current_keys {
          // process keys that are not in both sets: that is the set of keys that have changed.
          // XOR on the sets yields a set of keys that are in either last or current keys, but not both.

          // Modifier keys need to be processed first so that normal key processing includes modifiers that showed up in
          // the same report. The key sets are sorted by Usage, and modifier keys all have higher usages than normal keys
          // - so use a reverse iterator to process the modifier keys first. In addition, all released keys should be
          // processed first so that pressed keys (which typically generate key stroke events) have the most recent
          // key state associated with them.
          let mut released_keys = Vec::new();
          let mut pressed_keys = Vec::new();
          for changed_key in (&self.last_keys ^ &self.current_keys).into_iter().rev() {
            if self.last_keys.contains(&changed_key) {
              //In the last key list, but not in current. This is a key release.
              released_keys.push(changed_key);
            } else {
              //Not in last, so must be in current. This is a key press.
              pressed_keys.push(changed_key);
            }
          }
          for key in released_keys {
            self.key_queue.keystroke(key, key_queue::KeyAction::KeyUp);
          }
          for key in pressed_keys {
            self.key_queue.keystroke(key, key_queue::KeyAction::KeyDown);
          }

          //after processing all the key strokes, check if any keys were pressed that should trigger the notifier callback
          //and if so, signal the event to trigger notify processing at the appropriate TPL.
          if self.key_queue.peek_notify_key().is_some() {
            self.boot_services.signal_event(self.key_notify_event);
          }

          //after processing all the key strokes, send updated LED state if required.
          output_reports = self.generate_led_output_reports();
        }
        //after all key handling is complete for this report, update the last key set to match the current key set.
        self.last_keys = self.current_keys.clone();
      }
    }

    self.boot_services.restore_tpl(old_tpl);

    // if any output reoports, send them after releasing handler.
    for (id, output_report) in output_reports {
      let result = hid_io.set_output_report(id.map(|x| u32::from(x) as u8), &output_report);
      if let Err(result) = result {
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "unexpected error sending output report: {:?}", result);
        let _ = result;
      }
    }
  }
}

impl Drop for KeyboardHidHandler {
  fn drop(&mut self) {
    let status = self.uninstall_simple_text_in();
    if status.is_err() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "KeyboardHidHandler::drop: Failed to uninstall simple_text_in: {:?}", status);
    }
    let status = self.uninstall_simple_text_in_ex();
    if status.is_err() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "KeyboardHidHandler::drop: Failed to uninstall simple_text_in: {:?}", status);
    }
    let status = self.uninstall_layout_change_event();
    if status.is_err() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "KeyboardHidHandler::drop: Failed to close layout_change_event: {:?}", status);
    }
  }
}

// resets the keyboard state - part of the simple_text_in protocol interface.
extern "efiapi" fn simple_text_in_reset(
  this: *mut protocols::simple_text_input::Protocol,
  extended_verification: efi::Boolean,
) -> efi::Status {
  if this.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let context = unsafe { (this as *mut SimpleTextInContext).as_mut() }.expect("bad pointer");
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  let mut status = efi::Status::SUCCESS;
  'reset_processing: {
    let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
    if let Some(keyboard_handler) = keyboard_handler {
      match keyboard_handler.reset(extended_verification.into()) {
        Err(err) => {
          status = err;
          break 'reset_processing;
        }
        _ => (),
      }
    } else {
      status = efi::Status::DEVICE_ERROR;
      break 'reset_processing;
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// reads a key stroke - part of the simple_text_in protocol interface.
extern "efiapi" fn simple_text_in_read_key_stroke(
  this: *mut protocols::simple_text_input::Protocol,
  key: *mut protocols::simple_text_input::InputKey,
) -> efi::Status {
  if this.is_null() || key.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let context = unsafe { (this as *mut SimpleTextInContext).as_mut() }.expect("bad pointer");
  let status;
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  'read_key_stroke: {
    let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
    if let Some(keyboard_handler) = keyboard_handler {
      loop {
        if let Some(mut key_data) = keyboard_handler.key_queue.pop_key() {
          // skip partials
          if key_data.key.unicode_char == 0 && key_data.key.scan_code == 0 {
            continue;
          }
          //translate ctrl-alpha to corresponding control value. ctrl-a = 0x0001, ctrl-z = 0x001A
          const CONTROL_PRESSED: u32 = protocols::simple_text_input_ex::RIGHT_CONTROL_PRESSED
            | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED;
          if (key_data.key_state.key_shift_state & CONTROL_PRESSED) != 0 {
            if key_data.key.unicode_char >= 0x0061 && key_data.key.unicode_char <= 0x007a {
              //'a' to 'z'
              key_data.key.unicode_char = (key_data.key.unicode_char - 0x0061) + 1;
            }
            if key_data.key.unicode_char >= 0x0041 && key_data.key.unicode_char <= 0x005a {
              //'A' to 'Z'
              key_data.key.unicode_char = (key_data.key.unicode_char - 0x0041) + 1;
            }
          }
          unsafe { key.write(key_data.key) }
          status = efi::Status::SUCCESS;
        } else {
          status = efi::Status::NOT_READY;
        }
        break 'read_key_stroke;
      }
    } else {
      status = efi::Status::DEVICE_ERROR;
      break 'read_key_stroke;
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// Event handler function for the wait_for_key_event
extern "efiapi" fn simple_text_in_wait_for_key(event: efi::Event, context: *mut c_void) {
  if context.is_null() {
    #[cfg(not(test))]
    debugln!(DEBUG_ERROR, "simple_text_in_wait_for_key invoked with invalid context");
    return;
  }
  let context = unsafe { (context as *mut SimpleTextInContext).as_mut() }.expect("bad pointer");
  let mut wait_complete: bool = false;
  while !wait_complete {
    let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
    {
      if let Some(keyboard_handler) = unsafe { context.keyboard_handler.as_mut() } {
        while let Some(key_data) = keyboard_handler.key_queue.peek_key() {
          if key_data.key.unicode_char == 0 && key_data.key.scan_code == 0 {
            // consume (and ignore) the partial stroke.
            let _ = keyboard_handler.key_queue.pop_key();
            continue;
          } else {
            // valid keystroke
            context.boot_services.signal_event(event);
            wait_complete = true;
            break;
          }
        }
      } else {
        wait_complete = true;
      }
    }
    context.boot_services.restore_tpl(old_tpl);
  }
}

// resets the keyboard state - part of the simple_text_in_ex protocol interface.
extern "efiapi" fn simple_text_in_ex_reset(
  this: *mut protocols::simple_text_input_ex::Protocol,
  extended_verification: efi::Boolean,
) -> efi::Status {
  if this.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let context = unsafe { (this as *mut SimpleTextInExContext).as_mut() }.expect("bad pointer");
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  let mut status = efi::Status::SUCCESS;
  'reset_processing: {
    let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
    if let Some(keyboard_handler) = keyboard_handler {
      match keyboard_handler.reset(extended_verification.into()) {
        Err(err) => {
          status = err;
          break 'reset_processing;
        }
        _ => (),
      }
    } else {
      status = efi::Status::DEVICE_ERROR;
      break 'reset_processing;
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// reads a key stroke - part of the simple_text_in_ex protocol interface.
extern "efiapi" fn simple_text_in_ex_read_key_stroke(
  this: *mut protocols::simple_text_input_ex::Protocol,
  key_data: *mut protocols::simple_text_input_ex::KeyData,
) -> efi::Status {
  if this.is_null() || key_data.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let context = unsafe { (this as *mut SimpleTextInExContext).as_mut() }.expect("bad pointer");
  let mut status = efi::Status::SUCCESS;
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  'read_key_stroke: {
    let keyboard_handler = unsafe { context.keyboard_handler.as_mut() };
    if let Some(keyboard_handler) = keyboard_handler {
      if let Some(key) = keyboard_handler.key_queue.pop_key() {
        unsafe { key_data.write(key) }
      } else {
        let mut key: protocols::simple_text_input_ex::KeyData = Default::default();
        key.key_state = keyboard_handler.key_queue.init_key_state();
        unsafe { key_data.write(key) };
        status = efi::Status::NOT_READY
      }
    } else {
      status = efi::Status::DEVICE_ERROR;
      break 'read_key_stroke;
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// sets the keyboard state - part of the simple_text_in_ex protocol interface.
extern "efiapi" fn simple_text_in_ex_set_state(
  this: *mut protocols::simple_text_input_ex::Protocol,
  key_toggle_state: *mut protocols::simple_text_input_ex::KeyToggleState,
) -> efi::Status {
  if this.is_null() || key_toggle_state.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let context = unsafe { (this as *mut SimpleTextInExContext).as_mut() }.expect("bad pointer");
  let mut status = efi::Status::SUCCESS;
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  '_set_state_processing: {
    if let Some(keyboard_handler) = unsafe { context.keyboard_handler.as_mut() } {
      keyboard_handler.key_queue.set_key_toggle_state(unsafe { key_toggle_state.read() });
      let result = keyboard_handler.update_leds();
      if let Err(result) = result {
        status = result;
      }
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// registers a key notification callback function - part of the simple_text_in_ex protocol interface.
extern "efiapi" fn simple_text_in_ex_register_key_notify(
  this: *mut protocols::simple_text_input_ex::Protocol,
  key_data_ptr: *mut protocols::simple_text_input_ex::KeyData,
  key_notification_function: protocols::simple_text_input_ex::KeyNotifyFunction,
  notify_handle: *mut *mut c_void,
) -> efi::Status {
  if this.is_null() || key_data_ptr.is_null() || notify_handle.is_null() || key_notification_function as usize == 0 {
    return efi::Status::INVALID_PARAMETER;
  }

  let context = unsafe { (this as *mut SimpleTextInExContext).as_mut() }.expect("bad pointer");
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
  let status;
  'register_notify_processing: {
    if let Some(keyboard_handler) = unsafe { context.keyboard_handler.as_mut() } {
      let key_data = OrdKeyData(unsafe { key_data_ptr.read() });
      for (handle, entry) in &keyboard_handler.notification_callbacks {
        if entry.0 == key_data && entry.1 == key_notification_function {
          //if callback already exists, just return current handle.
          unsafe { notify_handle.write(*handle as *mut c_void) };
          status = efi::Status::SUCCESS;
          break 'register_notify_processing;
        }
      }
      // key_data/callback combo doesn't already exist; create a new registration for it.
      keyboard_handler.next_notify_handle += 1;
      keyboard_handler
        .notification_callbacks
        .insert(keyboard_handler.next_notify_handle, (key_data.clone(), key_notification_function));
      keyboard_handler.key_queue.add_notify_key(key_data);
      unsafe { notify_handle.write(keyboard_handler.next_notify_handle as *mut c_void)};
      status = efi::Status::SUCCESS;
    } else {
      status = efi::Status::DEVICE_ERROR;
    }
  }
  context.boot_services.restore_tpl(old_tpl);
  status
}

// unregisters a key notification callback function - part of the simple_text_in_ex protocol interface.
extern "efiapi" fn simple_text_in_ex_unregister_key_notify(
  _this: *mut protocols::simple_text_input_ex::Protocol,
  _notification_handle: *mut c_void,
) -> efi::Status {
  todo!();
}

// Event handler function for the wait_for_key_event
extern "efiapi" fn simple_text_in_ex_wait_for_key(event: efi::Event, context: *mut c_void) {
  if context.is_null() {
    #[cfg(not(test))]
    debugln!(DEBUG_ERROR, "simple_text_in_ex_wait_for_key invoked with invalid context");
    return;
  }
  let context = unsafe { (context as *mut SimpleTextInExContext).as_mut() }.expect("bad pointer");
  let mut wait_complete: bool = false;
  while !wait_complete {
    let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
    {
      if let Some(keyboard_handler) = unsafe { context.keyboard_handler.as_mut() } {
        while let Some(key_data) = keyboard_handler.key_queue.peek_key() {
          if key_data.key.unicode_char == 0 && key_data.key.scan_code == 0 {
            // consume (and ignore) the partial stroke.
            let _ = keyboard_handler.key_queue.pop_key();
            continue;
          } else {
            // valid keystroke
            context.boot_services.signal_event(event);
            wait_complete = true;
            break;
          }
        }
      } else {
        wait_complete = true;
      }
    }
    context.boot_services.restore_tpl(old_tpl);
  }
}

// Event callback function for handling registered key notifications. Iterates over the queue of keys to be notified,
// and invokes the registered callback function for each of those keys.
extern "efiapi" fn process_key_notifies(_event: efi::Event, context: *mut c_void) {
  if let Some(context) = unsafe { (context as *mut SimpleTextInExContext).as_mut() } {
    loop {
      let mut pending_key = None;
      let mut pending_callbacks = Vec::new();
      let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);
      if let Some(keyboard_handler) = unsafe { context.keyboard_handler.as_mut() } {
        if let Some(pending_notify_key) = keyboard_handler.key_queue.pop_notifiy_key() {
          pending_key = Some(pending_notify_key);
          for (key, callback) in keyboard_handler.notification_callbacks.values() {
            if OrdKeyData(pending_notify_key).matches_registered_key(key) {
              pending_callbacks.push(callback);
            }
          }
        }
      } else {
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "process_key_notifies event called without a valid keyboard_handler");
      }
      context.boot_services.restore_tpl(old_tpl);

      //dispatch notifies (if any) at the TPL this event callback was invoked at.
      if let Some(mut pending_key) = pending_key {
        let key_ptr = &mut pending_key as *mut protocols::simple_text_input_ex::KeyData;
        for callback in pending_callbacks {
          let _ = callback(key_ptr);
        }
      } else {
        // no pending notifies to process
        break;
      }
    }
  }
}

extern "efiapi" fn on_layout_update(_event: efi::Event, context: *mut c_void) {
  let context = unsafe { (context as *mut LayoutChangeContext).as_mut() }.expect("bad context pointer");
  let old_tpl = context.boot_services.raise_tpl(efi::TPL_NOTIFY);

  'layout_processing: {
    if context.keyboard_handler.is_null() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "on_layout_update invoked with invalid handler");
      break 'layout_processing;
    }

    let keyboard_handler = unsafe { context.keyboard_handler.as_mut() }.expect("bad keyboard handler");

    let mut hii_database_protocol_ptr: *mut protocols::hii_database::Protocol = core::ptr::null_mut();
    let status = context.boot_services.locate_protocol(
      &protocols::hii_database::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      core::ptr::null_mut(),
      core::ptr::addr_of_mut!(hii_database_protocol_ptr) as *mut *mut c_void,
    );

    if status.is_error() {
      //nothing to do if there is no hii protocol.
      break 'layout_processing;
    }

    let hii_database_protocol =
      unsafe { hii_database_protocol_ptr.as_mut().expect("Bad pointer returned from successful locate protocol.") };

    // retrieve keyboard layout size
    let mut layout_buffer_len: u16 = 0;
    let status = (hii_database_protocol.get_keyboard_layout)(
      hii_database_protocol_ptr,
      core::ptr::null_mut(),
      &mut layout_buffer_len as *mut u16,
      core::ptr::null_mut(),
    );
    if status != efi::Status::BUFFER_TOO_SMALL {
      #[cfg(not(test))]
      debugln!(
        DEBUG_ERROR,
        "Unexpected return from get_keyboard_layout when trying to determine length: {:x?}",
        status
      );
      break 'layout_processing;
    }

    let mut keyboard_layout_buffer = vec![0u8; layout_buffer_len as usize];
    let status = (hii_database_protocol.get_keyboard_layout)(
      hii_database_protocol_ptr,
      core::ptr::null_mut(),
      &mut layout_buffer_len as *mut u16,
      keyboard_layout_buffer.as_mut_ptr() as *mut protocols::hii_database::KeyboardLayout<0>,
    );

    if status.is_error() {
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "Unexpected return from get_keyboard_layout: {:x?}", status);
      break 'layout_processing;
    }

    let keyboard_layout = hii_keyboard_layout::keyboard_layout_from_buffer(&keyboard_layout_buffer);
    match keyboard_layout {
      Ok(keyboard_layout) => {
        keyboard_handler.key_queue.set_layout(Some(keyboard_layout));
      }
      Err(_) => {
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "keyboard::on_layout_update: Could not parse keyboard layout buffer.");
        break 'layout_processing;
      }
    }
  }

  context.boot_services.restore_tpl(old_tpl);
}

#[cfg(test)]
mod test {
  use core::{ffi::c_void, mem::MaybeUninit, slice::from_raw_parts_mut};
  use std::time::{SystemTime, UNIX_EPOCH};

  use hii_keyboard_layout::{self, HiiKeyboardLayout};
  use r_efi::{
    efi, hii,
    protocols::{self, simple_text_input, simple_text_input_ex},
  };
  use scroll::Pwrite;

  use crate::{
    boot_services::MockUefiBootServices,
    hid_io::{HidReportReciever, MockHidIo},
    keyboard::{
      on_layout_update, process_key_notifies, simple_text_in_ex_read_key_stroke, simple_text_in_ex_register_key_notify,
      simple_text_in_ex_reset, simple_text_in_ex_set_state, simple_text_in_ex_unregister_key_notify,
      simple_text_in_ex_wait_for_key, simple_text_in_wait_for_key, KeyboardHidHandler, SimpleTextInExContext,
    }, key_queue::OrdKeyData,
  };

  use super::{simple_text_in_read_key_stroke, simple_text_in_reset, LayoutChangeContext, SimpleTextInContext};

  static BOOT_KEYBOARD_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // USAGE_PAGE (Generic Desktop)
    0x09, 0x06, // USAGE (Keyboard)
    0xa1, 0x01, // COLLECTION (Application)
    0x75, 0x01, //    REPORT_SIZE (1)
    0x95, 0x08, //    REPORT_COUNT (8)
    0x05, 0x07, //    USAGE_PAGE (Key Codes)
    0x19, 0xE0, //    USAGE_MINIMUM (224)
    0x29, 0xE7, //    USAGE_MAXIMUM (231)
    0x15, 0x00, //    LOGICAL_MAXIMUM (0)
    0x25, 0x01, //    LOGICAL_MINIMUM (1)
    0x81, 0x02, //    INPUT (Data, Var, Abs) (Modifier Byte)
    0x95, 0x01, //    REPORT_COUNT (1)
    0x75, 0x08, //    REPORT_SIZE (8)
    0x81, 0x03, //    INPUT (Const) (Reserved Byte)
    0x95, 0x05, //    REPORT_COUNT (5)
    0x75, 0x01, //    REPORT_SIZE (1)
    0x05, 0x08, //    USAGE_PAGE (LEDs)
    0x19, 0x01, //    USAGE_MINIMUM (1)
    0x29, 0x05, //    USAGE_MAXIMUM (5)
    0x91, 0x02, //    OUTPUT (Data, Var, Abs) (LED report)
    0x95, 0x01, //    REPORT_COUNT (1)
    0x75, 0x03, //    REPORT_SIZE (3)
    0x91, 0x02, //    OUTPUT (Constant) (LED report padding)
    0x95, 0x06, //    REPORT_COUNT (6)
    0x75, 0x08, //    REPORT_SIZE (8)
    0x15, 0x00, //    LOGICAL_MINIMUM (0)
    0x26, 0xff, 00, //    LOGICAL_MAXIMUM (255)
    0x05, 0x07, //    USAGE_PAGE (Key Codes)
    0x19, 0x00, //    USAGE_MINIMUM (0)
    0x2a, 0xff, 00, //    USAGE_MAXIMUM (255)
    0x81, 0x00, //    INPUT (Data, Array)
    0xc0, // END_COLLECTION
  ];

  static MOUSE_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // USAGE_PAGE (Generic Desktop)
    0x09, 0x02, // USAGE (Mouse)
    0xa1, 0x01, // COLLECTION (Application)
    0x09, 0x01, //   USAGE(Pointer)
    0xa1, 0x00, //   COLLECTION (Physical)
    0x05, 0x09, //     USAGE_PAGE (Button)
    0x19, 0x01, //     USAGE_MINIMUM(1)
    0x29, 0x05, //     USAGE_MAXIMUM(5)
    0x15, 0x00, //     LOGICAL_MINIMUM(0)
    0x25, 0x01, //     LOGICAL_MAXIMUM(1)
    0x95, 0x05, //     REPORT_COUNT(5)
    0x75, 0x01, //     REPORT_SIZE(1)
    0x81, 0x02, //     INPUT(Data, Variable, Absolute)
    0x95, 0x01, //     REPORT_COUNT(1)
    0x75, 0x03, //     REPORT_SIZE(3)
    0x81, 0x01, //     INPUT(Constant, Array, Absolute)
    0x05, 0x01, //     USAGE_PAGE (Generic Desktop)
    0x09, 0x30, //     USAGE (X)
    0x09, 0x31, //     USAGE (Y)
    0x09, 0x38, //     USAGE (Wheel)
    0x15, 0x81, //     LOGICAL_MINIMUM (-127)
    0x25, 0x7f, //     LOGICAL_MAXIMUM (127)
    0x75, 0x08, //     REPORT_SIZE (8)
    0x95, 0x03, //     REPORT_COUNT (3)
    0x81, 0x06, //     INPUT(Data, Variable, Relative)
    0xc0, //   END_COLLECTION
    0xc0, // END_COLLECTION
  ];

  #[test]
  fn keyboard_initialize_should_fail_if_report_descriptor_not_supported() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(keyboard_handler.initialize(controller, &hid_io), Err(efi::Status::UNSUPPORTED));
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn successful_keyboard_initialize_should_install_protocol_and_drop_should_tear_it_down() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut SIMPLE_TEXT_IN_INTERFACE: *mut c_void = core::ptr::null_mut();
      static mut SIMPLE_TEXT_IN_EX_INTERFACE: *mut c_void = core::ptr::null_mut();
      static mut SIMPLE_TEXT_IN_EVENT: efi::Event = 0x1 as efi::Event;
      static mut SIMPLE_TEXT_IN_EX_EVENT: efi::Event = 0x2 as efi::Event;
      static mut PROCESS_KEY_NOTIFIES_EVENT: efi::Event = 0x3 as efi::Event;
      static mut LAYOUT_CHANGE_EVENT: efi::Event = 0x04 as efi::Event;

      //expected for both simple_text_in and simple_text_in_ex.
      boot_services.expect_create_event().returning(|_, _, function, _, event| {
        unsafe {
          match function {
            Some(x) if x == simple_text_in_wait_for_key => event.write(SIMPLE_TEXT_IN_EVENT),
            Some(x) if x == simple_text_in_ex_wait_for_key => event.write(SIMPLE_TEXT_IN_EX_EVENT),
            Some(x) if x == process_key_notifies => event.write(PROCESS_KEY_NOTIFIES_EVENT),
            _ => panic!("Invalid event function"),
          }
        }
        efi::Status::SUCCESS
      });

      //expected for layout initialization
      boot_services.expect_create_event_ex().returning(|_, _, function, _, _, event| {
        unsafe {
          match function {
            Some(x) if x == on_layout_update => event.write(LAYOUT_CHANGE_EVENT),
            _ => panic!("Invalid event function"),
          }
        }
        efi::Status::SUCCESS
      });
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      //expected for both simple_text_in and simple_text_in_ex.
      boot_services.expect_install_protocol_interface().returning(|_, protocol, _, interface| {
        unsafe {
          match *protocol {
            simple_text_input::PROTOCOL_GUID => SIMPLE_TEXT_IN_INTERFACE = interface,
            simple_text_input_ex::PROTOCOL_GUID => SIMPLE_TEXT_IN_EX_INTERFACE = interface,
            _ => panic!("Unrecognized simple text GUID"),
          }
        }
        efi::Status::SUCCESS
      });

      //expected for drop on both simple_text_in and simple_text_in_ex
      boot_services.expect_open_protocol().returning(|_, protocol, interface, _, _, _| {
        unsafe {
          match *protocol {
            simple_text_input::PROTOCOL_GUID => interface.write(SIMPLE_TEXT_IN_INTERFACE),
            simple_text_input_ex::PROTOCOL_GUID => interface.write(SIMPLE_TEXT_IN_EX_INTERFACE),
            _ => panic!("Unrecognized simple text GUID"),
          }
        }
        efi::Status::SUCCESS
      });

      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap()));

      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));
      let controller = 0x2 as efi::Handle;
      keyboard_handler.initialize(controller, &hid_io).unwrap();

      assert_ne!(unsafe { SIMPLE_TEXT_IN_INTERFACE }, core::ptr::null_mut());
      assert_ne!(unsafe { SIMPLE_TEXT_IN_EX_INTERFACE }, core::ptr::null_mut());
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn keyhandler_should_handle_key_report() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      let hid_io = MockHidIo::new();

      // press the 'a' key.
      let report: &[u8] = &[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      assert_eq!(keyboard_handler.key_queue.pop_key().unwrap().key.unicode_char, 'a' as u16);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // press the 'shift' key while 'a' remains pressed.
      let report: &[u8] = &[0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // holding the 'shift' key while releasing 'a'.
      let report: &[u8] = &[0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // holding the 'shift' key while pressing 'a' again.
      let report: &[u8] = &[0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      assert_eq!(keyboard_handler.key_queue.pop_key().unwrap().key.unicode_char, 'A' as u16);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // release the 'shift' key, press the 'ctrl' key, continue pressing 'a', and press 'b'
      let report: &[u8] = &[0x01, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      let key_data = keyboard_handler.key_queue.pop_key().unwrap();
      assert_eq!(key_data.key.unicode_char, 'b' as u16);
      assert_eq!(
        key_data.key_state.key_shift_state,
        protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED
      );
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // enable partial keystrokes
      keyboard_handler.key_queue.set_key_toggle_state(protocols::simple_text_input_ex::KEY_STATE_EXPOSED);

      // release the 'ctrl' key, press 'alt', continue pressing 'a' and 'b'
      let report: &[u8] = &[0x04, 0x00, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      let key_data = keyboard_handler.key_queue.pop_key().unwrap();
      assert_eq!(key_data.key.unicode_char, 0);
      assert_eq!(
        key_data.key_state.key_shift_state,
        protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_ALT_PRESSED
      );
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // release all the keys
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // press the right logo key
      let report: &[u8] = &[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      let key_data = keyboard_handler.key_queue.pop_key().unwrap();
      assert_eq!(key_data.key.unicode_char, 0);
      assert_eq!(
        key_data.key_state.key_shift_state,
        protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::RIGHT_LOGO_PRESSED
      );
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // simulate rollover
      let report: &[u8] = &[0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // pass some unsupported keys
      let report: &[u8] = &[0x00, 0x00, 0xF0, 0xF0, 0xF0, 0xF0, 0xF0, 0xF0];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_none());

      // try all possible modifers and all possible keys and make sure it doesn't panic.
      // some of these may generate LED reports
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|_, _| Ok(()));
      for i in 0..0xff {
        // avoid sending "Delete" keys as it will cause reset if CTRL-ALT are pressed.
        if (i == 0x4C) || (i == 0x63) {
          continue;
        }
        let report: &[u8] = &[i, 0x00, i, 0x00, 0x00, 0x00, 0x00, 0x00];
        keyboard_handler.receive_report(report, &hid_io);
      }
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn keyhandler_should_send_led_updates() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      // press the 'caps lock' key.
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|id, report| {
        assert_eq!(id, None);
        assert_eq!(report, &[0x02]);
        Ok(())
      });

      let report: &[u8] = &[0x00, 0x00, 0x39, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      // release 'caps lock' and press the 'num lock' key.
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|id, report| {
        assert_eq!(id, None);
        assert_eq!(report, &[0x03]);
        Ok(())
      });

      let report: &[u8] = &[0x00, 0x00, 0x53, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      // press 'caps lock' again
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|id, report| {
        assert_eq!(id, None);
        assert_eq!(report, &[0x01]);
        Ok(())
      });

      let report: &[u8] = &[0x00, 0x00, 0x39, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      // press 'scroll lock'
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|id, report| {
        assert_eq!(id, None);
        assert_eq!(report, &[0x05]);
        Ok(())
      });

      let report: &[u8] = &[0x00, 0x00, 0x47, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //release scroll lock
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      // press 'scroll lock' and 'num lock' again
      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|id, report| {
        assert_eq!(id, None);
        assert_eq!(report, &[0x00]);
        Ok(())
      });

      let report: &[u8] = &[0x00, 0x00, 0x47, 0x53, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  #[should_panic(expected = "Reset failed.")] //Reset will fail because RUNTIME_SERVICES is null for test cases.
  fn keyhandler_should_reset_on_ctrl_alt_del() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      let hid_io = MockHidIo::new();

      //send ctrl-alt-del
      let report: &[u8] = &[0x05, 0x00, 0x4C, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn install_default_layout_should_install_a_layout() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut HANDLER: *mut KeyboardHidHandler = core::ptr::null_mut();

      extern "efiapi" fn new_package_list(
        _this: *const protocols::hii_database::Protocol,
        _package_list: *const hii::PackageListHeader,
        _driver_handle: efi::Handle,
        _handle: *mut hii::Handle,
      ) -> efi::Status {
        efi::Status::SUCCESS
      }

      extern "efiapi" fn set_keyboard_layout(
        _this: *const protocols::hii_database::Protocol,
        _keyguid: *mut efi::Guid,
      ) -> efi::Status {
        unsafe {
          HANDLER.as_mut().unwrap().key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));
        }
        efi::Status::SUCCESS
      }

      boot_services.expect_locate_protocol().returning(|protocol, _, interface| {
        unsafe {
          match *protocol {
            protocols::hii_database::PROTOCOL_GUID => {
              let hii_database = MaybeUninit::<protocols::hii_database::Protocol>::zeroed();
              let mut hii_database = hii_database.assume_init();
              hii_database.new_package_list = new_package_list;
              hii_database.set_keyboard_layout = set_keyboard_layout;

              interface.write(Box::into_raw(Box::new(hii_database)) as *mut c_void);
            }
            unexpected_protocol => panic!("unexpected locate protocol request for {:?}", unexpected_protocol),
          }
        }
        efi::Status::SUCCESS
      });

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      unsafe { HANDLER = &mut keyboard_handler as *mut KeyboardHidHandler };
      keyboard_handler.install_default_layout().unwrap();
      assert!(keyboard_handler.key_queue.get_layout().is_some());
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn new_system_layout_installation_should_update_layout() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      const TEST_KEYBOARD_GUID: efi::Guid =
        efi::Guid::from_fields(0xf1796c10, 0xdafb, 0x4989, 0xa0, 0x82, &[0x75, 0xe9, 0x65, 0x76, 0xbe, 0x52]);

      static mut TEST_KEYBOARD_LAYOUT: HiiKeyboardLayout =
        HiiKeyboardLayout { keys: Vec::new(), guid: TEST_KEYBOARD_GUID, descriptions: Vec::new() };
      unsafe {
        //make a test keyboard layout that is different than the default.
        TEST_KEYBOARD_LAYOUT = hii_keyboard_layout::get_default_keyboard_layout();
        TEST_KEYBOARD_LAYOUT.guid = TEST_KEYBOARD_GUID;
        TEST_KEYBOARD_LAYOUT.keys.pop();
        TEST_KEYBOARD_LAYOUT.keys.pop();
        TEST_KEYBOARD_LAYOUT.keys.pop();
        TEST_KEYBOARD_LAYOUT.descriptions[0].description = "Test Keyboard Layout".to_string();
        TEST_KEYBOARD_LAYOUT.descriptions[0].language = "ts-TS".to_string();
      }

      extern "efiapi" fn get_keyboard_layout(
        _this: *const protocols::hii_database::Protocol,
        _keyguid: *const efi::Guid,
        keyboard_layout_length: *mut u16,
        keyboard_layout_ptr: *mut protocols::hii_database::KeyboardLayout,
      ) -> efi::Status {
        let mut keyboard_layout_buffer = vec![0u8; 4096];
        let buffer_size = keyboard_layout_buffer.pwrite(unsafe { &TEST_KEYBOARD_LAYOUT }, 0).unwrap();
        keyboard_layout_buffer.resize(buffer_size, 0);
        unsafe {
          if keyboard_layout_length.read() < buffer_size as u16 {
            keyboard_layout_length.write(buffer_size as u16);
            return efi::Status::BUFFER_TOO_SMALL;
          } else {
            if keyboard_layout_ptr.is_null() {
              panic!("bad keyboard pointer)");
            }
            keyboard_layout_length.write(buffer_size as u16);
            let slice = from_raw_parts_mut(keyboard_layout_ptr as *mut u8, buffer_size);
            slice.copy_from_slice(&keyboard_layout_buffer);
            return efi::Status::SUCCESS;
          }
        }
      }

      boot_services.expect_locate_protocol().returning(|protocol, _, interface| {
        unsafe {
          match *protocol {
            protocols::hii_database::PROTOCOL_GUID => {
              let hii_database = MaybeUninit::<protocols::hii_database::Protocol>::zeroed();
              let mut hii_database = hii_database.assume_init();
              hii_database.get_keyboard_layout = get_keyboard_layout;
              interface.write(Box::into_raw(Box::new(hii_database)) as *mut c_void);
            }
            unexpected_protocol => panic!("unexpected locate protocol request for {:?}", unexpected_protocol),
          }
        }
        efi::Status::SUCCESS
      });

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      let event = 0x02 as efi::Event;

      let context = Box::into_raw(Box::new(LayoutChangeContext {
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      }));

      on_layout_update(event, context as *mut c_void);
      assert_eq!(&keyboard_handler.key_queue.get_layout().unwrap(), unsafe { &TEST_KEYBOARD_LAYOUT });
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn reset_should_reset_keyboard() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      extern "efiapi" fn mock_set_report(
        _this: *const hid_io::protocol::Protocol,
        _report_id: u8,
        _report_type: hid_io::protocol::HidReportType,
        _report_buffer_size: usize,
        _report_buffer: *mut c_void,
      ) -> efi::Status {
        efi::Status::SUCCESS
      }

      boot_services.expect_open_protocol().returning(|_, protocol, interface, _, _, attributes| {
        unsafe {
          assert_eq!(protocol.read(), hid_io::protocol::GUID);
          assert_eq!(attributes, efi::OPEN_PROTOCOL_GET_PROTOCOL);
          let hid_io = MaybeUninit::<hid_io::protocol::Protocol>::zeroed();
          let mut hid_io = hid_io.assume_init();
          hid_io.set_report = mock_set_report;
          // note: this will leak a hid_io instance
          interface.write(Box::into_raw(Box::new(hid_io)) as *mut c_void);
        }
        efi::Status::SUCCESS
      });

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));
      keyboard_handler.controller = Some(0x2 as efi::Handle); //pretend full init has occurred.

      // test SimpleTextIn::reset
      let context = SimpleTextInContext {
        simple_text_in: protocols::simple_text_input::Protocol {
          reset: simple_text_in_reset,
          read_key_stroke: simple_text_in_read_key_stroke,
          wait_for_key: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|_, _| Ok(()));

      //buffer CapsLock + a, b, c
      let report: &[u8] = &[0x00, 0x00, 0x39, 0x04, 0x04, 0x05, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_some());
      assert!(!keyboard_handler.led_state.is_empty());
      let prev_led_state = keyboard_handler.led_state.clone();
      assert!(!keyboard_handler.last_keys.is_empty());

      let this_ptr = Box::into_raw(Box::new(context)) as *mut protocols::simple_text_input::Protocol;
      let status = simple_text_in_reset(this_ptr, efi::Boolean::from(false));
      assert_eq!(status, efi::Status::SUCCESS);
      assert!(keyboard_handler.key_queue.peek_key().is_none());
      assert!(keyboard_handler.last_keys.is_empty());
      assert_eq!(keyboard_handler.led_state, prev_led_state);

      let status = simple_text_in_reset(this_ptr, efi::Boolean::from(true));
      assert_eq!(status, efi::Status::SUCCESS);
      assert!(keyboard_handler.led_state.is_empty());

      //test SimpleTextInEx::reset
      let context: SimpleTextInExContext = SimpleTextInExContext {
        simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
          reset: simple_text_in_ex_reset,
          read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
          set_state: simple_text_in_ex_set_state,
          register_key_notify: simple_text_in_ex_register_key_notify,
          unregister_key_notify: simple_text_in_ex_unregister_key_notify,
          wait_for_key_ex: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let mut hid_io = MockHidIo::new();
      hid_io.expect_set_output_report().returning(|_, _| Ok(()));

      //buffer CapsLock + a, b, c
      let report: &[u8] = &[0x00, 0x00, 0x39, 0x04, 0x04, 0x05, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(keyboard_handler.key_queue.peek_key().is_some());
      assert!(!keyboard_handler.led_state.is_empty());
      let prev_led_state = keyboard_handler.led_state.clone();
      assert!(!keyboard_handler.last_keys.is_empty());

      let this_ptr = Box::into_raw(Box::new(context)) as *mut protocols::simple_text_input_ex::Protocol;
      let status = simple_text_in_ex_reset(this_ptr, efi::Boolean::from(false));
      assert_eq!(status, efi::Status::SUCCESS);
      assert!(keyboard_handler.key_queue.peek_key().is_none());
      assert!(keyboard_handler.last_keys.is_empty());
      assert_eq!(keyboard_handler.led_state, prev_led_state);

      let status = simple_text_in_ex_reset(this_ptr, efi::Boolean::from(true));
      assert_eq!(status, efi::Status::SUCCESS);
      assert!(keyboard_handler.led_state.is_empty());

      keyboard_handler.controller = None; //avoid boot services interactions in KeyboardHidHandler.drop().
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn read_key_stroke_should_return_key_data() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      // enable partial keystrokes
      keyboard_handler.key_queue.set_key_toggle_state(protocols::simple_text_input_ex::KEY_STATE_EXPOSED);

      let hid_io = MockHidIo::new();

      // build a simple text in context
      let context = SimpleTextInContext {
        simple_text_in: protocols::simple_text_input::Protocol {
          reset: simple_text_in_reset,
          read_key_stroke: simple_text_in_read_key_stroke,
          wait_for_key: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let context_ex = SimpleTextInExContext {
        simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
          reset: simple_text_in_ex_reset,
          read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
          set_state: simple_text_in_ex_set_state,
          register_key_notify: simple_text_in_ex_register_key_notify,
          unregister_key_notify: simple_text_in_ex_unregister_key_notify,
          wait_for_key_ex: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let simple_this_ptr = Box::into_raw(Box::new(context)) as *mut protocols::simple_text_input::Protocol;
      let simple_ex_this_ptr = Box::into_raw(Box::new(context_ex)) as *mut protocols::simple_text_input_ex::Protocol;
      let mut key_data: protocols::simple_text_input::InputKey = Default::default();
      let mut key_data_ex: protocols::simple_text_input_ex::KeyData = Default::default();

      //send 'a', 'b', 'c'. Simultaneous key stroke ordering is not defined by spec, but this implementation processes
      //them in descending usage code order, so 'c', 'b', 'a'.
      let report: &[u8] = &[0x00, 0x00, 0x04, 0x05, 0x06, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      //release keys and push ctrl
      let report: &[u8] = &[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //read with simple_text_in
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data.unicode_char, 'c' as u16);
      assert_eq!(key_data.scan_code, 0);

      //read with simple_text_in_ex
      let status = simple_text_in_ex_read_key_stroke(
        simple_ex_this_ptr,
        &mut key_data_ex as *mut protocols::simple_text_input_ex::KeyData,
      );
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data_ex.key.unicode_char, 'b' as u16);
      assert_eq!(key_data_ex.key.scan_code, 0);
      assert_eq!(key_data_ex.key_state.key_shift_state, protocols::simple_text_input_ex::SHIFT_STATE_VALID);
      assert_eq!(
        key_data_ex.key_state.key_toggle_state,
        protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED
      );

      //read again with simple_text_in
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data.unicode_char, 'a' as u16);
      assert_eq!(key_data.scan_code, 0);

      //read with empty queue with simple_text_in
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::NOT_READY);

      //read with empty queue with simple_text_in_ex
      let status = simple_text_in_ex_read_key_stroke(
        simple_ex_this_ptr,
        &mut key_data_ex as *mut protocols::simple_text_input_ex::KeyData,
      );
      assert_eq!(status, efi::Status::NOT_READY);
      assert_eq!(key_data_ex.key.unicode_char, 0);
      assert_eq!(key_data_ex.key.scan_code, 0);
      assert_eq!(
        key_data_ex.key_state.key_shift_state,
        protocols::simple_text_input_ex::SHIFT_STATE_VALID | protocols::simple_text_input_ex::LEFT_CONTROL_PRESSED
      );
      assert_eq!(
        key_data_ex.key_state.key_toggle_state,
        protocols::simple_text_input_ex::TOGGLE_STATE_VALID | protocols::simple_text_input_ex::KEY_STATE_EXPOSED
      );

      //send ctrl-a - expect it to be switched to control-character 0x01
      let report: &[u8] = &[0x01, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      //release keys
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data.unicode_char, 0x1);
      assert_eq!(key_data.scan_code, 0);

      //send ctrl-shift-Z - expect it to be switched to control-character 0x1A
      let report: &[u8] = &[0x03, 0x00, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      //release keys
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data.unicode_char, 0x1a);
      assert_eq!(key_data.scan_code, 0);

      // press the right logo key
      let report: &[u8] = &[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //release keys
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //make sure there is a partial key stroke in the queue
      assert_eq!(keyboard_handler.key_queue.peek_key().unwrap().key.unicode_char, 0);
      assert_eq!(keyboard_handler.key_queue.peek_key().unwrap().key.scan_code, 0);

      // press the a key
      let report: &[u8] = &[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //release keys
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      // should get only the 'a', partial keystroke should be dropped.
      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(key_data.unicode_char, 'a' as u16);
      assert_eq!(key_data.scan_code, 0);

      let status =
        simple_text_in_read_key_stroke(simple_this_ptr, &mut key_data as *mut protocols::simple_text_input::InputKey);
      assert_eq!(status, efi::Status::NOT_READY);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn wait_for_key_should_wait_for_key() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      const WAIT_FOR_KEY_EVENT: efi::Event = 0x15 as efi::Event;
      static mut KEY_HANDLER_PTR: *mut KeyboardHidHandler = core::ptr::null_mut();
      static mut START: SystemTime = UNIX_EPOCH;
      const WAIT_TIME_IN_MS: u128 = 200;
      static mut RECEIVED_EVENT: bool = false;

      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));

      unsafe { KEY_HANDLER_PTR = &mut keyboard_handler as *mut KeyboardHidHandler };

      // use a separate boot services instance for the actuall event callback - this allows
      // for different expectations on boot_services for the event callback and other functionality
      // (e.g. the receive_report call in the restore_tpl expectation below will use the expectations
      // from boot_services, not from context_boot services).
      let mut context_boot_services = MockUefiBootServices::new();
      context_boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      context_boot_services.expect_restore_tpl().returning(|_| {
        // In a real scenario, reports will be received as part of event callback that occurs at the end of
        // boot_services::restore_tpl. So here we just fake it by directly invoking the receive after a certain
        // amount of time has elapsed.
        let now = SystemTime::now();
        unsafe {
          if now.duration_since(START).unwrap().as_millis() > WAIT_TIME_IN_MS {
            START = now;
            // press and release the 'a' key
            let hid_io = MockHidIo::new();
            let report: &[u8] = &[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
            (*KEY_HANDLER_PTR).receive_report(report, &hid_io);
            let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
            (*KEY_HANDLER_PTR).receive_report(report, &hid_io);
          }
        }
      });

      context_boot_services.expect_signal_event().returning(|event| {
        if event == WAIT_FOR_KEY_EVENT {
          unsafe {
            assert!(!RECEIVED_EVENT);
            RECEIVED_EVENT = true
          };
        }
        efi::Status::SUCCESS
      });

      let context_boot_services_ptr = Box::into_raw(Box::new(context_boot_services));
      let context_boot_services = unsafe { context_boot_services_ptr.as_mut().unwrap() };
      {
        // build a simple text in context
        let context = SimpleTextInContext {
          simple_text_in: protocols::simple_text_input::Protocol {
            reset: simple_text_in_reset,
            read_key_stroke: simple_text_in_read_key_stroke,
            wait_for_key: WAIT_FOR_KEY_EVENT,
          },
          boot_services: context_boot_services,
          keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
        };
        let context_ptr = Box::into_raw(Box::new(context)) as *mut c_void;

        unsafe { START = SystemTime::now() };
        assert!(keyboard_handler.key_queue.peek_key().is_none());
        simple_text_in_wait_for_key(WAIT_FOR_KEY_EVENT, context_ptr);
        assert!(keyboard_handler.key_queue.peek_key().is_some());
        assert!(unsafe { RECEIVED_EVENT });

        drop(unsafe { Box::from_raw(context_ptr) });

        keyboard_handler.reset(false).unwrap();

        // build a simple text in context
        let context_ex = SimpleTextInExContext {
          simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
            reset: simple_text_in_ex_reset,
            read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
            set_state: simple_text_in_ex_set_state,
            register_key_notify: simple_text_in_ex_register_key_notify,
            unregister_key_notify: simple_text_in_ex_unregister_key_notify,
            wait_for_key_ex: core::ptr::null_mut(),
          },
          boot_services: context_boot_services,
          keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
        };
        let context_ptr = Box::into_raw(Box::new(context_ex)) as *mut c_void;

        unsafe { RECEIVED_EVENT = false };
        unsafe { START = SystemTime::now() };
        assert!(keyboard_handler.key_queue.peek_key().is_none());
        simple_text_in_ex_wait_for_key(WAIT_FOR_KEY_EVENT, context_ptr);
        assert!(keyboard_handler.key_queue.peek_key().is_some());
        assert!(unsafe { RECEIVED_EVENT });

        drop(unsafe { Box::from_raw(context_ptr) });
      }
      // drop the second faux static boot service
      unsafe { drop(Box::from_raw(context_boot_services_ptr)) };
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn set_state_should_set_state() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event().returning(|_| efi::Status::SUCCESS);

      extern "efiapi" fn mock_set_report(
        _this: *const hid_io::protocol::Protocol,
        _report_id: u8,
        _report_type: hid_io::protocol::HidReportType,
        _report_buffer_size: usize,
        _report_buffer: *mut c_void,
      ) -> efi::Status {
        efi::Status::SUCCESS
      }

      boot_services.expect_open_protocol().returning(|_, protocol, interface, _, _, attributes| {
        unsafe {
          assert_eq!(protocol.read(), hid_io::protocol::GUID);
          assert_eq!(attributes, efi::OPEN_PROTOCOL_GET_PROTOCOL);
          let hid_io = MaybeUninit::<hid_io::protocol::Protocol>::zeroed();
          let mut hid_io = hid_io.assume_init();
          hid_io.set_report = mock_set_report;
          // note: this will leak a hid_io instance
          interface.write(Box::into_raw(Box::new(hid_io)) as *mut c_void);
        }
        efi::Status::SUCCESS
      });

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));
      keyboard_handler.controller = Some(0x2 as efi::Handle);

      let context_ex = SimpleTextInExContext {
        simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
          reset: simple_text_in_ex_reset,
          read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
          set_state: simple_text_in_ex_set_state,
          register_key_notify: simple_text_in_ex_register_key_notify,
          unregister_key_notify: simple_text_in_ex_unregister_key_notify,
          wait_for_key_ex: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let context_ex_ptr = Box::into_raw(Box::new(context_ex));

      let mut key_toggle_state = protocols::simple_text_input_ex::KEY_STATE_EXPOSED;
      let status = simple_text_in_ex_set_state(
        context_ex_ptr as *mut protocols::simple_text_input_ex::Protocol,
        core::ptr::addr_of_mut!(key_toggle_state),
      );

      assert_eq!(status, efi::Status::SUCCESS);
      assert_ne!(
        keyboard_handler.key_queue.init_key_state().key_toggle_state
          & protocols::simple_text_input_ex::KEY_STATE_EXPOSED,
        0
      );

      keyboard_handler.controller = None;
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn register_key_notify_should_register_a_notification_callback() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      const NOTIFY_EVENT: efi::Event = 0x1 as efi::Event;
      static mut SIMPLE_TEXT_IN_EX_CTX_PTR: *mut c_void = core::ptr::null_mut();
      static mut KEY_NOTIFIED: bool = false;

      boot_services.expect_raise_tpl().returning(|_| efi::TPL_APPLICATION);
      boot_services.expect_restore_tpl().returning(|_| ());
      boot_services.expect_signal_event()
        .returning(|event| {
          if event == NOTIFY_EVENT {
            process_key_notifies(NOTIFY_EVENT, unsafe {SIMPLE_TEXT_IN_EX_CTX_PTR});            
          }
          efi::Status::SUCCESS
        });

      extern "efiapi" fn key_notify_callback (key_data: *mut protocols::simple_text_input_ex::KeyData) -> efi::Status {
        let key = unsafe {key_data.read()};
        assert_eq!(key.key.unicode_char, 'a' as u16);
        unsafe {KEY_NOTIFIED = true};
        efi::Status::SUCCESS
      }

      let agent = 0x1 as efi::Handle;
      let mut keyboard_handler = KeyboardHidHandler::new(boot_services, agent);
      let descriptor = hidparser::parse_report_descriptor(&BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap();
      keyboard_handler.process_descriptor(descriptor).unwrap();
      keyboard_handler.key_queue.set_layout(Some(hii_keyboard_layout::get_default_keyboard_layout()));
      keyboard_handler.key_notify_event = NOTIFY_EVENT;

      let hid_io = MockHidIo::new();

      let context_ex = SimpleTextInExContext {
        simple_text_in_ex: protocols::simple_text_input_ex::Protocol {
          reset: simple_text_in_ex_reset,
          read_key_stroke_ex: simple_text_in_ex_read_key_stroke,
          set_state: simple_text_in_ex_set_state,
          register_key_notify: simple_text_in_ex_register_key_notify,
          unregister_key_notify: simple_text_in_ex_unregister_key_notify,
          wait_for_key_ex: core::ptr::null_mut(),
        },
        boot_services,
        keyboard_handler: &mut keyboard_handler as *mut KeyboardHidHandler,
      };

      let context_ex_ptr = Box::into_raw(Box::new(context_ex));
      unsafe {SIMPLE_TEXT_IN_EX_CTX_PTR = context_ex_ptr as *mut c_void};

      let mut key_data: protocols::simple_text_input_ex::KeyData = Default::default();  
      key_data.key.unicode_char = 'a' as u16;

      let mut notify_handle = core::ptr::null_mut();

      let status = simple_text_in_ex_register_key_notify (
        context_ex_ptr as *mut protocols::simple_text_input_ex::Protocol, 
        &mut key_data as *mut protocols::simple_text_input_ex::KeyData, 
        key_notify_callback, 
        core::ptr::addr_of_mut!(notify_handle));

      assert_eq!(status, efi::Status::SUCCESS);
      assert_eq!(keyboard_handler.notification_callbacks.len(), 1);
      assert!(keyboard_handler.notification_callbacks.contains_key(&1));
      assert_eq!(keyboard_handler.notification_callbacks.get(&1).unwrap().0, OrdKeyData(key_data));
      assert_eq!(keyboard_handler.next_notify_handle, 1);
      assert_eq!(notify_handle as usize, 1);

      //send 'b'
      let report: &[u8] = &[0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //release
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(!unsafe {KEY_NOTIFIED});

      //send 'a'
      let report: &[u8] = &[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);

      //release
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
      keyboard_handler.receive_report(report, &hid_io);
      assert!(unsafe {KEY_NOTIFIED});

      drop(unsafe{Box::from_raw(context_ex_ptr)});
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }
}
