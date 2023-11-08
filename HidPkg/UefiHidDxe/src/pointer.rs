use core::ffi::c_void;

use alloc::{
  boxed::Box,
  collections::{BTreeMap, BTreeSet},
  vec::Vec,
};
use hidparser::{
  report_data_types::{ReportId, Usage},
  ReportDescriptor, ReportField, VariableField,
};
use r_efi::{efi, protocols::absolute_pointer};

#[cfg(not(test))]
use rust_advanced_logger_dxe::{debugln, DEBUG_ERROR, DEBUG_INFO, DEBUG_WARN};

use crate::{
  boot_services::UefiBootServices,
  hid_io::{HidIo, HidReportReciever},
};

// Usages supported by this module.
const GENERIC_DESKTOP_X: u32 = 0x00010030;
const GENERIC_DESKTOP_Y: u32 = 0x00010031;
const GENERIC_DESKTOP_Z: u32 = 0x00010032;
const GENERIC_DESKTOP_WHEEL: u32 = 0x00010038;
const BUTTON_PAGE: u16 = 0x0009;
const BUTTON_MIN: u32 = 0x00090001;
const BUTTON_MAX: u32 = 0x00090020; //Per spec, the Absolute Pointer protocol supports a 32-bit button state field.

// number of points on the X/Y axis for this implementation.
const AXIS_RESOLUTION: u64 = 1024;
const CENTER: u64 = AXIS_RESOLUTION / 2;

// Maps a given field to a routine that handles input from it.
#[derive(Debug, Clone)]
struct ReportFieldWithHandler {
  field: VariableField,
  report_handler: fn(&mut PointerHidHandler, field: VariableField, report: &[u8]),
}

// Defines a report and the fields of interest within it.
#[derive(Debug, Default, Clone)]
struct PointerReportData {
  report_id: Option<ReportId>,
  report_size: usize,
  relevant_fields: Vec<ReportFieldWithHandler>,
}

pub struct PointerHidHandler {
  boot_services: &'static dyn UefiBootServices,
  agent: efi::Handle,
  controller: Option<efi::Handle>,
  input_reports: BTreeMap<Option<ReportId>, PointerReportData>,
  supported_usages: BTreeSet<Usage>,
  report_id_present: bool,
  state_changed: bool,
  current_state: absolute_pointer::State,
}

// FFI context
// Safety: a pointer to PointerHidHandler is included in the context so that it can be reclaimed in the absolute_pointer
// API implementation. Care must be taken to ensure that rust invariants are respected when accessing the
// PointerHidHandler. In particular, the design must ensure mutual exclusion on the PointerHidHandler between callbacks
// running at different TPL; this is accomplished by ensuring all access to the structure is at TPL_NOTIFY once
// initialization is complete - for this reason the context structure includes a direct reference to boot_services so
// that TPL can be enforced without access to the *mut PointerHidHandle.
//
// In addition, the absolute_pointer element needs to be the first element in the structure so that the full structure
// can be recovered by simple casting for absolute_pointer FFI interfaces that only receive a pointer to the
// absolute_pointer structure.
#[repr(C)]
pub struct PointerContext {
  absolute_pointer: absolute_pointer::Protocol,
  boot_services: &'static dyn UefiBootServices,
  pointer_handler: *mut PointerHidHandler,
}

impl Drop for PointerContext {
  fn drop(&mut self) {
    if !self.absolute_pointer.mode.is_null() {
      drop(unsafe { Box::from_raw(self.absolute_pointer.mode) });
    }
  }
}

impl PointerHidHandler {
  pub fn new(boot_services: &'static dyn UefiBootServices, agent: efi::Handle) -> Self {
    let mut handler = Self {
      boot_services,
      agent,
      controller: None,
      input_reports: BTreeMap::new(),
      supported_usages: BTreeSet::new(),
      report_id_present: false,
      state_changed: false,
      current_state: Default::default(),
    };
    handler.reset_state();
    handler
  }
  fn process_descriptor(&mut self, descriptor: ReportDescriptor) -> Result<(), efi::Status> {
    let multiple_reports = descriptor.input_reports.len() > 1;

    for report in &descriptor.input_reports {
      let mut report_data = PointerReportData { report_id: report.report_id, ..Default::default() };

      self.report_id_present = report.report_id.is_some();

      if multiple_reports && !self.report_id_present {
        //invalid to have None ReportId if multiple reports present.
        Err(efi::Status::DEVICE_ERROR)?;
      }

      report_data.report_size = report.size_in_bits.div_ceil(8);

      for field in &report.fields {
        match field {
          ReportField::Variable(field) => {
            match field.usage.into() {
              GENERIC_DESKTOP_X => {
                let field_handler =
                  ReportFieldWithHandler { field: field.clone(), report_handler: Self::x_axis_handler };
                report_data.relevant_fields.push(field_handler);
                self.supported_usages.insert(field.usage);
              }
              GENERIC_DESKTOP_Y => {
                let field_handler =
                  ReportFieldWithHandler { field: field.clone(), report_handler: Self::y_axis_handler };
                report_data.relevant_fields.push(field_handler);
                self.supported_usages.insert(field.usage);
              }
              GENERIC_DESKTOP_Z | GENERIC_DESKTOP_WHEEL => {
                let field_handler =
                  ReportFieldWithHandler { field: field.clone(), report_handler: Self::z_axis_handler };
                report_data.relevant_fields.push(field_handler);
                self.supported_usages.insert(field.usage);
              }
              BUTTON_MIN..=BUTTON_MAX => {
                let field_handler =
                  ReportFieldWithHandler { field: field.clone(), report_handler: Self::button_handler };
                report_data.relevant_fields.push(field_handler);
                self.supported_usages.insert(field.usage);
              }
              _ => (), //other usages irrelevant
            }
          }
          _ => (), // other field types irrelevant
        }
      }

      if report_data.relevant_fields.len() > 0 {
        self.input_reports.insert(report_data.report_id, report_data);
      }
    }

    if self.input_reports.len() > 0 {
      Ok(())
    } else {
      Err(efi::Status::UNSUPPORTED)
    }
  }

  // Helper routine that handles projecting relative and absolute axis reports onto the fixed
  // absolute report axis that this driver produces.
  fn resolve_axis(current_value: u64, field: VariableField, report: &[u8]) -> Option<u64> {
    if field.attributes.relative {
      //for relative, just update and clamp the current state.
      let new_value = current_value as i64 + field.field_value(report)?;
      return Some(new_value.clamp(0, AXIS_RESOLUTION as i64) as u64);
    } else {
      //for absolute, project onto 0..AXIS_RESOLUTION
      let mut new_value = field.field_value(report)?;

      //translate to zero.
      new_value = new_value.checked_sub(i32::from(field.logical_minimum) as i64)?;

      //scale to AXIS_RESOLUTION
      new_value = (new_value * AXIS_RESOLUTION as i64 * 1000) / (field.field_range()? as i64 * 1000);

      return Some(new_value.clamp(0, AXIS_RESOLUTION as i64) as u64);
    }
  }

  // handles x_axis inputs
  fn x_axis_handler(&mut self, field: VariableField, report: &[u8]) {
    if let Some(x_value) = Self::resolve_axis(self.current_state.current_x, field, report) {
      if self.current_state.current_x != x_value {
        self.current_state.current_x = x_value;
        self.state_changed = true;
      }
    }
  }

  // handles y_axis inputs
  fn y_axis_handler(&mut self, field: VariableField, report: &[u8]) {
    if let Some(y_value) = Self::resolve_axis(self.current_state.current_y, field, report) {
      if self.current_state.current_y != y_value {
        self.current_state.current_y = y_value;
        self.state_changed = true;
      }
    }
  }

  // handles z_axis inputs
  fn z_axis_handler(&mut self, field: VariableField, report: &[u8]) {
    if let Some(z_value) = Self::resolve_axis(self.current_state.current_z, field, report) {
      if self.current_state.current_z != z_value {
        self.current_state.current_z = z_value;
        self.state_changed = true;
      }
    }
  }

  // handles button inputs
  fn button_handler(&mut self, field: VariableField, report: &[u8]) {
    let shift: u32 = field.usage.into();
    if (shift < BUTTON_MIN) || (shift > BUTTON_MAX) {
      return;
    }

    if let Some(button_value) = field.field_value(report) {
      let button_value = button_value as u32;

      let shift = shift - BUTTON_MIN;
      if shift > u32::BITS {
        return;
      }
      let button_value = button_value << shift;

      let new_buttons = self.current_state.active_buttons
        & !(1 << shift)  // zero the relevant bit in the button state field.
        | button_value; // or in the current button state into that bit position.

      if new_buttons != self.current_state.active_buttons {
        self.current_state.active_buttons = new_buttons;
        self.state_changed = true;
      }
    }
  }

  fn install_protocol_interfaces(
    &mut self,
    boot_services: &'static dyn UefiBootServices,
    controller: efi::Handle,
  ) -> Result<(), efi::Status> {
    // Create pointer context.
    let pointer_ctx = PointerContext {
      absolute_pointer: absolute_pointer::Protocol {
        reset: absolute_pointer_reset,
        get_state: absolute_pointer_get_state,
        mode: Box::into_raw(Box::new(self.initialize_mode())),
        wait_for_input: core::ptr::null_mut(),
      },
      boot_services,
      pointer_handler: self as *mut PointerHidHandler,
    };

    let absolute_pointer_ptr = Box::into_raw(Box::new(pointer_ctx));

    // create event for wait_for_input.
    let mut wait_for_pointer_input_event: efi::Event = core::ptr::null_mut();
    let status = boot_services.create_event(
      efi::EVT_NOTIFY_WAIT,
      efi::TPL_NOTIFY,
      Some(wait_for_pointer),
      absolute_pointer_ptr as *mut c_void,
      core::ptr::addr_of_mut!(wait_for_pointer_input_event),
    );
    if status.is_error() {
      drop(unsafe { Box::from_raw(absolute_pointer_ptr) });
      return Err(status);
    }

    unsafe { (*absolute_pointer_ptr).absolute_pointer.wait_for_input = wait_for_pointer_input_event };

    // install the absolute_pointer protocol.
    let mut controller = controller;
    let status = boot_services.install_protocol_interface(
      core::ptr::addr_of_mut!(controller),
      &absolute_pointer::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
      efi::NATIVE_INTERFACE,
      absolute_pointer_ptr as *mut c_void,
    );

    if status.is_error() {
      let _ = self.boot_services.close_event(wait_for_pointer_input_event);
      drop(unsafe { Box::from_raw(absolute_pointer_ptr) });
      return Err(status);
    }

    self.controller = Some(controller);

    // after this point, access to self must be guarded by raising TPL to NOTIFY.
    Ok(())
  }

  // Initializes the absolute_pointer mode structure.
  fn initialize_mode(&self) -> absolute_pointer::Mode {
    let mut mode: absolute_pointer::Mode = Default::default();

    if self.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_X)) {
      mode.absolute_max_x = AXIS_RESOLUTION;
      mode.absolute_min_x = 0;
    } else {
      #[cfg(not(test))]
      debugln!(DEBUG_WARN, "No x-axis usages found in the report descriptor.");
    }

    if self.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_Y)) {
      mode.absolute_max_y = AXIS_RESOLUTION;
      mode.absolute_min_y = 0;
    } else {
      #[cfg(not(test))]
      debugln!(DEBUG_WARN, "No y-axis usages found in the report descriptor.");
    }

    if (self.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_Z)))
      || (self.supported_usages.contains(&Usage::from(GENERIC_DESKTOP_WHEEL)))
    {
      mode.absolute_max_z = AXIS_RESOLUTION;
      mode.absolute_min_z = 0;
      //TODO: Z-axis is interpreted as pressure data. This is for compat with reference implementation in C, but
      //could consider e.g. looking for actual digitizer tip pressure usages or something.
      mode.attributes = mode.attributes | 0x02;
    } else {
      #[cfg(not(test))]
      debugln!(DEBUG_INFO, "No z-axis usages found in the report descriptor.");
    }

    let button_count = self.supported_usages.iter().filter(|x| x.page() == BUTTON_PAGE).count();

    if button_count > 1 {
      mode.attributes = mode.attributes | 0x01; // alternate button exists.
    }

    mode
  }

  fn reset_state(&mut self) {
    self.current_state = Default::default();
    // initialize pointer to center of screen
    self.current_state.current_x = CENTER;
    self.current_state.current_y = CENTER;
    self.state_changed = false;
  }
}

impl HidReportReciever for PointerHidHandler {
  fn initialize(&mut self, controller: efi::Handle, hid_io: &dyn HidIo) -> Result<(), efi::Status> {
    let descriptor = hid_io.get_report_descriptor()?;
    self.process_descriptor(descriptor)?;

    self.install_protocol_interfaces(self.boot_services, controller)?;

    Ok(())
  }
  fn receive_report(&mut self, report: &[u8], _hid_io: &dyn HidIo) {
    let old_tpl = self.boot_services.raise_tpl(efi::TPL_NOTIFY);

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

        // hand the report data to the handler for each relevant field for field-specific processing.
        for field in report_data.relevant_fields {
          (field.report_handler)(self, field.field, report);
        }
      }
    }

    self.boot_services.restore_tpl(old_tpl);
  }
}

impl Drop for PointerHidHandler {
  fn drop(&mut self) {
    if let Some(controller) = self.controller {
      //Controller is set - that means initialize() was called, and there is potential state exposed thru FFI that needs
      //to be cleaned up.

      let mut absolute_pointer_ptr: *mut PointerContext = core::ptr::null_mut();
      let status = self.boot_services.open_protocol(
        controller,
        &absolute_pointer::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        core::ptr::addr_of_mut!(absolute_pointer_ptr) as *mut *mut c_void,
        self.agent,
        controller,
        efi::OPEN_PROTOCOL_GET_PROTOCOL,
      );
      if status.is_error() {
        //No protocol is actually installed on this controller, so nothing to clean up.
        return;
      }

      //Attempt to uninstall the absolute_pointer interface - this should disconnect any drivers using it and release
      //the interface.
      let status = self.boot_services.uninstall_protocol_interface(
        controller,
        &absolute_pointer::PROTOCOL_GUID as *const efi::Guid as *mut efi::Guid,
        absolute_pointer_ptr as *mut c_void,
      );
      if status.is_error() {
        //An error here means some other driver might be holding on to the absolute_pointer_ptr.
        //Mark the instance invalid by setting the pointer_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on absolute_pointer_ptr to continue to resolve
        //and return error based on observing pointer_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to uninstall absolute_pointer interface, status: {:x?}", status);

        unsafe {
          (*absolute_pointer_ptr).pointer_handler = core::ptr::null_mut();
        }
        //return without tearing down the context.
        return;
      }

      let wait_for_input_event: efi::Handle = unsafe { (*absolute_pointer_ptr).absolute_pointer.wait_for_input };
      let status = self.boot_services.close_event(wait_for_input_event);
      if status.is_error() {
        //An error here means the event was not uninstalled, so in theory the notification_callback on it could still be
        //fired.
        //Mark the instance invalid by setting the pointer_handler raw pointer to null, but leak the PointerContext
        //instance. Leaking context allows calls through the pointers on absolute_pointer_ptr to continue to resolve
        //and return error based on observing pointer_handler is null.
        #[cfg(not(test))]
        debugln!(DEBUG_ERROR, "Failed to close absolute_pointer.wait_for_input event, status: {:x?}", status);
        unsafe {
          (*absolute_pointer_ptr).pointer_handler = core::ptr::null_mut();
        }
        return;
      }

      // None of the parts of absolute pointer are in use, so it is safe to reclaim it.
      drop(unsafe { Box::from_raw(absolute_pointer_ptr) });
    }
  }
}

// event handler for wait_for_pointer event that is part of the absolute pointer interface.
extern "efiapi" fn wait_for_pointer(event: efi::Event, context: *mut c_void) {
  let pointer_ctx = unsafe { (context as *mut PointerContext).as_mut().expect("bad context") };
  let mut signal_event: bool = false;

  {
    // raise to notify to protect access to pointer_handler, and check if event should be signalled.
    let old_tpl = pointer_ctx.boot_services.raise_tpl(efi::TPL_NOTIFY);

    let pointer_handler = unsafe { pointer_ctx.pointer_handler.as_mut() };
    if let Some(pointer_handler) = pointer_handler {
      signal_event = pointer_handler.state_changed;
    } else {
      // implies that this API was invoked after pointer handler was dropped.
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "absolute_pointer_reset invoked after pointer dropped.");
    }

    pointer_ctx.boot_services.restore_tpl(old_tpl);
  }

  if signal_event {
    pointer_ctx.boot_services.signal_event(event);
  }
}

// resets the pointer state - part of the absolute pointer interface.
extern "efiapi" fn absolute_pointer_reset(
  this: *mut absolute_pointer::Protocol,
  _extended_verification: bool,
) -> efi::Status {
  if this.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }
  let pointer_ctx = unsafe { (this as *mut PointerContext).as_mut().expect("bad context") };
  let mut status = efi::Status::SUCCESS;
  {
    // raise to notify to protect access to pointer_handler and reset pointer handler state
    let old_tpl = pointer_ctx.boot_services.raise_tpl(efi::TPL_NOTIFY);

    let pointer_handler = unsafe { pointer_ctx.pointer_handler.as_mut() };
    if let Some(pointer_handler) = pointer_handler {
      pointer_handler.reset_state();
    } else {
      // implies that this API was invoked after pointer handler was dropped.
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "absolute_pointer_reset invoked after pointer dropped.");
      status = efi::Status::DEVICE_ERROR;
    }

    pointer_ctx.boot_services.restore_tpl(old_tpl);
  }
  status
}

// returns the current pointer state in the `state` buffer provided by the caller - part of the absolute pointer
// interface.
extern "efiapi" fn absolute_pointer_get_state(
  this: *mut absolute_pointer::Protocol,
  state: *mut absolute_pointer::State,
) -> efi::Status {
  if state.is_null() || state.is_null() {
    return efi::Status::INVALID_PARAMETER;
  }

  let pointer_ctx = unsafe { (this as *mut PointerContext).as_mut().expect("bad context") };
  let mut status = efi::Status::SUCCESS;
  {
    // raise to notify to protect access to pointer_handler, and retrieve pointer handler state.
    let old_tpl = pointer_ctx.boot_services.raise_tpl(efi::TPL_NOTIFY);

    let pointer_handler = unsafe { pointer_ctx.pointer_handler.as_mut() };
    if let Some(pointer_handler) = pointer_handler {
      if pointer_handler.state_changed {
        unsafe {
          state.write(pointer_handler.current_state);
        }
        pointer_handler.state_changed = false;
      } else {
        status = efi::Status::NOT_READY;
      }
    } else {
      // implies that this API was invoked after pointer handler was dropped.
      #[cfg(not(test))]
      debugln!(DEBUG_ERROR, "absolute_pointer_get_state invoked after pointer dropped.");
      status = efi::Status::DEVICE_ERROR;
    }

    pointer_ctx.boot_services.restore_tpl(old_tpl);
  }
  status
}

#[cfg(test)]
mod test {
  use core::{cmp::min, ffi::c_void};

  use crate::{
    boot_services::MockUefiBootServices,
    hid_io::{HidReportReciever, MockHidIo},
    pointer::{
      absolute_pointer_get_state, absolute_pointer_reset, wait_for_pointer, PointerContext, AXIS_RESOLUTION, CENTER,
    },
  };
  use r_efi::{efi, protocols::absolute_pointer};

  use super::PointerHidHandler;

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

  static ABS_POINTER_REPORT_DESCRIPTOR: &[u8] = &[
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
    0x15, 0x00, //     LOGICAL_MINIMUM (0)
    0x26, 0xff, 0x0f, // LOGICAL_MAXIMUM (4095)
    0x75, 0x10, //     REPORT_SIZE (16)
    0x95, 0x03, //     REPORT_COUNT (3)
    0x81, 0x02, //     INPUT(Data, Variable, Absolute)
    0xc0, //   END_COLLECTION
    0xc0, // END_COLLECTION
  ];

  #[test]
  fn pointer_initialize_should_fail_if_report_descriptor_not_supported() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      let agent = 0x1 as efi::Handle;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MINIMAL_BOOT_KEYBOARD_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Err(efi::Status::UNSUPPORTED));
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn successful_pointer_initialize_should_install_protocol_and_drop_should_tear_it_down() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, _, _, _| efi::Status::SUCCESS);
      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      let agent = 0x1 as efi::Handle;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_ne!(unsafe { ABS_PTR_INTERFACE }, core::ptr::null_mut());
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn receive_report_should_process_relative_reports() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, _, _, _| efi::Status::SUCCESS);
      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      let agent = 0x1 as efi::Handle;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //click two buttons and move the cursor (+32,+32)
      let report: &[u8] = &[0x05, 0x20, 0x20, 0];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0x05);
      assert_eq!(pointer_handler.current_state.current_x, CENTER + 32);
      assert_eq!(pointer_handler.current_state.current_y, CENTER + 32);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, true);

      //unclick and move the cursor (+32,-16) and wheel(+32).
      let report: &[u8] = &[0x00, 0x20, 0xF0, 0x20]; //0xF0 = -16.
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER + 64);
      assert_eq!(pointer_handler.current_state.current_y, CENTER + 16);
      assert_eq!(pointer_handler.current_state.current_z, 32);

      //unclick and move the cursor (+32,-32) and wheel(+32).
      let report: &[u8] = &[0x00, 0x20, 0xE0, 0x20]; //0xE0 = -32.
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER + 96);
      assert_eq!(pointer_handler.current_state.current_y, CENTER - 16);
      assert_eq!(pointer_handler.current_state.current_z, 64);

      //move the cursor (0,127) until is past saturation, and check the value each time
      let report: &[u8] = &[0x00, 0x00, 0x7F, 0x0]; //0x7F = +127.
      let starting_y = pointer_handler.current_state.current_y;
      for i in 0..AXIS_RESOLUTION {
        //starts near the cetner, so moving it well past saturation point
        pointer_handler.receive_report(report, &hid_io);
        let expected_y = min(AXIS_RESOLUTION, starting_y.saturating_add((i + 1) * 127));
        assert_eq!(pointer_handler.current_state.current_y, expected_y);
      }

      //move the cursor(0,-127) until it saturates at zero, and check the value each time.
      let report: &[u8] = &[0x00, 0x00, 0x81, 0x0]; //0x80 = -127.
      let starting_y = pointer_handler.current_state.current_y;
      for i in 0..AXIS_RESOLUTION {
        // starts at max, but moving 127 each time, so well past saturation point.
        pointer_handler.receive_report(report, &hid_io);
        let expected_y = starting_y.saturating_sub((i + 1) * 127);
        assert_eq!(pointer_handler.current_state.current_y, expected_y);
      }
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn receive_report_should_process_absolute_reports() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, _, _, _| efi::Status::SUCCESS);
      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      let agent = 0x1 as efi::Handle;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&ABS_POINTER_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //click two buttons and move the cursor (1024, 1024).
      let report: &[u8] = &[0x05, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0x05);
      // input x from range 0-4095 projected on to 0-1024 axis is (x/4095) * 1024. For x=1024, result is 256.
      assert_eq!(pointer_handler.current_state.current_x, 256);
      assert_eq!(pointer_handler.current_state.current_y, 256);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, true);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn bad_reports_should_be_ignored() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, _, _, _| efi::Status::SUCCESS);
      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      let agent = 0x1 as efi::Handle;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&ABS_POINTER_REPORT_DESCRIPTOR).unwrap()));

      let controller = 0x2 as efi::Handle;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //move the cursor (4096, 4096, 0) - changed fields are out of range
      let report: &[u8] = &[0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //report too long
      let report: &[u8] = &[0x00, 0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x10];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //report too short
      let report: &[u8] = &[0x00, 0x10, 0x00, 0x10, 0x00, 0x10];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn wait_for_event_should_wait_for_event() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      const AGENT_HANDLE: efi::Handle = 0x01 as efi::Handle;
      const CONTROLLER_HANDLE: efi::Handle = 0x02 as efi::Handle;
      const POINTER_EVENT: efi::Event = 0x03 as efi::Event;

      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();
      static mut EVENT_CONTEXT: *mut c_void = core::ptr::null_mut();
      static mut EVENT_SIGNALED: bool = false;

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, wait_for_ptr, context, event_ptr| {
        assert!(wait_for_ptr == Some(wait_for_pointer));
        assert_ne!(context, core::ptr::null_mut());
        unsafe {
          EVENT_CONTEXT = context;
          event_ptr.write(POINTER_EVENT);
        }
        efi::Status::SUCCESS
      });

      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      boot_services.expect_signal_event().returning(|event| {
        assert_eq!(event, POINTER_EVENT);
        unsafe { EVENT_SIGNALED = true };
        efi::Status::SUCCESS
      });

      let agent = AGENT_HANDLE;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = CONTROLLER_HANDLE;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      let absolute_pointer = unsafe { (ABS_PTR_INTERFACE as *mut PointerContext).as_mut() }.unwrap();
      assert_eq!(absolute_pointer.absolute_pointer.wait_for_input, POINTER_EVENT);

      // no pointer state change - should not signal event.
      wait_for_pointer(POINTER_EVENT, unsafe { EVENT_CONTEXT });

      assert_eq!(unsafe { EVENT_SIGNALED }, false);

      //click two buttons and move the cursor (+32,+32)
      let report: &[u8] = &[0x05, 0x20, 0x20, 0];
      pointer_handler.receive_report(report, &hid_io);

      // pointer state change in place - should signal event.
      wait_for_pointer(POINTER_EVENT, unsafe { EVENT_CONTEXT });
      assert_eq!(unsafe { EVENT_SIGNALED }, true);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn absolute_pointer_reset_should_reset_pointer_state() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      const AGENT_HANDLE: efi::Handle = 0x01 as efi::Handle;
      const CONTROLLER_HANDLE: efi::Handle = 0x02 as efi::Handle;
      const EVENT_HANDLE: efi::Handle = 0x03 as efi::Handle;

      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();
      static mut EVENT_CONTEXT: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, wait_for_ptr, context, event_ptr| {
        assert!(wait_for_ptr == Some(wait_for_pointer));
        assert_ne!(context, core::ptr::null_mut());
        unsafe {
          EVENT_CONTEXT = context;
          event_ptr.write(EVENT_HANDLE);
        }
        efi::Status::SUCCESS
      });

      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      let agent = AGENT_HANDLE;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = CONTROLLER_HANDLE;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //click two buttons and move the cursor (+32,+32,+32)
      let report: &[u8] = &[0x05, 0x20, 0x20, 0x20];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0x5);
      assert_eq!(pointer_handler.current_state.current_x, CENTER + 0x20);
      assert_eq!(pointer_handler.current_state.current_y, CENTER + 0x20);
      assert_eq!(pointer_handler.current_state.current_z, 0x20);
      assert_eq!(pointer_handler.state_changed, true);

      //reset state
      let status = absolute_pointer_reset(unsafe { ABS_PTR_INTERFACE as *mut absolute_pointer::Protocol }, false);
      assert_eq!(status, efi::Status::SUCCESS);

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }

  #[test]
  fn absolute_pointer_get_state_should_return_current_state_and_clear_changed_flag() {
    // usage model for boot_services is global static, and so this implementation use &'static dyn UefiBootServices.
    // to emulate this without actually creating a static, use a raw pointer.
    let raw_boot_services = Box::into_raw(Box::new(MockUefiBootServices::new()));
    let boot_services = unsafe { raw_boot_services.as_mut().unwrap() };

    {
      const AGENT_HANDLE: efi::Handle = 0x01 as efi::Handle;
      const CONTROLLER_HANDLE: efi::Handle = 0x02 as efi::Handle;
      const EVENT_HANDLE: efi::Handle = 0x03 as efi::Handle;

      static mut ABS_PTR_INTERFACE: *mut c_void = core::ptr::null_mut();
      static mut EVENT_CONTEXT: *mut c_void = core::ptr::null_mut();

      // expected on PointerHidHandler::initialize().
      boot_services.expect_create_event().returning(|_, _, wait_for_ptr, context, event_ptr| {
        assert!(wait_for_ptr == Some(wait_for_pointer));
        assert_ne!(context, core::ptr::null_mut());
        unsafe {
          EVENT_CONTEXT = context;
          event_ptr.write(EVENT_HANDLE);
        }
        efi::Status::SUCCESS
      });

      boot_services.expect_install_protocol_interface().returning(|_, _, _, interface| {
        unsafe { ABS_PTR_INTERFACE = interface };
        efi::Status::SUCCESS
      });

      // expected on PointerHidHandler::drop().
      boot_services.expect_open_protocol().returning(|_, _, interface, _, _, _| {
        unsafe { *interface = ABS_PTR_INTERFACE };
        efi::Status::SUCCESS
      });
      boot_services.expect_uninstall_protocol_interface().returning(|_, _, _| efi::Status::SUCCESS);
      boot_services.expect_close_event().returning(|_| efi::Status::SUCCESS);

      // expected on PointerHidHandler::receive_report
      boot_services.expect_raise_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_NOTIFY);
        efi::TPL_APPLICATION
      });

      boot_services.expect_restore_tpl().returning(|new_tpl| {
        assert_eq!(new_tpl, efi::TPL_APPLICATION);
        ()
      });

      let agent = AGENT_HANDLE;
      let mut pointer_handler = PointerHidHandler::new(boot_services, agent);
      let mut hid_io = MockHidIo::new();
      hid_io
        .expect_get_report_descriptor()
        .returning(|| Ok(hidparser::parse_report_descriptor(&MOUSE_REPORT_DESCRIPTOR).unwrap()));

      let controller = CONTROLLER_HANDLE;
      assert_eq!(pointer_handler.initialize(controller, &hid_io), Ok(()));

      assert_eq!(pointer_handler.current_state.active_buttons, 0);
      assert_eq!(pointer_handler.current_state.current_x, CENTER);
      assert_eq!(pointer_handler.current_state.current_y, CENTER);
      assert_eq!(pointer_handler.current_state.current_z, 0);
      assert_eq!(pointer_handler.state_changed, false);

      //click two buttons and move the cursor (+32,+32,+32)
      let report: &[u8] = &[0x05, 0x20, 0x20, 0x20];
      pointer_handler.receive_report(report, &hid_io);

      assert_eq!(pointer_handler.current_state.active_buttons, 0x5);
      assert_eq!(pointer_handler.current_state.current_x, CENTER + 0x20);
      assert_eq!(pointer_handler.current_state.current_y, CENTER + 0x20);
      assert_eq!(pointer_handler.current_state.current_z, 0x20);
      assert_eq!(pointer_handler.state_changed, true);

      let mut absolute_pointer_state: absolute_pointer::State = Default::default();
      let status = absolute_pointer_get_state(
        unsafe { ABS_PTR_INTERFACE as *mut absolute_pointer::Protocol },
        &mut absolute_pointer_state as *mut absolute_pointer::State,
      );
      assert_eq!(status, efi::Status::SUCCESS);

      assert_eq!(absolute_pointer_state.current_x, pointer_handler.current_state.current_x);
      assert_eq!(absolute_pointer_state.current_y, pointer_handler.current_state.current_y);
      assert_eq!(absolute_pointer_state.current_z, pointer_handler.current_state.current_z);
      assert_eq!(absolute_pointer_state.active_buttons, pointer_handler.current_state.active_buttons);
      assert_eq!(pointer_handler.state_changed, false);

      //if get_state is attempted when there are no changes to state, it should return NOT_READY.
      let mut absolute_pointer_state: absolute_pointer::State = Default::default();
      let status = absolute_pointer_get_state(
        unsafe { ABS_PTR_INTERFACE as *mut absolute_pointer::Protocol },
        &mut absolute_pointer_state as *mut absolute_pointer::State,
      );
      assert_eq!(status, efi::Status::NOT_READY);
    }

    //drop the faux static boot services.
    unsafe { drop(Box::from_raw(raw_boot_services)) };
  }
}
